use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, request::Parts},
};

use crate::error::{AppError, AppResult};

pub struct ClientAddress(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for ClientAddress {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(address)| *address),
        ))
    }
}

/// A single IP overwritten by the configured ingress, never a forwarded chain.
#[derive(Clone, Copy, Debug, Default)]
pub enum ProxyIpHeader {
    #[default]
    XForwardedFor,
    CfConnectingIp,
}
impl FromStr for ProxyIpHeader {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "x-forwarded-for" => Ok(Self::XForwardedFor),
            "cf-connecting-ip" => Ok(Self::CfConnectingIp),
            _ => {
                anyhow::bail!("TRUSTED_PROXY_IP_HEADER must be x-forwarded-for or cf-connecting-ip")
            }
        }
    }
}
impl ProxyIpHeader {
    fn name(self) -> &'static str {
        match self {
            Self::XForwardedFor => "x-forwarded-for",
            Self::CfConnectingIp => "cf-connecting-ip",
        }
    }
}
#[derive(Default)]
struct Buckets {
    entries: HashMap<String, VecDeque<Instant>>,
    last_sweep: Option<Instant>,
}
const MAX_IDENTITIES: usize = 10_000;
#[derive(Clone)]
pub struct LoginRateLimiter {
    attempts: Arc<Mutex<Buckets>>,
    limit: usize,
    proxy_header: ProxyIpHeader,
    window: Duration,
    message: &'static str,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self {
            attempts: Arc::new(Mutex::new(Buckets::default())),
            limit: 10,
            proxy_header: ProxyIpHeader::default(),
            window: Duration::from_secs(60),
            message: "too many authentication attempts; try again shortly",
        }
    }
}

impl LoginRateLimiter {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            attempts: Arc::new(Mutex::new(Buckets::default())),
            limit,
            proxy_header: ProxyIpHeader::default(),
            window: Duration::from_secs(60),
            message: "too many requests; try again shortly",
        }
    }

    pub fn with_proxy_header(mut self, header: ProxyIpHeader) -> Self {
        self.proxy_header = header;
        self
    }

    pub fn check(
        &self,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
        trust_proxy_headers: bool,
    ) -> AppResult<()> {
        let key = client_key(headers, peer, trust_proxy_headers, self.proxy_header);
        self.check_key(&key)
    }

    pub fn check_key(&self, key: &str) -> AppResult<()> {
        let now = Instant::now();
        let cutoff = now - self.window;
        let mut attempts = self.attempts.lock().map_err(|_| AppError::InvalidRequest {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            message: "rate limiter is unavailable".to_owned(),
        })?;
        if attempts
            .last_sweep
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
        {
            attempts
                .entries
                .retain(|_, entries| entries.back().is_some_and(|instant| *instant > cutoff));
            attempts.last_sweep = Some(now);
        }
        if key.len() > 128
            || (!attempts.entries.contains_key(key) && attempts.entries.len() >= MAX_IDENTITIES)
        {
            return Err(AppError::InvalidRequest {
                status: axum::http::StatusCode::TOO_MANY_REQUESTS,
                message: self.message.to_owned(),
            });
        }
        let entries = attempts.entries.entry(key.to_owned()).or_default();
        while entries.front().is_some_and(|instant| *instant <= cutoff) {
            entries.pop_front();
        }
        if entries.len() >= self.limit {
            return Err(AppError::InvalidRequest {
                status: axum::http::StatusCode::TOO_MANY_REQUESTS,
                message: self.message.to_owned(),
            });
        }
        entries.push_back(now);
        Ok(())
    }
}

fn client_key(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    trust_proxy_headers: bool,
    proxy_header: ProxyIpHeader,
) -> String {
    if trust_proxy_headers {
        let mut values = headers.get_all(proxy_header.name()).iter();
        let first = values.next();
        if values.next().is_none()
            && let Some(ip) = first
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<IpAddr>().ok())
        {
            return ip.to_string();
        }
    }
    peer.map(|peer| peer.ip().to_string())
        .unwrap_or_else(|| "unknown-peer".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_identity_buckets_have_a_hard_capacity_limit() {
        let limiter = LoginRateLimiter::default();
        for n in 0..10000 {
            limiter.check_key(&format!("fixture-{n}")).unwrap();
        }
        assert!(limiter.check_key("one-more-identity").is_err());
        assert_eq!(limiter.attempts.lock().unwrap().entries.len(), 10000);
    }

    #[test]
    fn limits_repeated_attempts_for_the_same_peer() {
        let limiter = LoginRateLimiter::default();
        let peer = Some("127.0.0.1:4000".parse().unwrap());
        for _ in 0..10 {
            limiter.check(&HeaderMap::new(), peer, false).unwrap();
        }
        assert!(limiter.check(&HeaderMap::new(), peer, false).is_err());
    }

    #[test]
    fn selected_ingress_header_is_single_validated_and_canonical() {
        let peer = Some("127.0.0.1:4000".parse().unwrap());
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.1, 10.0.0.1".parse().unwrap());
        headers.insert("cf-connecting-ip", "2001:0db8:0:0::1".parse().unwrap());
        assert_eq!(
            client_key(&headers, peer, true, ProxyIpHeader::CfConnectingIp),
            "2001:db8::1"
        );
        assert_eq!(
            client_key(&headers, peer, true, ProxyIpHeader::XForwardedFor),
            "127.0.0.1"
        );
        headers.append("cf-connecting-ip", "203.0.113.2".parse().unwrap());
        assert_eq!(
            client_key(&headers, peer, true, ProxyIpHeader::CfConnectingIp),
            "127.0.0.1"
        );
        headers.insert("x-forwarded-for", "203.0.113.4".parse().unwrap());
        assert_eq!(
            client_key(&headers, peer, true, ProxyIpHeader::XForwardedFor),
            "203.0.113.4"
        );
    }

    #[test]
    fn stale_buckets_are_reclaimed_but_oversized_keys_never_allocate() {
        let limiter = LoginRateLimiter::default();
        assert!(limiter.check_key(&"x".repeat(4096)).is_err());
        assert!(limiter.attempts.lock().unwrap().entries.is_empty());
        let before = Instant::now() - Duration::from_secs(61);
        {
            let mut buckets = limiter.attempts.lock().unwrap();
            buckets.entries = (0..MAX_IDENTITIES)
                .map(|n| (format!("stale-{n}"), VecDeque::from([before])))
                .collect();
            buckets.last_sweep = None;
        }
        limiter.check_key("fresh").unwrap();
        assert_eq!(limiter.attempts.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn only_trusts_forwarded_addresses_when_configured() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.4".parse().unwrap());
        assert_eq!(
            client_key(
                &headers,
                Some("127.0.0.1:4000".parse().unwrap()),
                false,
                ProxyIpHeader::default()
            ),
            "127.0.0.1"
        );
        assert_eq!(
            client_key(
                &headers,
                Some("127.0.0.1:4000".parse().unwrap()),
                true,
                ProxyIpHeader::default()
            ),
            "203.0.113.4"
        );
    }
}
