//! Discord protocol/client boundary. No credentials or message bodies are logged.
use std::time::Duration;

use anyhow::{Context, bail};
use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};
use chrono::Utc;
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};

pub const CONSENT_VERSION: &str = "2026-10-31";
pub const MAX_BODY: usize = 64 * 1024;

#[derive(Clone)]
pub struct DiscordConfig {
    pub application_id: String,
    pub client_secret: String,
    pub public_key: String,
    pub redirect_uri: String,
}

impl std::fmt::Debug for DiscordConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiscordConfig")
            .field("application_id", &self.application_id)
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct Discord {
    pub config: DiscordConfig,
    key: Vec<u8>,
    http: reqwest::Client,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "Discord {stage} rate limited the request (HTTP 429, scope={scope}, retry_after_seconds={retry_after_seconds:?})"
)]
pub(crate) struct DiscordRateLimit {
    pub(crate) stage: &'static str,
    pub(crate) scope: &'static str,
    pub(crate) retry_after_seconds: Option<u64>,
}

#[derive(Deserialize)]
pub struct DiscordIdentity {
    pub id: String,
    pub username: String,
}

#[derive(Deserialize)]
pub struct Interaction {
    pub id: String,
    pub application_id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    pub user: Option<Actor>,
    pub member: Option<Member>,
    pub data: Option<Value>,
}

#[derive(Deserialize)]
pub struct Actor {
    pub id: String,
    #[serde(default)]
    pub bot: bool,
}

#[derive(Deserialize)]
pub struct Member {
    pub user: Actor,
}

impl Interaction {
    pub fn created_at_millis(&self) -> Option<i64> {
        // Discord snowflakes bind an event to its creation time. This prevents
        // a delayed signed command from moving to a newly linked account.
        self.id
            .parse::<u64>()
            .ok()
            .map(|id| ((id >> 22) as i64) + 1_420_070_400_000)
    }

    pub fn actor(&self) -> Option<&str> {
        let user = self
            .member
            .as_ref()
            .map(|member| &member.user)
            .or(self.user.as_ref())?;
        (!user.bot && snowflake(&user.id)).then_some(user.id.as_str())
    }
}

pub fn snowflake(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|number| number > 0)
}

pub fn decode_hex(value: &str) -> anyhow::Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid hexadecimal value");
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair)?, 16).map_err(Into::into))
        .collect()
}

impl Discord {
    pub fn new(config: DiscordConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(
            snowflake(&config.application_id),
            "DISCORD_APPLICATION_ID must be a Discord application ID"
        );
        let key =
            decode_hex(&config.public_key).context("DISCORD_PUBLIC_KEY must be hexadecimal")?;
        anyhow::ensure!(key.len() == 32, "DISCORD_PUBLIC_KEY must contain 32 bytes");
        anyhow::ensure!(
            !config.client_secret.trim().is_empty(),
            "DISCORD_CLIENT_SECRET is required"
        );
        let redirect = Url::parse(&config.redirect_uri).context("invalid DISCORD_REDIRECT_URI")?;
        anyhow::ensure!(
            matches!(redirect.scheme(), "http" | "https")
                && redirect.host_str().is_some()
                && redirect.username().is_empty()
                && redirect.password().is_none()
                && redirect.query().is_none()
                && redirect.fragment().is_none()
                && redirect.path() == "/api/v1/auth/discord/callback",
            "invalid DISCORD_REDIRECT_URI"
        );
        Ok(Self {
            config,
            key,
            http: reqwest::Client::builder()
                .user_agent("DiscordBot (https://github.com/danarrigo/rakkio, 0.1.0)")
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(12))
                .build()?,
        })
    }

    pub fn verify(&self, timestamp: &str, signature: &str, body: &[u8]) -> anyhow::Result<()> {
        let time: i64 = timestamp.parse().context("invalid Discord timestamp")?;
        anyhow::ensure!(
            Utc::now().timestamp().abs_diff(time) <= 300,
            "stale Discord request"
        );
        let signature = decode_hex(signature)?;
        let mut signed = Vec::with_capacity(timestamp.len() + body.len());
        signed.extend_from_slice(timestamp.as_bytes());
        signed.extend_from_slice(body);
        UnparsedPublicKey::new(&ED25519, &self.key)
            .verify(&signed, &signature)
            .map_err(|_| anyhow::anyhow!("invalid Discord signature"))
    }

    pub fn authorization_url(&self, state: &str, challenge: &str) -> String {
        let mut url =
            Url::parse("https://discord.com/oauth2/authorize").expect("fixed Discord URL");
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", self.config.application_id.as_str()),
            ("redirect_uri", self.config.redirect_uri.as_str()),
            ("scope", "identify"),
            ("state", state),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("prompt", "consent"),
        ]);
        url.into()
    }

    pub fn install_url(&self) -> String {
        let mut url =
            Url::parse("https://discord.com/oauth2/authorize").expect("fixed Discord URL");
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.config.application_id.as_str()),
            ("scope", "applications.commands"),
            ("integration_type", "1"),
        ]);
        url.into()
    }

    fn token_request(&self, code: &str, verifier: &str) -> reqwest::RequestBuilder {
        self.http
            .post("https://discord.com/api/v10/oauth2/token")
            .basic_auth(
                &self.config.application_id,
                Some(&self.config.client_secret),
            )
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.config.redirect_uri.as_str()),
                ("code_verifier", verifier),
            ])
    }

    pub async fn identity(&self, code: &str, verifier: &str) -> anyhow::Result<DiscordIdentity> {
        // Fixed endpoints only. Error messages deliberately omit reqwest errors,
        // whose URLs can contain short-lived credentials in other API methods.
        let response = self
            .token_request(code, verifier)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Discord authorization unavailable"))?;
        let token = bounded_json(response, "token exchange").await?;
        anyhow::ensure!(
            token["token_type"]
                .as_str()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("bearer"))
                && token["scope"].as_str().is_some_and(|scopes| scopes
                    .split_whitespace()
                    .any(|scope| scope == "identify")),
            "Discord did not grant identity access"
        );
        let access = token["access_token"]
            .as_str()
            .filter(|token| !token.is_empty() && token.len() <= 4096)
            .context("Discord did not provide an access token")?;
        let response = self
            .http
            .get("https://discord.com/api/v10/users/@me")
            .bearer_auth(access)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Discord identity unavailable"))?;
        let identity = bounded_json(response, "identity lookup").await?;
        let user: DiscordIdentity = serde_json::from_value(identity)
            .map_err(|_| anyhow::anyhow!("invalid Discord identity"))?;
        anyhow::ensure!(
            snowflake(&user.id) && !user.username.trim().is_empty() && user.username.len() <= 256,
            "invalid Discord identity"
        );
        // Access/refresh tokens are discarded, never stored. Commands use signed
        // Discord identities, not ongoing access to the user's Discord account.
        Ok(user)
    }
}

fn cooldown_seconds(value: f64) -> Option<u64> {
    // This is a validation bound, not a provider rate limit. Longer/invalid
    // hints stay unknown: never shorten a cooldown and never automatically retry.
    (value.is_finite() && (0.0..=86_400.0).contains(&value)).then_some(value.ceil() as u64)
}

async fn bounded_json(
    mut response: reqwest::Response,
    stage: &'static str,
) -> anyhow::Result<Value> {
    let status = response.status();
    let header_delay = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.parse::<f64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(value)
                    .ok()
                    .map(|date| {
                        (date.with_timezone(&Utc) - Utc::now())
                            .num_milliseconds()
                            .max(0) as f64
                            / 1000.0
                    })
            })
        })
        .filter(|value| value.is_finite() && *value >= 0.0);
    let scope = match response
        .headers()
        .get("x-ratelimit-scope")
        .and_then(|value| value.to_str().ok())
    {
        Some("global") => "global",
        Some("shared") => "shared",
        Some("user") => "user",
        _ => "unavailable",
    };
    let header_global = response
        .headers()
        .get("x-ratelimit-global")
        .is_some_and(|value| value == "true");
    let limited = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
    let mut body = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) if limited => {
                body.clear();
                break;
            }
            Err(_) => bail!("Discord response interrupted"),
        };
        if body.len().saturating_add(chunk.len()) > MAX_BODY {
            if limited {
                body.clear();
                break;
            }
            bail!("Discord response too large");
        }
        body.extend_from_slice(&chunk);
    }
    let parsed = serde_json::from_slice::<Value>(&body);
    if limited {
        let value = parsed.as_ref().ok();
        let json_delay = value
            .and_then(|value| value["retry_after"].as_f64())
            .filter(|value| value.is_finite() && *value >= 0.0);
        let delay = match (header_delay, json_delay) {
            (Some(header), Some(json)) => Some(header.max(json)),
            (header, json) => header.or(json),
        }
        .and_then(cooldown_seconds);
        let global = header_global
            || value
                .and_then(|value| value["global"].as_bool())
                .unwrap_or(false);
        return Err(DiscordRateLimit {
            stage,
            scope: if scope == "unavailable" && global {
                "global"
            } else {
                scope
            },
            retry_after_seconds: delay,
        }
        .into());
    }
    if !status.is_success() {
        // Only standard OAuth categories are safe to report. Never include
        // arbitrary provider text, descriptions, URLs, codes, or token fields.
        let category = parsed
            .as_ref()
            .ok()
            .and_then(|value| value["error"].as_str())
            .filter(|category| {
                matches!(
                    *category,
                    "invalid_client"
                        | "invalid_grant"
                        | "invalid_request"
                        | "invalid_scope"
                        | "unauthorized_client"
                        | "unsupported_grant_type"
                        | "access_denied"
                        | "server_error"
                        | "temporarily_unavailable"
                )
            })
            .unwrap_or("unavailable");
        bail!(
            "Discord {stage} rejected the request (HTTP {}, OAuth category: {category})",
            status.as_u16()
        );
    }
    parsed.map_err(|_| anyhow::anyhow!("invalid Discord response"))
}

pub fn message(content: impl Into<String>) -> Value {
    json!({ "content": content.into(), "components": [], "allowed_mentions": { "parse": [] } })
}

/// Plain labels/inline-code text, bounded in UTF-16 units like Discord limits.
/// Callers wrap message titles in inline code; mentions are separately disabled.
pub fn plain_title(title: &str, limit: usize) -> String {
    let mut result = String::new();
    let mut units = 0;
    for character in title.chars() {
        let character = if character.is_control()
            || character == '`'
            || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            ' '
        } else {
            character
        };
        units += character.len_utf16();
        if units > limit {
            break;
        }
        result.push(character);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_fit_discord_utf16_limits_without_inline_code_or_direction_breakout() {
        let title = format!("{}\n`\u{202e}", "😀".repeat(200));
        let rendered = plain_title(&title, 60);
        assert_eq!(rendered.encode_utf16().count(), 60);
        assert_eq!(rendered.chars().count(), 30);
        assert_eq!(plain_title("a\n`\u{202e}b", 20), "a   b");
    }

    #[tokio::test]
    async fn provider_rejections_report_safe_status_and_oauth_category() {
        let response: reqwest::Response = http::Response::builder().status(400)
            .body(r#"{"error":"invalid_grant","error_description":"private-provider-detail","access_token":"private-token-value"}"#.to_owned()).unwrap().into();
        let error = bounded_json(response, "test")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 400"), "{error}");
        assert!(error.contains("invalid_grant"), "{error}");
        assert!(!error.contains("private-provider-detail"));
        assert!(!error.contains("private-token-value"));
    }

    #[tokio::test]
    async fn unknown_error_payloads_are_never_logged() {
        let response: reqwest::Response = http::Response::builder()
            .status(403)
            .body(
                r#"{"error":"private-unrecognized-value","message":"private-message"}"#.to_owned(),
            )
            .unwrap()
            .into();
        let error = bounded_json(response, "test")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 403"), "{error}");
        assert!(!error.contains("private-unrecognized-value"));
        assert!(!error.contains("private-message"));
    }

    #[tokio::test]
    async fn html_and_oversized_provider_errors_remain_bounded_and_redacted() {
        let html: reqwest::Response = http::Response::builder()
            .status(403)
            .body("<html>private-block-page</html>".to_owned())
            .unwrap()
            .into();
        let error = bounded_json(html, "test").await.unwrap_err().to_string();
        assert!(error.contains("HTTP 403"));
        assert!(!error.contains("private-block-page"));
        let large: reqwest::Response = http::Response::builder()
            .status(400)
            .body("x".repeat(MAX_BODY + 1))
            .unwrap()
            .into();
        assert_eq!(
            bounded_json(large, "test").await.unwrap_err().to_string(),
            "Discord response too large"
        );
    }

    #[test]
    fn token_exchange_authenticates_in_header_and_preserves_pkce() {
        use base64::Engine;
        let discord = Discord::new(DiscordConfig {
            application_id: "123".into(),
            client_secret: "test-secret".into(),
            public_key: "00".repeat(32),
            redirect_uri: "https://example.test/api/v1/auth/discord/callback".into(),
        })
        .unwrap();
        let request = discord
            .token_request("test-code", "test-verifier")
            .build()
            .unwrap();
        let auth = request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .expect("token exchange must authenticate in the Authorization header")
            .to_str()
            .unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(auth.strip_prefix("Basic ").unwrap())
                .unwrap(),
            b"123:test-secret"
        );
        let body = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
        let decoded = Url::parse(&format!("https://example.test/?{body}")).unwrap();
        let form: std::collections::HashMap<String, String> =
            decoded.query_pairs().into_owned().collect();
        assert_eq!(form.get("code_verifier").unwrap(), "test-verifier");
        assert_eq!(form.get("code").unwrap(), "test-code");
        assert_eq!(form.get("grant_type").unwrap(), "authorization_code");
        assert_eq!(
            form.get("redirect_uri").unwrap(),
            &discord.config.redirect_uri
        );
        assert!(!form.contains_key("client_secret"));
        assert!(!form.contains_key("client_id"));
    }

    #[tokio::test]
    async fn rate_limit_metadata_is_safe_and_does_not_shorten_cooldown() {
        let response: reqwest::Response = http::Response::builder()
            .status(429)
            .header("retry-after", "2.2")
            .body(r#"{"retry_after":8.1,"global":true,"message":"private-detail"}"#.to_owned())
            .unwrap()
            .into();
        let error = bounded_json(response, "token exchange")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("retry_after_seconds=Some(9)"), "{error}");
        assert!(error.contains("scope=global"), "{error}");
        assert!(!error.contains("private-detail"));
    }

    #[tokio::test]
    async fn rate_hints_ignore_untrusted_values_without_shortening_long_waits() {
        for (header, body) in [
            (
                "private-header",
                r#"{"retry_after":-1,"global":false,"message":"private-message"}"#,
            ),
            ("90000", r#"{"retry_after":1}"#),
        ] {
            let response: reqwest::Response = http::Response::builder()
                .status(429)
                .header("retry-after", header)
                .header("x-ratelimit-scope", "private-scope")
                .body(body.to_owned())
                .unwrap()
                .into();
            let error = bounded_json(response, "token exchange").await.unwrap_err();
            let rate = error.downcast_ref::<DiscordRateLimit>().unwrap();
            assert_eq!(rate.retry_after_seconds, None);
            assert_eq!(rate.scope, "unavailable");
            let logged = format!("{error:?}");
            assert!(!logged.contains("private-"));
            assert!(!logged.contains("90000"));
        }
    }

    #[tokio::test]
    async fn rate_limit_survives_html_or_oversized_bodies_and_http_date_hints() {
        for body in [
            "<html>private-block-page</html>".to_owned(),
            "x".repeat(MAX_BODY + 1),
        ] {
            let response: reqwest::Response = http::Response::builder()
                .status(429)
                .header("retry-after", "5")
                .header("x-ratelimit-scope", "shared")
                .body(body)
                .unwrap()
                .into();
            let error = bounded_json(response, "token exchange").await.unwrap_err();
            let rate = error.downcast_ref::<DiscordRateLimit>().unwrap();
            assert_eq!(rate.retry_after_seconds, Some(5));
            assert_eq!(rate.scope, "shared");
            assert!(!error.to_string().contains("private-block-page"));
        }
        let date = (Utc::now() + chrono::Duration::hours(1))
            .format("%a, %d %b %Y %T GMT")
            .to_string();
        let response: reqwest::Response = http::Response::builder()
            .status(429)
            .header("retry-after", date)
            .body(String::new())
            .unwrap()
            .into();
        let error = bounded_json(response, "identity lookup").await.unwrap_err();
        let rate = error.downcast_ref::<DiscordRateLimit>().unwrap();
        assert!(
            rate.retry_after_seconds
                .is_some_and(|seconds| seconds > 0 && seconds <= 3600)
        );
    }

    #[test]
    fn hex_keys_fail_closed_on_bad_encodings() {
        assert!(decode_hex("a").is_err());
        assert!(decode_hex("gg").is_err());
        assert_eq!(decode_hex("aB01").unwrap(), vec![171, 1]);
    }
}
