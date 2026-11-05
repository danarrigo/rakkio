use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
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

#[derive(Clone)]
pub struct LoginRateLimiter {
    attempts: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
    limit: usize,
    window: Duration,
    message: &'static str,
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self {
            attempts: Arc::new(Mutex::new(HashMap::new())),
            limit: 10,
            window: Duration::from_secs(60),
            message: "too many authentication attempts; try again shortly",
        }
    }
}

impl LoginRateLimiter {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            attempts: Arc::new(Mutex::new(HashMap::new())),
            limit,
            window: Duration::from_secs(60),
            message: "too many requests; try again shortly",
        }
    }

    pub fn check(
        &self,
        headers: &HeaderMap,
        peer: Option<SocketAddr>,
        trust_proxy_headers: bool,
    ) -> AppResult<()> {
        let key = client_key(headers, peer, trust_proxy_headers);
        self.check_key(&key)
    }

    pub fn check_key(&self, key: &str) -> AppResult<()> {
        let now = Instant::now();
        let cutoff = now - self.window;
        let mut attempts = self.attempts.lock().map_err(|_| AppError::InvalidRequest {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            message: "rate limiter is unavailable".to_owned(),
        })?;
        let entries = attempts.entry(key.to_owned()).or_default();
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
        if attempts.len() > 10_000 {
            attempts.retain(|_, entries| entries.back().is_some_and(|instant| *instant > cutoff));
        }
        Ok(())
    }
}

fn client_key(headers: &HeaderMap, peer: Option<SocketAddr>, trust_proxy_headers: bool) -> String {
    if trust_proxy_headers {
        if let Some(value) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return value.to_owned();
        }
        if let Some(value) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
        {
            return value.to_owned();
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
        assert_eq!(limiter.attempts.lock().unwrap().len(), 10000);
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
    fn only_trusts_forwarded_addresses_when_configured() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.4".parse().unwrap());
        assert_eq!(
            client_key(&headers, Some("127.0.0.1:4000".parse().unwrap()), false),
            "127.0.0.1"
        );
        assert_eq!(
            client_key(&headers, Some("127.0.0.1:4000".parse().unwrap()), true),
            "203.0.113.4"
        );
    }
}
