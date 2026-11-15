use super::*;
use sqlx::PgConnection;
use std::{net::SocketAddr, time::Duration as StdDuration};

async fn last_database_activity(
    observer: &mut PgConnection,
) -> anyhow::Result<Option<chrono::DateTime<Utc>>> {
    Ok(sqlx::query_scalar(
        "SELECT MAX(query_start) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()",
    )
    .fetch_one(observer)
    .await?)
}

async fn assert_idle_health_checks_do_not_query_postgres(
    pool: PgPool,
    google_enabled: bool,
) -> anyhow::Result<()> {
    let mut config = test_config();
    if google_enabled {
        config.google_oauth = Some(GoogleOAuthConfig {
            client_id: "unused-client".into(),
            client_secret: "unused-secret".into(),
            redirect_uri: "http://localhost/callback".into(),
            token_encryption_key: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".into(),
        });
    }
    // No users, provider credentials, or jobs: startup and real HTTP health checks
    // must not turn an otherwise idle database into a continuous polling target.
    let router = app::build(&config, Store::from_pool(pool.clone()))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    // Hold a separate observer connection. Its own statistics queries are excluded
    // by backend PID, and no production database or provider is involved.
    let mut observer = pool.acquire().await?;
    let before = tokio::time::timeout(StdDuration::from_secs(10), async {
        loop {
            let first = last_database_activity(&mut observer).await?;
            tokio::time::sleep(StdDuration::from_millis(300)).await;
            let second = last_database_activity(&mut observer).await?;
            if first.is_some() && first == second {
                break Ok::<_, anyhow::Error>(second);
            }
        }
    })
    .await??;
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(5))
        .build()?;
    let mut newest_activity = before;
    for _ in 0..11 {
        let response = client
            .get(format!("http://{address}/health"))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        tokio::time::sleep(StdDuration::from_secs(1)).await;
        // Idle SQLx test connections may close. Preserve every observed maximum
        // instead of mistaking disappearance for activity or missing an earlier poll.
        newest_activity = newest_activity.max(last_database_activity(&mut observer).await?);
    }
    server.abort();
    assert_eq!(
        before, newest_activity,
        "idle API and repeated health checks must not issue background database queries"
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn idle_google_api_and_health_checks_leave_postgres_quiet(
    pool: PgPool,
) -> anyhow::Result<()> {
    assert_idle_health_checks_do_not_query_postgres(pool, true).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn idle_maintenance_api_and_health_checks_leave_postgres_quiet(
    pool: PgPool,
) -> anyhow::Result<()> {
    assert_idle_health_checks_do_not_query_postgres(pool, false).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn authenticated_mutation_schedules_future_cleanup_without_idle_polling(
    pool: PgPool,
) -> anyhow::Result<()> {
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
                .body(Body::from(r#"{"title":"Deadline cleanup fixture"}"#))?,
        )
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let task: serde_json::Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 65536).await?)?;
    let deleted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/tasks/{}/delete-with-undo",
                    task["id"].as_str().unwrap()
                ))
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"expected_version":{}}}"#,
                    task["version"]
                )))?,
        )
        .await?;
    assert_eq!(deleted.status(), StatusCode::OK);
    // Shorten only this disposable receipt, then notify through an ordinary HTTP
    // mutation. No manual dispatcher wake, run_once or future cleanup call.
    sqlx::query("UPDATE task_delete_undos SET expires_at=clock_timestamp()+INTERVAL '2 seconds'")
        .execute(&pool)
        .await?;
    let note = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/notes")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"title":"Real activity"}"#))?,
        )
        .await?;
    assert_eq!(note.status(), StatusCode::CREATED);
    tokio::time::timeout(StdDuration::from_secs(4), async {
        loop {
            let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_delete_undos")
                .fetch_one(&pool)
                .await?;
            if remaining == 0 {
                break Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(StdDuration::from_millis(25)).await;
        }
    })
    .await??;
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn health_stays_live_but_readiness_fails_when_database_is_unavailable(
    pool: PgPool,
) -> anyhow::Result<()> {
    let router = app::build(&test_config(), Store::from_pool(pool.clone()))?;
    pool.close().await;
    for (path, status) in [
        ("/health", StatusCode::OK),
        ("/ready", StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), status);
    }
    Ok(())
}
