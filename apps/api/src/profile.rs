//! Explicit, permanent handle claims. No public account directory.
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::CurrentUser,
    error::{AppError, AppResult, ErrorResponse},
    extract::ApiJson,
    store::Store,
};

const MAX_RESERVATIONS: i64 = 10_000;

#[derive(Serialize, ToSchema)]
pub struct HandleProfile {
    pub handle: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimHandleRequest {
    /// Public, permanent handle: 3-30 ASCII letters, digits or underscores.
    /// Case-insensitive; optional leading @. Cannot be renamed or recycled.
    pub handle: String,
}

fn normalized_handle(value: &str) -> AppResult<String> {
    let value = value.trim();
    let value = value.strip_prefix('@').unwrap_or(value);
    if !(3..=30).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(AppError::Validation(
            "Choose 3-30 letters, numbers or underscores.".into(),
        ));
    }
    let handle = value.to_ascii_lowercase();
    if [
        "admin",
        "administrator",
        "api",
        "auth",
        "billing",
        "discord",
        "everyone",
        "help",
        "here",
        "moderator",
        "owner",
        "rakkio",
        "root",
        "security",
        "support",
        "system",
        "team",
        "www",
    ]
    .contains(&handle.as_str())
    {
        return Err(AppError::Validation(
            "That handle is reserved. Choose another.".into(),
        ));
    }
    Ok(handle)
}

impl Store {
    pub async fn own_handle(&self, user: Uuid) -> AppResult<HandleProfile> {
        // A missing claim is not a missing account, and never exposes anyone
        // else's profile. Keep legacy session/profile projections unchanged.
        let handle = sqlx::query_scalar::<_, Option<String>>(
            "SELECT h.handle FROM users u LEFT JOIN user_handles h ON h.user_id=u.id WHERE u.id=$1",
        )
        .bind(user)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AppError::Unauthorized)?;
        Ok(HandleProfile { handle })
    }

    pub async fn claim_handle(&self, user: Uuid, value: &str) -> AppResult<HandleProfile> {
        let handle = normalized_handle(value)?;
        let mut tx = self.pool.begin().await?;
        // Registry admission is rare. Serialize it to bound lifetime reservations
        // and make case-colliding claims deterministic, including capacity races.
        sqlx::query("SELECT pg_advisory_xact_lock(1886547816,1)")
            .execute(&mut *tx)
            .await?;
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(user)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::Unauthorized)?;
        let current =
            sqlx::query_scalar::<_, String>("SELECT handle FROM user_handles WHERE user_id=$1")
                .bind(user)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(current) = current {
            if current != handle {
                return Err(AppError::Conflict(
                    "Your handle has already been claimed and cannot be changed.".into(),
                ));
            }
            tx.commit().await?;
            return Ok(HandleProfile {
                handle: Some(current),
            });
        }
        let digest = Sha256::digest(handle.as_bytes());
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM user_handles WHERE handle_hash=$1)",
        )
        .bind(digest.as_slice())
        .fetch_one(&mut *tx)
        .await?
        {
            return Err(AppError::Conflict(
                "That handle is unavailable. Choose another.".into(),
            ));
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_handles")
            .fetch_one(&mut *tx)
            .await?;
        if count >= MAX_RESERVATIONS {
            return Err(AppError::Forbidden(
                "Handle registration is at capacity. Your personal workspace is still available.",
            ));
        }
        sqlx::query("INSERT INTO user_handles(handle_hash,user_id,handle) VALUES($1,$2,$3)")
            .bind(digest.as_slice())
            .bind(user)
            .bind(&handle)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(HandleProfile {
            handle: Some(handle),
        })
    }
}

#[utoipa::path(get,path="/api/v1/profile/handle",responses((status=200,body=HandleProfile),(status=401,body=ErrorResponse)),security(("session_cookie"=[]),("development_user"=[])),tag="account")]
pub async fn get_handle(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<Json<HandleProfile>> {
    Ok(Json(state.store.own_handle(user).await?))
}

#[utoipa::path(post,path="/api/v1/profile/handle",request_body=ClaimHandleRequest,responses((status=200,body=HandleProfile),(status=401,body=ErrorResponse),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse),(status=422,body=ErrorResponse),(status=429,body=ErrorResponse)),security(("session_cookie"=[]),("development_user"=[])),tag="account")]
pub async fn claim_handle(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(request): ApiJson<ClaimHandleRequest>,
) -> AppResult<Json<HandleProfile>> {
    state
        .action_rate_limiter
        .check_key(&format!("handle-claim:{user}"))?;
    Ok(Json(state.store.claim_handle(user, &request.handle).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_without_unicode_confusables_or_reserved_impersonation() {
        assert_eq!(normalized_handle(" @DaNiel_18 ").unwrap(), "daniel_18");
        for value in [
            "",
            "ab",
            "@@alice",
            "alice.smith",
            "alice smith",
            "Ａlice",
            "@SUPPORT",
            "rakkio",
            "everyone",
            "alice\nsmith",
        ] {
            assert!(normalized_handle(value).is_err(), "{value}");
        }
        assert!(normalized_handle(&"x".repeat(30)).is_ok());
        assert!(normalized_handle(&"x".repeat(31)).is_err());
    }
}
