use super::*;

#[sqlx::test(migrations = "../../migrations")]
async fn slow_account_writes_do_not_consume_every_accounts_capacity(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool);
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let other = uuid::Uuid::now_v7();
    store.ensure_development_user(other).await?;
    let router = app::build(&test_config(), store)?;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(5));
    let mut pending = Vec::new();
    for _ in 0..4 {
        let router = router.clone();
        let barrier = barrier.clone();
        let body = Body::from_stream(futures_util::stream::once(async move {
            barrier.wait().await;
            std::future::pending::<Result<bytes::Bytes, std::io::Error>>().await
        }));
        pending.push(tokio::spawn(async move {
            router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/notes")
                        .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(body)
                        .unwrap(),
                )
                .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), barrier.wait()).await?;
    let busy = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/notes")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Busy"}"#))?,
        )
        .await?;
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
    let available = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/notes")
                .header(DEVELOPMENT_USER_HEADER, other.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Other account"}"#))?,
        )
        .await?;
    assert_eq!(available.status(), StatusCode::CREATED);
    for task in pending {
        task.abort();
        let _ = task.await;
    }
    let available = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/notes")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Released"}"#))?,
        )
        .await?;
    assert_eq!(available.status(), StatusCode::CREATED);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn write_admission_is_charged_once_and_keeps_csrf_protected_logout_available(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool);
    let user = store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let session = store.create_session(user).await?;
    let cookie = format!("prosepect_session={}", session.token);
    let csrf = session.response.csrf_token;
    let mut config = test_config();
    config.allow_insecure_dev_auth = false;
    let router = app::build(&config, store)?;
    for index in 0..241 {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/notes")
                    .header(header::COOKIE, &cookie)
                    .header("x-csrf-token", &csrf)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("not-json"))?,
            )
            .await?;
        assert_eq!(
            response.status(),
            if index < 240 {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    let denied = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/session/logout")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let logout = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/session/logout")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", csrf)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let expired = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/session")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

fn oauth_config() -> Config {
    let mut config = test_config();
    config.google_oauth = Some(GoogleOAuthConfig {
        client_id: "test-client".to_owned(),
        client_secret: "test-secret".to_owned(),
        redirect_uri: "http://localhost/callback".to_owned(),
        token_encryption_key: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_owned(),
    });
    config
}

#[sqlx::test(migrations = "../../migrations")]
async fn content_quota_is_an_actionable_error_without_partial_writes(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    sqlx::query("UPDATE workspace_content_limits SET user_records = 1")
        .execute(&pool)
        .await?;
    let router = app::build(&test_config(), store)?;
    for expected in [StatusCode::CREATED, StatusCode::FORBIDDEN] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/notes")
                    .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"title":"Quota","markdown":"Text"}"#))?,
            )
            .await?;
        assert_eq!(response.status(), expected);
        if expected == StatusCode::FORBIDDEN {
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?;
            assert_eq!(body["error"]["code"], "content_quota_exceeded");
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

const LOGIN_START: &str = "/api/v1/auth/google/start?terms_version=2026-10-04&privacy_version=2026-11-03-handles&age_confirmed=true";

fn oauth_response(response: &axum::response::Response) -> anyhow::Result<(String, String)> {
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let url = reqwest::Url::parse(response.headers()[header::LOCATION].to_str()?)?;
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    assert!(!url.as_str().contains(cookie.split_once('=').unwrap().1));
    Ok((cookie, state))
}

#[sqlx::test(migrations = "../../migrations")]
async fn oauth_callback_rejects_other_browsers_without_burning_the_attempt(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let router = app::build(&oauth_config(), store.clone())?;
    let first = router
        .clone()
        .oneshot(Request::builder().uri(LOGIN_START).body(Body::empty())?)
        .await?;
    let (cookie, first_state) = oauth_response(&first)?;
    let second = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(LOGIN_START)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    let (second_cookie, second_state) = oauth_response(&second)?;
    assert_eq!(
        cookie, second_cookie,
        "independent tabs must share the protected browser binding"
    );
    assert_ne!(first_state, second_state);
    for supplied in [
        String::new(),
        "prosepect_oauth=short".to_owned(),
        format!("prosepect_oauth={}", "A".repeat(43)),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/auth/google/callback?state={first_state}&code=offline-unused"
                    ))
                    .header(header::COOKIE, supplied)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
    let binding = cookie.split_once('=').unwrap().1;
    store
        .consume_google_login_attempt(&first_state, binding)
        .await?;
    store
        .consume_google_login_attempt(&second_state, binding)
        .await?;
    assert!(
        store
            .consume_google_login_attempt(&first_state, binding)
            .await
            .is_err()
    );
    let third = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(LOGIN_START)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    let (_, expired_state) = oauth_response(&third)?;
    sqlx::query("UPDATE oauth_login_attempts SET expires_at = NOW() - INTERVAL '1 second'")
        .execute(&pool)
        .await?;
    let response = router
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/auth/google/callback?state={expired_state}&code=offline-unused"
                ))
                .header(header::COOKIE, cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn oauth_production_binding_uses_host_only_secure_cookie(pool: PgPool) -> anyhow::Result<()> {
    let mut config = oauth_config();
    config.environment = Environment::Production;
    config.allow_insecure_dev_auth = false;
    let router = app::build(&config, Store::from_pool(pool))?;
    let response = router
        .oneshot(Request::builder().uri(LOGIN_START).body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    let cookie = response.headers()[header::SET_COOKIE].to_str()?;
    assert!(cookie.starts_with("__Host-prosepect_oauth="));
    assert!(cookie.contains("; Secure"));
    assert!(cookie.contains("; HttpOnly"));
    assert!(cookie.contains("; Path=/"));
    assert!(!cookie.to_lowercase().contains("domain="));
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn incremental_consent_rejects_changed_login_session_before_exchange(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool);
    let owner = store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let other = store.ensure_development_user(uuid::Uuid::now_v7()).await?;
    let owner = store.create_session(owner).await?;
    let other = store.create_session(other).await?;
    let router = app::build(&oauth_config(), store)?;
    for purpose in ["tasks", "calendar"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/auth/google/{purpose}/start"))
                    .header(header::COOKIE, format!("prosepect_session={}", owner.token))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        let (binding, state) = oauth_response(&response)?;
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/auth/google/callback?state={state}&code=offline-unused"
                    ))
                    .header(
                        header::COOKIE,
                        format!("{binding}; prosepect_session={}", other.token),
                    )
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn multipart_fields_are_bounded_and_rejected_before_metadata_creation(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let mut config = test_config();
    config.max_file_size_bytes = 8;
    let router = app::build(&config, store)?;
    for (fields, expected) in [
        ("Content-Disposition: form-data; name=\"file\"; filename=\"large.txt\"\r\n\r\n123456789".to_owned(), StatusCode::PAYLOAD_TOO_LARGE),
        (format!("Content-Disposition: form-data; name=\"project_id\"\r\n\r\n{}", "x".repeat(65)), StatusCode::PAYLOAD_TOO_LARGE),
        ("Content-Disposition: form-data; name=\"unexpected\"\r\n\r\nx".to_owned(), StatusCode::UNPROCESSABLE_ENTITY),
        ("Content-Disposition: form-data; name=\"file\"; filename=\"one.txt\"\r\n\r\nx\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"two.txt\"\r\n\r\ny".to_owned(), StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let response = router.clone().oneshot(Request::builder().method("POST").uri("/api/v1/files")
            .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=boundary")
            .body(Body::from(format!("--boundary\r\n{fields}\r\n--boundary--\r\n")))?).await?;
        assert_eq!(response.status(), expected);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn oauth_start_sets_a_short_lived_browser_binding(pool: PgPool) -> anyhow::Result<()> {
    let mut config = test_config();
    config.google_oauth = Some(GoogleOAuthConfig {
        client_id: "test-client".to_owned(),
        client_secret: "test-secret".to_owned(),
        redirect_uri: "http://localhost/callback".to_owned(),
        token_encryption_key: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_owned(),
    });
    let router = app::build(&config, Store::from_pool(pool))?;
    let response = router.oneshot(Request::builder()
        .uri("/api/v1/auth/google/start?terms_version=2026-10-04&privacy_version=2026-11-03-handles&age_confirmed=true")
        .body(Body::empty())?).await?;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("OAuth start must bind the receiving browser")
        .to_str()?;
    assert!(cookie.starts_with("prosepect_oauth="));
    for required in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=600"] {
        assert!(cookie.contains(required));
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn json_is_bounded_before_deserialization(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool);
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let router = app::build(&test_config(), store)?;
    let body = serde_json::json!({"title":"Oversized", "markdown":"x".repeat(2 * 1024 * 1024)})
        .to_string();
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/notes")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn csv_export_neutralizes_untrusted_spreadsheet_cells(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool);
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    store
        .create_task(
            DEVELOPMENT_USER_ID,
            serde_json::from_value(serde_json::json!({
                "title":"=1+1", "description":"@SUM(1,2)", "labels":["+example"]
            }))?,
        )
        .await?;
    let router = app::build(&test_config(), store)?;
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/exports/tasks.csv")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let csv = String::from_utf8(to_bytes(response.into_body(), 65536).await?.to_vec())?;
    assert!(
        csv.contains("'=1+1"),
        "CSV export must neutralize formula prefixes"
    );
    assert!(csv.contains("\"'@SUM(1,2)\""));
    assert!(csv.contains("'+example"));
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn api_responses_apply_security_headers(pool: PgPool) -> anyhow::Result<()> {
    let router = app::build(&test_config(), Store::from_pool(pool))?;
    for path in ["/health", "/api/v1/session", "/unknown"] {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty())?)
            .await?;
        assert_eq!(
            response
                .headers()
                .get("x-content-type-options")
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
        assert_eq!(
            response
                .headers()
                .get("x-frame-options")
                .and_then(|v| v.to_str().ok()),
            Some("DENY")
        );
    }
    Ok(())
}
