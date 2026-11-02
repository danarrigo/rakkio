use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Redirect, Response},
};
use oauth2::{CsrfToken, PkceCodeChallenge};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::{CurrentUser, oauth_browser_secret, oauth_cookie_name},
    discord::{self, Interaction, message},
    discord_store::DiscordConnection,
    error::{AppError, AppResult, ErrorBody, ErrorResponse},
    extract::{ApiJson, ApiQuery},
};

#[derive(Serialize, ToSchema)]
pub struct DiscordStatus {
    pub configured: bool,
    pub connection: Option<DiscordConnection>,
    pub install_url: Option<String>,
    pub consent_version: &'static str,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscordConnectRequest {
    pub timezone: String,
    pub consent_version: String,
}

#[derive(Serialize, ToSchema)]
pub struct DiscordAuthorization {
    pub authorization_url: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscordDisconnectRequest {
    pub expected_id: Uuid,
}

#[derive(Deserialize)]
pub struct DiscordCallback {
    pub state: String,
    pub code: Option<String>,
    pub error: Option<String>,
    pub iss: Option<String>,
}

#[utoipa::path(get,operation_id="discord_status",path="/api/v1/integrations/discord",responses((status=200,body=DiscordStatus),(status=401,body=ErrorResponse)),security(("session_cookie"=[]),("development_user"=[])),tag="discord")]
pub async fn status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<Json<DiscordStatus>> {
    Ok(Json(DiscordStatus {
        configured: state.discord.is_some(),
        connection: state.store.discord_connection(user).await?,
        install_url: state.discord.as_ref().map(|discord| discord.install_url()),
        consent_version: discord::CONSENT_VERSION,
    }))
}

#[utoipa::path(post,operation_id="discord_connect",path="/api/v1/integrations/discord/connect",request_body=DiscordConnectRequest,responses((status=200,body=DiscordAuthorization),(status=401,body=ErrorResponse),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse),(status=503,body=ErrorResponse)),security(("session_cookie"=[]),("development_user"=[])),tag="discord")]
pub async fn connect(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    ApiJson(request): ApiJson<DiscordConnectRequest>,
) -> AppResult<Response> {
    if request.consent_version != discord::CONSENT_VERSION {
        return Err(AppError::Validation(
            "Review and accept the current Discord data-sharing notice.".into(),
        ));
    }
    let discord = state
        .discord
        .as_ref()
        .ok_or(AppError::NotConfigured("Discord"))?;
    state
        .action_rate_limiter
        .check_key(&format!("discord-connect:{user}"))?;
    let csrf = CsrfToken::new_random_len(32);
    let fresh_browser = CsrfToken::new_random_len(32);
    let browser =
        oauth_browser_secret(&headers, state.secure_cookies).unwrap_or(fresh_browser.secret());
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    state
        .store
        .begin_discord_link(
            user,
            csrf.secret(),
            browser,
            verifier.secret(),
            &request.timezone,
        )
        .await?;
    let cookie = format!(
        "{}={browser}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        oauth_cookie_name(state.secure_cookies),
        if state.secure_cookies { "; Secure" } else { "" }
    );
    let mut response = Json(DiscordAuthorization {
        authorization_url: discord.authorization_url(csrf.secret(), challenge.as_str()),
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie
            .parse()
            .map_err(|_| AppError::Validation("Invalid OAuth cookie".into()))?,
    );
    Ok(response)
}

// The callback still requires the initiating signed-in account, not just state.
// Authentication and browser-bound state are validated before contacting Discord.
pub async fn callback(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<DiscordCallback>,
) -> AppResult<Response> {
    let discord = state
        .discord
        .as_ref()
        .ok_or(AppError::NotConfigured("Discord"))?;
    if query.state.len() != 43 {
        return Err(AppError::Forbidden("Invalid Discord state"));
    }
    let browser = oauth_browser_secret(&headers, state.secure_cookies).ok_or(
        AppError::Forbidden("Use the browser that started the Discord connection"),
    )?;
    let attempt = state
        .store
        .consume_discord_link(user, &query.state, browser)
        .await?;
    if query.error.is_some() {
        return Ok(Redirect::to(&format!(
            "{}/settings?discord=cancelled",
            state.app_url.trim_end_matches('/')
        ))
        .into_response());
    }
    if query.iss.as_deref() != Some("https://discord.com") {
        return Err(AppError::Forbidden("Invalid Discord authorization issuer"));
    }
    let code = query
        .code
        .filter(|code| !code.is_empty() && code.len() <= 4096)
        .ok_or_else(|| AppError::Validation("Missing Discord authorization code".into()))?;
    let identity = match discord.identity(&code, &attempt.pkce_verifier).await {
        Ok(identity) => identity,
        Err(error) => return Ok(provider_failure(error)),
    };
    state.store.finish_discord_link(attempt, identity).await?;
    Ok(Redirect::to(&format!(
        "{}/settings?discord=connected",
        state.app_url.trim_end_matches('/')
    ))
    .into_response())
}

fn provider_failure(error: anyhow::Error) -> Response {
    let Some(rate) = error.downcast_ref::<discord::DiscordRateLimit>() else {
        return AppError::Integration(error).into_response();
    };
    tracing::warn!(error = %rate, "Discord integration rate limited");
    let wait = rate
        .retry_after_seconds
        .map(|seconds| format!("Wait {seconds} seconds, then "))
        .unwrap_or_else(|| "Try again later: ".into());
    let message = if rate.stage == "reply" {
        "Discord is temporarily rate limiting command delivery. The command may already have succeeded. Check rakkio before submitting it again.".to_owned()
    } else {
        format!(
            "Discord is temporarily rate limiting account linking. {wait}start a fresh connection from rakkio Settings. Do not refresh this callback page."
        )
    };
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(ErrorResponse {
            error: ErrorBody {
                code: "integration_rate_limited",
                message,
            },
        }),
    )
        .into_response();
    response.headers_mut().insert(
        "x-rakkio-discord-provider-rate-limit",
        axum::http::HeaderValue::from_static("1"),
    );
    if let Some(seconds) = rate.retry_after_seconds {
        // A validated integer contains only header-safe ASCII digits.
        if let Ok(value) = seconds.to_string().parse() {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
    }
    response
}

#[utoipa::path(post,operation_id="discord_disconnect",path="/api/v1/integrations/discord/disconnect",request_body=DiscordDisconnectRequest,responses((status=204),(status=401,body=ErrorResponse),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse)),security(("session_cookie"=[]),("development_user"=[])),tag="discord")]
pub async fn disconnect(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(request): ApiJson<DiscordDisconnectRequest>,
) -> AppResult<StatusCode> {
    state
        .store
        .disconnect_discord(user, request.expected_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Receives original signed bytes from Discord directly or our fast relay.
/// The relay cannot impersonate a user: the backend verifies Discord's signature
/// again, including timestamp, application, and the signed actor identity.
pub async fn interactions(State(state): State<AppState>, request: Request) -> AppResult<Response> {
    let discord = state
        .discord
        .as_ref()
        .ok_or(AppError::NotConfigured("Discord"))?;
    let _admission = state.request_limits.discord_admission().await?;
    let (parts, body) = request.into_parts();
    let timestamp = parts
        .headers
        .get("x-signature-timestamp")
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let signature = parts
        .headers
        .get("x-signature-ed25519")
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        axum::body::to_bytes(body, discord::MAX_BODY),
    )
    .await
    .map_err(|_| AppError::InvalidRequest {
        status: StatusCode::REQUEST_TIMEOUT,
        message: "Discord body deadline exceeded".into(),
    })?
    .map_err(|_| AppError::InvalidRequest {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        message: "Discord request too large".into(),
    })?;
    discord
        .verify(timestamp, signature, &bytes)
        .map_err(|_| AppError::Unauthorized)?;
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Validation("Invalid Discord payload".into()))?;
    if body["type"] == 1 {
        return Ok(Json(json!({"type":1})).into_response());
    }
    let interaction: Interaction = serde_json::from_value(body)
        .map_err(|_| AppError::Validation("Invalid Discord interaction".into()))?;
    if interaction.application_id != discord.config.application_id
        || !discord::snowflake(&interaction.id)
        || !matches!(interaction.kind, 2 | 3)
    {
        return Err(AppError::Forbidden(
            "Invalid Discord application or interaction",
        ));
    }
    let actor = interaction
        .actor()
        .ok_or(AppError::Forbidden("Invalid Discord user"))?;
    let response = match state.store.discord_owner(actor).await? {
        None => message("Connect your Discord account in rakkio Settings before using commands."),
        Some(owner) => {
            let write = interaction.kind == 3
                || interaction
                    .data
                    .as_ref()
                    .is_some_and(|data| data["name"] == "add");
            let _permit = state
                .request_limits
                .discord_command(owner.user_id, write)
                .await?;
            state
                .action_rate_limiter
                .check_key(&format!("discord:{}", owner.user_id))?;
            let response = state.store.run_discord_command(owner, &interaction).await?;
            state.sync_dispatcher.wake();
            response
        }
    };
    if parts
        .headers
        .get("x-rakkio-discord-deferred")
        .is_some_and(|value| value == "1")
    {
        // Only a UI/transport choice; never an authentication credential. PATCH
        // updates the same original message, so retrying delivery is idempotent.
        if let Err(error) = discord.reply(&interaction, &response).await {
            return Ok(provider_failure(error));
        }
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        // Always-on self-hosters can use this endpoint directly, without Vercel.
        let mut response = response;
        response["flags"] = json!(64);
        Ok(Json(json!({"type":4,"data":response})).into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn identity_rate_limit_is_actionable_without_replaying_the_callback() {
        for delay in [Some(9), None] {
            let response = provider_failure(
                discord::DiscordRateLimit {
                    stage: "token exchange",
                    scope: "global",
                    retry_after_seconds: delay,
                }
                .into(),
            );
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(
                response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .map(|value| value.to_str().unwrap()),
                delay.map(|_| "9")
            );
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"]["code"], "integration_rate_limited");
            let message = body["error"]["message"].as_str().unwrap();
            assert!(message.contains("fresh connection"));
            assert!(message.contains("Do not refresh"));
            if delay.is_some() {
                assert!(message.contains("Wait 9 seconds"));
            } else {
                assert!(message.contains("Try again later"));
            }
        }
    }

    #[tokio::test]
    async fn delivery_rate_limit_preserves_cooldown_without_encouraging_duplicate_tasks() {
        let response = provider_failure(
            discord::DiscordRateLimit {
                stage: "reply",
                scope: "shared",
                retry_after_seconds: Some(5),
            }
            .into(),
        );
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers()["x-rakkio-discord-provider-rate-limit"],
            "1"
        );
        assert_eq!(response.headers()[header::RETRY_AFTER], "5");
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains("may already have succeeded"));
        assert!(!message.contains("fresh connection"));
    }

    #[tokio::test]
    async fn other_identity_failures_keep_generic_public_response() {
        let response = provider_failure(anyhow::anyhow!("test-provider-detail"));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(!response.headers().contains_key(header::RETRY_AFTER));
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["message"], "An external integration failed");
        assert!(!String::from_utf8_lossy(&bytes).contains("test-provider-detail"));
    }
}
