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
    pub token: String,
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
        .chunks_exact(2)
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

    pub async fn identity(&self, code: &str, verifier: &str) -> anyhow::Result<DiscordIdentity> {
        // Fixed endpoints only. Error messages deliberately omit reqwest errors,
        // whose URLs can contain short-lived credentials in other API methods.
        let response = self
            .http
            .post("https://discord.com/api/v10/oauth2/token")
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", self.config.application_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", self.config.redirect_uri.as_str()),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Discord authorization unavailable"))?;
        let token = bounded_json(response).await?;
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
        let user: DiscordIdentity = serde_json::from_value(bounded_json(response).await?)
            .map_err(|_| anyhow::anyhow!("invalid Discord identity"))?;
        anyhow::ensure!(
            snowflake(&user.id) && !user.username.trim().is_empty() && user.username.len() <= 256,
            "invalid Discord identity"
        );
        // Access/refresh tokens are discarded, never stored. Commands use signed
        // Discord identities, not ongoing access to the user's Discord account.
        Ok(user)
    }

    pub async fn reply(&self, interaction: &Interaction, response: &Value) -> anyhow::Result<()> {
        anyhow::ensure!(
            snowflake(&interaction.application_id)
                && !interaction.token.is_empty()
                && interaction.token.len() <= 4096
                && interaction
                    .token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')),
            "invalid interaction token"
        );
        let url = format!(
            "https://discord.com/api/v10/webhooks/{}/{}/messages/@original",
            self.config.application_id, interaction.token
        );
        let response = self
            .http
            .patch(url)
            .json(response)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Discord reply unavailable"))?;
        anyhow::ensure!(response.status().is_success(), "Discord rejected the reply");
        Ok(())
    }
}

async fn bounded_json(mut response: reqwest::Response) -> anyhow::Result<Value> {
    anyhow::ensure!(
        response.status().is_success(),
        "Discord rejected the request"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("Discord response interrupted"))?
    {
        anyhow::ensure!(
            body.len().saturating_add(chunk.len()) <= MAX_BODY,
            "Discord response too large"
        );
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("invalid Discord response"))
}

pub fn message(content: impl Into<String>) -> Value {
    json!({ "content": content.into(), "components": [], "allowed_mentions": { "parse": [] } })
}

/// User text remains literal and cannot introduce mentions, links, or Markdown.
pub fn plain_title(title: &str, limit: usize) -> String {
    title
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .map(|c| {
            if matches!(
                c,
                '*' | '_' | '~' | '`' | '[' | ']' | '<' | '>' | '@' | '\\' | '|'
            ) {
                ' '
            } else {
                c
            }
        })
        .collect()
}
