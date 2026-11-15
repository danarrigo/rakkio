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
    for _ in 0..11 {
        let response = client
            .get(format!("http://{address}/health"))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        tokio::time::sleep(StdDuration::from_secs(1)).await;
    }
    let after = last_database_activity(&mut observer).await?;
    server.abort();
    assert_eq!(
        before, after,
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
