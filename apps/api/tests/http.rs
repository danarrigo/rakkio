use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use prosepect_api::{
    app::{self, ApiDoc},
    auth::DEVELOPMENT_USER_HEADER,
    config::{Config, Environment, GoogleOAuthConfig, ObjectStorageConfig},
    store::{DEVELOPMENT_USER_ID, Store},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use utoipa::OpenApi;

#[path = "http/discord.rs"]
mod discord;
#[path = "http/groups.rs"]
mod groups;
#[path = "http/handles.rs"]
mod handles;
#[path = "http/rebrand.rs"]
mod rebrand;
#[path = "http/security_regressions.rs"]
mod security_regressions;
#[path = "http/upload_stream_regression.rs"]
mod upload_stream_regression;

#[sqlx::test(migrations = "../../migrations")]
async fn google_tasks_consent_start_persists_a_user_bound_single_use_attempt(
    pool: PgPool,
) -> anyhow::Result<()> {
    let mut config = test_config();
    config.google_oauth = Some(GoogleOAuthConfig {
        client_id: "test-client".to_owned(),
        client_secret: "test-secret".to_owned(),
        redirect_uri: "http://localhost/callback".to_owned(),
        token_encryption_key: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_owned(),
    });
    let store = Store::from_pool(pool.clone());
    let router = app::build(&config, store.clone())?;
    let anonymous = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/google/tasks/start")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let session = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(session.status(), StatusCode::OK);
    let cookie = session.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let session_body: serde_json::Value =
        serde_json::from_slice(&to_bytes(session.into_body(), 65536).await?)?;
    let user_id: uuid::Uuid = session_body["user"]["id"].as_str().unwrap().parse()?;
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/google/tasks/start")
                .header(header::COOKIE, cookie)
                .body(Body::empty())?,
        )
        .await?;
    let status = response.status();
    let location = response.headers().get(header::LOCATION).cloned();
    let oauth_cookie = response.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let browser_secret = oauth_cookie.split_once('=').unwrap().1;
    let body = to_bytes(response.into_body(), 65536).await?;
    assert_eq!(
        status,
        StatusCode::TEMPORARY_REDIRECT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    // Inspect the redirect without following it: no Google requests or grants.
    let location = reqwest::Url::parse(location.unwrap().to_str()?)?;
    assert_eq!(
        location.origin().ascii_serialization(),
        "https://accounts.google.com"
    );
    let params: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
    assert!(
        params["scope"]
            .split_whitespace()
            .any(|scope| scope == prosepect_api::google_tasks::TASKS_SCOPE)
    );
    assert!(!params["scope"].contains("calendar"));
    assert_eq!(params["include_granted_scopes"], "true");
    assert_eq!(params["prompt"], "consent");
    assert_eq!(params["code_challenge_method"], "S256");
    let attempt = store
        .consume_google_login_attempt(&params["state"], browser_secret)
        .await?;
    assert_eq!(attempt.user_id, Some(user_id));
    assert_eq!(attempt.purpose, "tasks_connect");
    assert_eq!(attempt.nonce, params["nonce"]);
    let challenge = oauth2::PkceCodeChallenge::from_code_verifier_sha256(
        &oauth2::PkceCodeVerifier::new(attempt.pkce_verifier),
    );
    assert_eq!(challenge.as_str(), params["code_challenge"]);
    assert!(attempt.terms_version.is_none());
    assert!(attempt.privacy_version.is_none());
    assert!(!attempt.age_confirmed);
    assert!(
        store
            .consume_google_login_attempt(&params["state"], browser_secret)
            .await
            .is_err()
    );
    let connections: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM google_task_connections")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        connections, 0,
        "Starting consent must not enable task synchronization"
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn saved_view_routes_require_authentication_and_validate_filters(
    pool: PgPool,
) -> anyhow::Result<()> {
    let router = app::build(&test_config(), Store::from_pool(pool))?;
    let anonymous = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/saved-task-views")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let session: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?;
    let csrf = session["csrf_token"].as_str().unwrap();
    let valid = r#"{"name":"Reports","search":"report","status":"open","sort":"due"}"#;
    let rejected = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/saved-task-views")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(valid))?,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    for body in [
        r#"{"name":"Bad","search":"","status":"unknown","sort":"due"}"#,
        r#"{"name":"Bad","search":"","status":"open","sort":"due","user_id":"foreign"}"#,
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/saved-task-views")
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", csrf)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/saved-task-views")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(valid))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn development_session_uses_an_http_only_cookie_and_csrf_token(
    pool: PgPool,
) -> anyhow::Result<()> {
    let config = test_config();
    let router = app::build(&config, Store::from_pool(pool))?;

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("session cookie")
        .to_str()?
        .split(';')
        .next()
        .expect("cookie value")
        .to_owned();
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()?
            .contains("HttpOnly")
    );
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()?
            .contains("SameSite=Lax")
    );
    let body = to_bytes(response.into_body(), 64 * 1024).await?;
    let session: serde_json::Value = serde_json::from_slice(&body)?;
    let csrf_token = session["csrf_token"].as_str().expect("CSRF token");

    let current = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/session")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(current.status(), StatusCode::OK);

    let rejected = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"No CSRF"}"#))?,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);

    let accepted = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", csrf_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"With CSRF"}"#))?,
        )
        .await?;
    assert_eq!(accepted.status(), StatusCode::CREATED);

    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn google_login_requires_current_legal_acceptance(pool: PgPool) -> anyhow::Result<()> {
    let router = app::build(&test_config(), Store::from_pool(pool))?;

    let missing = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/google/start")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

    let outdated = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/google/start?terms_version=2026-09-04&privacy_version=2026-09-04&age_confirmed=true")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(outdated.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let accepted = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/google/start?terms_version=2026-10-04&privacy_version=2026-11-04-groups&age_confirmed=true")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(accepted.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn synchronization_trigger_requires_its_service_token(pool: PgPool) -> anyhow::Result<()> {
    let token = "worker-trigger-token-for-tests-123";
    let mut config = test_config();
    config.worker_trigger_token = Some(token.to_owned());
    config.google_oauth = Some(GoogleOAuthConfig {
        client_id: "test-client".to_owned(),
        client_secret: "test-secret".to_owned(),
        redirect_uri: "http://localhost/callback".to_owned(),
        token_encryption_key: "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=".to_owned(),
    });
    let router = app::build(&config, Store::from_pool(pool))?;

    let rejected = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/synchronization/run")
                .header(header::AUTHORIZATION, "Bearer wrong-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

    let accepted = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/internal/synchronization/run")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(accepted.status(), StatusCode::OK);
    let body = to_bytes(accepted.into_body(), 64 * 1024).await?;
    let body: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(body["enqueued"], 0);
    assert_eq!(body["processed"], false);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn google_calendar_webhooks_are_authenticated_and_idempotent(
    pool: PgPool,
) -> anyhow::Result<()> {
    let config = test_config();
    let router = app::build(&config, Store::from_pool(pool.clone()))?;
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/calendars")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    let calendar_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM calendars WHERE user_id = $1 AND is_default")
            .bind(DEVELOPMENT_USER_ID)
            .fetch_one(&pool)
            .await?;
    let channel_id = uuid::Uuid::new_v4();
    let token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    sqlx::query(
        r#"
        INSERT INTO google_watch_channels (
            channel_id, user_id, calendar_id, resource_id, token_hash, expires_at
        ) VALUES ($1, $2, $3, 'resource-1', $4, $5)
        "#,
    )
    .bind(channel_id)
    .bind(DEVELOPMENT_USER_ID)
    .bind(calendar_id)
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .bind(Utc::now() + Duration::hours(1))
    .execute(&pool)
    .await?;

    for _ in 0..2 {
        let accepted = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhooks/google/calendar")
                    .header("x-goog-channel-id", channel_id.to_string())
                    .header("x-goog-resource-id", "resource-1")
                    .header("x-goog-channel-token", token)
                    .header("x-goog-message-number", "42")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(accepted.status(), StatusCode::NO_CONTENT);
    }
    let rejected = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhooks/google/calendar")
                .header("x-goog-channel-id", channel_id.to_string())
                .header("x-goog-resource-id", "resource-1")
                .header("x-goog-channel-token", "wrong-token")
                .header("x-goog-message-number", "43")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sync_jobs WHERE idempotency_key = $1")
        .bind(format!("google-watch:{channel_id}:42"))
        .fetch_one(&pool)
        .await?;
    assert_eq!(jobs, 1);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn file_upload_download_and_deletion_are_tenant_scoped(pool: PgPool) -> anyhow::Result<()> {
    let mut config = test_config();
    config.max_file_size_bytes = 100;
    config.max_user_file_storage_bytes = 200;
    let router = app::build(&config, Store::from_pool(pool))?;
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;

    let boundary = "prosepect-test-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"brief.txt\"\r\nContent-Type: text/plain\r\n\r\nprivate attachment\r\n--{boundary}--\r\n"
    );
    let uploaded = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/files")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))?,
        )
        .await?;
    let upload_status = uploaded.status();
    let body = to_bytes(uploaded.into_body(), 64 * 1024).await?;
    assert_eq!(
        upload_status,
        StatusCode::CREATED,
        "upload response: {}",
        String::from_utf8_lossy(&body)
    );
    let file: serde_json::Value = serde_json::from_slice(&body)?;
    let file_id = file["id"].as_str().expect("file id");
    assert_eq!(file["filename"], "brief.txt");

    let denied = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/files/{file_id}/download"))
                .header(DEVELOPMENT_USER_HEADER, uuid::Uuid::new_v4().to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);

    let downloaded = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/files/{file_id}/download"))
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(downloaded.into_body(), 64 * 1024).await?,
        "private attachment"
    );

    let unauthenticated = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/files/usage")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    for (user_id, expected_bytes) in [(DEVELOPMENT_USER_ID, 18), (uuid::Uuid::new_v4(), 0)] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/files/usage?user_id={DEVELOPMENT_USER_ID}"))
                    .header(DEVELOPMENT_USER_HEADER, user_id.to_string())
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let usage: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024).await?)?;
        assert_eq!(
            usage,
            serde_json::json!({
                "used_bytes": expected_bytes, "max_user_storage_bytes": 200, "max_file_size_bytes": 100
            })
        );
    }

    let deleted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/files/{file_id}"))
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/files/usage")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    let usage: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024).await?)?;
    assert_eq!(usage["used_bytes"], 0);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn file_uploads_respect_the_global_storage_quota(pool: PgPool) -> anyhow::Result<()> {
    let mut config = test_config();
    config.max_total_file_storage_bytes = 8;
    let router = app::build(&config, Store::from_pool(pool.clone()))?;
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;

    for (contents, expected) in [
        ("12345678", StatusCode::CREATED),
        ("x", StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let boundary = format!("prosepect-quota-{}", contents.len());
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"quota.txt\"\r\nContent-Type: text/plain\r\n\r\n{contents}\r\n--{boundary}--\r\n"
        );
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/files")
                    .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))?,
            )
            .await?;
        assert_eq!(response.status(), expected);
    }

    let stored_bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(byte_size), 0)::BIGINT FROM files")
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored_bytes, 8);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn file_uploads_respect_the_per_user_storage_quota(pool: PgPool) -> anyhow::Result<()> {
    let mut config = test_config();
    config.max_user_file_storage_bytes = 8;
    config.max_total_file_storage_bytes = 16;
    let router = app::build(&config, Store::from_pool(pool.clone()))?;
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;

    for (contents, expected) in [
        ("12345678", StatusCode::CREATED),
        ("x", StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let boundary = format!("prosepect-user-quota-{}", contents.len());
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"quota.txt\"\r\nContent-Type: text/plain\r\n\r\n{contents}\r\n--{boundary}--\r\n"
        );
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/files")
                    .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))?,
            )
            .await?;
        let status = response.status();
        let body = to_bytes(response.into_body(), 64 * 1024).await?;
        assert_eq!(status, expected);
        if expected == StatusCode::PAYLOAD_TOO_LARGE {
            assert!(String::from_utf8_lossy(&body).contains("personal attachment storage quota"));
        }
    }

    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn settings_exports_and_account_deletion_work_end_to_end(pool: PgPool) -> anyhow::Result<()> {
    let config = test_config();
    let router = app::build(&config, Store::from_pool(pool.clone()))?;
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;

    let settings = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"theme":"dark","automatic_daily_review":false,"sync_conflict_policy":"latest","sidebar_visible":false,"expected_version":1}"#,
                ))?,
        )
        .await?;
    assert_eq!(settings.status(), StatusCode::OK);

    let export = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/exports/json")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(export.status(), StatusCode::OK);
    assert_eq!(
        export.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"rakkio-export.json\""
    );
    let export_body = to_bytes(export.into_body(), 1024 * 1024).await?;
    let export_json: serde_json::Value = serde_json::from_slice(&export_body)?;
    assert_eq!(export_json["user"]["id"], DEVELOPMENT_USER_ID.to_string());

    let deleted = router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/account")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"confirmation":"DELETE"}"#))?,
        )
        .await?;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let user_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(DEVELOPMENT_USER_ID)
        .fetch_one(&pool)
        .await?;
    assert!(!user_exists);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn todoist_import_is_created_atomically_through_the_api(pool: PgPool) -> anyhow::Result<()> {
    let router = app::build(&test_config(), Store::from_pool(pool.clone()))?;
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/imports/todoist")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "project_name":"Imported work",
                        "project_description":"Imported from Todoist",
                        "tasks":[{
                            "title":"Imported task",
                            "description":"Context",
                            "due_at":"2026-09-15T16:00:00Z",
                            "scheduled_start":null,
                            "scheduled_end":null,
                            "priority":"urgent",
                            "recurrence":"none",
                            "labels":["review"],
                            "parent_index":null
                        }]
                    }"#,
                ))?,
        )
        .await?;

    let status = response.status();
    let body = to_bytes(response.into_body(), 64 * 1024).await?;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "import response: {}",
        String::from_utf8_lossy(&body)
    );
    let result: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(result["project"]["name"], "Imported work");
    assert_eq!(result["imported_tasks"], 1);
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE user_id = $1")
        .bind(DEVELOPMENT_USER_ID)
        .fetch_one(&pool)
        .await?;
    assert_eq!(task_count, 1);
    Ok(())
}

#[test]
fn openapi_operation_ids_are_unique() -> anyhow::Result<()> {
    let document = serde_json::to_value(ApiDoc::openapi())?;
    let mut seen = std::collections::HashSet::new();
    for methods in document["paths"].as_object().unwrap().values() {
        for operation in methods.as_object().unwrap().values() {
            if let Some(id) = operation["operationId"].as_str() {
                assert!(seen.insert(id), "duplicate OpenAPI operationId: {id}");
            }
        }
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn extractor_failures_use_the_error_envelope(pool: PgPool) -> anyhow::Result<()> {
    let openapi = serde_json::to_value(ApiDoc::openapi())?;
    assert_eq!(
        openapi["components"]["securitySchemes"]["development_user"]["name"],
        DEVELOPMENT_USER_HEADER
    );
    assert_eq!(
        openapi["paths"]["/api/v1/tasks"]["get"]["security"],
        serde_json::json!([
            { "session_cookie": [] },
            { "development_user": [] }
        ])
    );

    let config = test_config();
    let router = app::build(&config, Store::from_pool(pool))?;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/projects")
        .header(header::CONTENT_TYPE, "application/json")
        .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
        .body(Body::from("{"))?;

    let response = router.clone().oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), 64 * 1024).await?;
    let body: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(body["error"]["code"], "invalid_request");
    assert!(body["error"]["message"].is_string());

    let missing = router
        .clone()
        .oneshot(Request::builder().uri("/missing").body(Body::empty())?)
        .await?;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(missing.into_body(), 64 * 1024).await?;
    let body: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(body["error"]["code"], "route_not_found");

    let wrong_method = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/health")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
    let body = to_bytes(wrong_method.into_body(), 64 * 1024).await?;
    let body: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(body["error"]["code"], "method_not_allowed");

    Ok(())
}

fn test_config() -> Config {
    Config {
        environment: Environment::Test,
        bind_address: "127.0.0.1:3000".parse().expect("test address"),
        database_url: "unused".to_owned(),
        database_max_connections: 1,
        cors_allowed_origin: "http://localhost:5173".to_owned(),
        app_url: "http://localhost:5173".to_owned(),
        allow_insecure_dev_auth: true,
        invite_only: false,
        groups_enabled: false,
        trust_proxy_headers: false,
        google_oauth: None,
        discord: None,
        object_storage: ObjectStorageConfig::Local {
            root: std::env::temp_dir()
                .join("prosepect-http-tests")
                .to_string_lossy()
                .into_owned(),
        },
        max_file_size_bytes: 25 * 1024 * 1024,
        max_user_file_storage_bytes: 5_i64 * 1024 * 1024 * 1024,
        max_total_file_storage_bytes: 5_i64 * 1024 * 1024 * 1024,
        max_user_accounts: None,
        admin_user_ids: Default::default(),
        worker_trigger_token: None,
        google_calendar_webhook_url: None,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn operations_authorizes_sessions_before_aggregates(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let owner = store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let other = store.ensure_development_user(uuid::Uuid::now_v7()).await?;
    let owner_session = store.create_session(owner).await?;
    let other_session = store.create_session(other).await?;
    let mut config = test_config();
    config.allow_insecure_dev_auth = false;
    config.admin_user_ids.insert(DEVELOPMENT_USER_ID);
    let router = app::build(&config, store.clone())?;
    for path in ["/api/v1/operations", "/api/v1/operations/capability"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let other_cookie = format!("prosepect_session={}", other_session.token);
    let rejected = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/operations")
                .header(header::COOKIE, &other_cookie)
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    for (cookie, allowed) in [
        (&other_cookie, false),
        (&format!("prosepect_session={}", owner_session.token), true),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/operations/capability")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(response.into_body(), 64 * 1024).await?;
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body)?, allowed);
    }
    // Ordinary session responses remain tenant-scoped, with no operations fields.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/session")
                .header(header::COOKIE, &other_cookie)
                .body(Body::empty())?,
        )
        .await?;
    let body = to_bytes(response.into_body(), 64 * 1024).await?;
    let body: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(body.as_object().unwrap().len(), 2);
    assert!(body.get("metrics").is_none());

    config.admin_user_ids.clear();
    let deny_all = app::build(&config, store)?;
    let rejected = deny_all
        .oneshot(
            Request::builder()
                .uri("/api/v1/operations")
                .header(
                    header::COOKIE,
                    format!("prosepect_session={}", owner_session.token),
                )
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn operations_counts_all_tenants_without_private_payloads(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let owner = store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let other_id = uuid::Uuid::now_v7();
    let other = store.ensure_development_user(other_id).await?;
    let other_session = store.create_session(other).await?;
    let session = store.create_session(owner).await?;
    let mut config = test_config();
    config.allow_insecure_dev_auth = false;
    config.admin_user_ids.insert(DEVELOPMENT_USER_ID);
    let router = app::build(&config, store)?;
    let request = || {
        Request::builder()
            .uri("/api/v1/operations")
            .header(
                header::COOKIE,
                format!("prosepect_session={}", session.token),
            )
            .body(Body::empty())
            .unwrap()
    };
    let empty = router.clone().oneshot(request()).await?;
    assert_eq!(empty.status(), StatusCode::OK);
    let empty: serde_json::Value =
        serde_json::from_slice(&to_bytes(empty.into_body(), 64 * 1024).await?)?;
    assert_eq!(empty["metrics"]["pending"], 0);
    assert_eq!(empty["metrics"]["file_bytes"], 0);
    assert!(empty["metrics"]["latest_completed_job_at"].is_null());
    assert!(empty["metrics"]["oldest_waiting_created_at"].is_null());
    assert!(empty["limits"]["max_user_accounts"].is_null());
    for (user, bytes) in [(DEVELOPMENT_USER_ID, 11_i64), (other_id, 19_i64)] {
        sqlx::query("INSERT INTO files (id, user_id, object_key, filename, content_type, byte_size) VALUES ($1, $2, $1::TEXT, 'private-title', 'text/plain', $3)")
            .bind(uuid::Uuid::now_v7()).bind(user).bind(bytes).execute(&pool).await?;
    }
    for (index, (status, attempts)) in [
        ("pending", 0),
        ("running", 1),
        ("failed", 7),
        ("failed", 8),
        ("failed", 9),
        ("succeeded", 1),
    ]
    .into_iter()
    .enumerate()
    {
        sqlx::query("INSERT INTO sync_jobs (id, user_id, kind, idempotency_key, status, attempt_count, last_error, created_at, updated_at) VALUES ($1, $2, CASE WHEN $4 = 'succeeded' THEN 'credential_revoke' ELSE 'calendar_discovery' END, $3, $4, $5, 'secret-provider-payload', '2026-01-01T00:00:00Z'::TIMESTAMPTZ + ($6 * INTERVAL '1 day'), CASE WHEN $4 = 'succeeded' THEN '2026-01-07T00:00:00Z'::TIMESTAMPTZ ELSE '2026-02-01T00:00:00Z'::TIMESTAMPTZ END)")
            .bind(uuid::Uuid::now_v7()).bind(if index % 2 == 0 { DEVELOPMENT_USER_ID } else { other_id })
            .bind(index.to_string()).bind(status).bind(attempts).bind(index as i32).execute(&pool).await?;
    }
    let ordinary = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/integrations/google")
                .header(
                    header::COOKIE,
                    format!("prosepect_session={}", other_session.token),
                )
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(ordinary.status(), StatusCode::OK);
    let ordinary: serde_json::Value =
        serde_json::from_slice(&to_bytes(ordinary.into_body(), 64 * 1024).await?)?;
    assert_eq!(ordinary["pending_synchronization_count"], 1);
    assert_eq!(ordinary["failed_synchronization_count"], 1);
    assert!(ordinary.get("metrics").is_none());
    assert!(ordinary.get("limits").is_none());

    config.max_user_accounts = Some(2);
    let router = app::build(&config, Store::from_pool(pool))?;
    let response = router.oneshot(request()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let raw = to_bytes(response.into_body(), 64 * 1024).await?;
    let text = std::str::from_utf8(&raw)?;
    for forbidden in [
        "secret-provider-payload",
        "last_error",
        "private-title",
        "development.invalid",
        &other_id.to_string(),
    ] {
        assert!(!text.contains(forbidden));
    }
    let body: serde_json::Value = serde_json::from_slice(&raw)?;
    assert_eq!(body["api"], "ok");
    assert_eq!(body["database"], "ok");
    let metrics = &body["metrics"];
    assert_eq!(metrics["accounts"], 2);
    assert_eq!(metrics["file_bytes"], 30);
    for key in ["pending", "running", "retryable", "succeeded_all_time"] {
        assert_eq!(metrics[key], 1, "{key}");
    }
    assert_eq!(metrics["final_failed_all_time"], 2); // A newer unrelated success cannot erase failures.
    assert_eq!(metrics["latest_completed_job_at"], "2026-01-07T00:00:00Z");
    assert_eq!(body["limits"]["max_user_accounts"], 2);
    assert_eq!(metrics["oldest_waiting_created_at"], "2026-01-01T00:00:00Z");
    assert!(body["as_of"].as_str().is_some());
    assert_eq!(
        body["limits"]["max_file_size_bytes"],
        config.max_file_size_bytes
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn operations_degrades_without_failing_open(pool: PgPool) -> anyhow::Result<()> {
    let mut config = test_config();
    config.admin_user_ids.insert(DEVELOPMENT_USER_ID);
    let router = app::build(&config, Store::from_pool(pool.clone()))?;
    pool.close().await;
    // Development authentication is DB-independent; real session authentication still fails closed.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/operations")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await?)?;
    assert_eq!(body["database"], "unavailable");
    assert!(body["metrics"].is_null());
    let denied = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/operations")
                .header(DEVELOPMENT_USER_HEADER, uuid::Uuid::now_v7().to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let session_failure = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/operations")
                .header(header::COOKIE, "prosepect_session=unverifiable")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(session_failure.status(), StatusCode::INTERNAL_SERVER_ERROR);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn operations_aggregate_failure_is_unknown_not_zero(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let owner = store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let session = store.create_session(owner).await?;
    let mut config = test_config();
    config.allow_insecure_dev_auth = false;
    config.admin_user_ids.insert(DEVELOPMENT_USER_ID);
    let router = app::build(&config, store)?;
    sqlx::query("DROP TABLE sync_jobs CASCADE")
        .execute(&pool)
        .await?;
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/operations")
                .header(
                    header::COOKIE,
                    format!("prosepect_session={}", session.token),
                )
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await?)?;
    assert_eq!(body["database"], "ok");
    assert!(body["metrics"].is_null());
    assert!(body.get("error").is_none());
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn calendar_move_and_undo_require_cookie_csrf(pool: PgPool) -> anyhow::Result<()> {
    let router = app::build(&test_config(), Store::from_pool(pool))?;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .body(Body::empty())?,
        )
        .await?;
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body = to_bytes(response.into_body(), 64 * 1024).await?;
    let session: serde_json::Value = serde_json::from_slice(&body)?;
    for path in [
        "/api/v1/events/00000000-0000-0000-0000-000000000001/move",
        "/api/v1/tasks/00000000-0000-0000-0000-000000000001/move",
        "/api/v1/calendar-move-undos/00000000-0000-0000-0000-000000000001/consume",
    ] {
        let response = router.clone().oneshot(Request::builder().method("POST").uri(path)
            .header(header::COOKIE,&cookie).header(header::CONTENT_TYPE,"application/json")
            .body(Body::from(r#"{"starts_at":"2026-09-10T10:00:00Z","ends_at":"2026-09-10T11:00:00Z","expected_version":1}"#))?).await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/calendar-move-undos/00000000-0000-0000-0000-000000000001/consume")
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", session["csrf_token"].as_str().unwrap())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn calendar_move_http_is_authenticated_owner_scoped_and_one_shot(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let router = app::build(&test_config(), store.clone())?;
    let session = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(session.status(), StatusCode::OK);
    let calendars = store.list_calendars(DEVELOPMENT_USER_ID).await?;
    let event = store
        .create_calendar_event(
            DEVELOPMENT_USER_ID,
            serde_json::from_value(serde_json::json!({
                "calendar_id":calendars.items[0].id,"title":"Move HTTP","description":"",
                "starts_at":"2026-09-10T10:00:00Z","ends_at":"2026-09-10T11:00:00Z",
                "all_day":false,"timezone":"UTC","location":"","attendees":[],"recurrence":"none"
            }))?,
        )
        .await?;
    let other = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO users (id,email,display_name) VALUES ($1,'other@example.test','Other')",
    )
    .bind(other)
    .execute(&pool)
    .await?;
    let moved = router.clone().oneshot(Request::builder().method("POST")
        .uri(format!("/api/v1/events/{}/move",event.id)).header(DEVELOPMENT_USER_HEADER,DEVELOPMENT_USER_ID.to_string())
        .header(header::CONTENT_TYPE,"application/json")
        .body(Body::from(r#"{"starts_at":"2026-09-10T12:00:00Z","ends_at":"2026-09-10T13:00:00Z","expected_version":1}"#))?).await?;
    assert_eq!(moved.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(moved.into_body(), 64 * 1024).await?)?;
    let consume = format!(
        "/api/v1/calendar-move-undos/{}/consume",
        body["id"].as_str().unwrap()
    );
    let unauthenticated = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/calendar-move-undos")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let other_list = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/calendar-move-undos")
                .header(DEVELOPMENT_USER_HEADER, other.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(other_list.status(), StatusCode::OK);
    let other_body: serde_json::Value =
        serde_json::from_slice(&to_bytes(other_list.into_body(), 64 * 1024).await?)?;
    assert_eq!(other_body["items"], serde_json::json!([]));
    for (user, expected) in [
        (other, StatusCode::NOT_FOUND),
        (DEVELOPMENT_USER_ID, StatusCode::NO_CONTENT),
        (DEVELOPMENT_USER_ID, StatusCode::NOT_FOUND),
    ] {
        let result = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&consume)
                    .header(DEVELOPMENT_USER_HEADER, user.to_string())
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(result.status(), expected);
    }
    Ok(())
}

// Protocol-only RED: this deliberately uses no reversible-deletion Rust symbols.
#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_http_restores_identity_once(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let router = app::build(&test_config(), store)?;
    let created = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tasks")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"title":"Recover this task","description":"Private contents"}"#,
                ))?,
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let task: serde_json::Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 64 * 1024).await?)?;
    let id = task["id"].as_str().expect("task id");
    let deleted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/tasks/{id}/delete-with-undo"))
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"expected_version": task["version"]}).to_string(),
                ))?,
        )
        .await?;
    assert_eq!(
        deleted.status(),
        StatusCode::OK,
        "explicit reversible deletion must exist"
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&to_bytes(deleted.into_body(), 64 * 1024).await?)?;
    assert_eq!(receipt["task_id"], task["id"]);
    assert_eq!(receipt["task_title"], task["title"]);
    assert_eq!(
        receipt.as_object().expect("receipt object").len(),
        4,
        "receipt must expose only the approved UI fields"
    );
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE id=$1")
        .bind(uuid::Uuid::parse_str(id)?)
        .fetch_one(&pool)
        .await?;
    assert_eq!(remaining, 0, "canonical row must be hard deleted");
    let listed = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/task-delete-undos")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: serde_json::Value =
        serde_json::from_slice(&to_bytes(listed.into_body(), 64 * 1024).await?)?;
    assert_eq!(listed["items"], serde_json::json!([receipt.clone()]));
    let consume = format!(
        "/api/v1/task-delete-undos/{}/consume",
        receipt["id"].as_str().expect("receipt id")
    );
    for expected in [StatusCode::NO_CONTENT, StatusCode::NOT_FOUND] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&consume)
                    .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), expected);
    }
    let restored: (String, String, i32) =
        sqlx::query_as("SELECT title, description, version FROM tasks WHERE id=$1")
            .bind(uuid::Uuid::parse_str(id)?)
            .fetch_one(&pool)
            .await?;
    assert_eq!(restored.0, "Recover this task");
    assert_eq!(restored.1, "Private contents");
    assert!(i64::from(restored.2) > task["version"].as_i64().unwrap());
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_http_requires_authentication_and_cookie_csrf(
    pool: PgPool,
) -> anyhow::Result<()> {
    let router = app::build(&test_config(), Store::from_pool(pool))?;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/task-delete-undos")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let session = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .body(Body::empty())?,
        )
        .await?;
    let cookie = session.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap();
    for path in [
        "/api/v1/tasks/00000000-0000-0000-0000-000000000001/delete-with-undo",
        "/api/v1/task-delete-undos/00000000-0000-0000-0000-000000000001/consume",
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(header::COOKIE, cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"expected_version":1}"#))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn google_tasks_settings_are_authenticated_and_do_not_infer_calendar_permission(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool);
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let router = app::build(&test_config(), store)?;
    let denied = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/integrations/google/tasks")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .body(Body::empty())?,
        )
        .await?;
    let status = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/integrations/google/tasks")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(status.status(), StatusCode::OK);
    let status: serde_json::Value =
        serde_json::from_slice(&to_bytes(status.into_body(), 65536).await?)?;
    assert_eq!(status["authorized"], false);
    assert_eq!(status["enabled"], false);
    assert_eq!(status["version"], 0);
    for path in [
        "/api/v1/integrations/google/tasks/lists",
        "/api/v1/integrations/google/tasks/sync",
    ] {
        let method = if path.ends_with("sync") {
            "POST"
        } else {
            "GET"
        };
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                    .body(Body::empty())?,
            )
            .await?;
        assert!(matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::CONFLICT
        ));
    }
    let disabled = router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/integrations/google/tasks")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":false,"expected_version":0}"#))?,
        )
        .await?;
    assert_eq!(disabled.status(), StatusCode::OK);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn google_tasks_consent_requires_authentication_without_starting_sync(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let router = app::build(&test_config(), store)?;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/google/tasks/start")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let session = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(session.status(), StatusCode::OK);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/google/tasks/start")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_jobs WHERE user_id=$1")
        .bind(DEVELOPMENT_USER_ID)
        .fetch_one(&pool)
        .await?;
    assert_eq!(jobs, 0, "consent must not enqueue provider writes");
    Ok(())
}
