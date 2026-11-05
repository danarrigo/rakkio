use super::*;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    routing::any,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Clone, Default)]
struct Bucket {
    objects: Arc<Mutex<HashMap<String, bytes::Bytes>>>,
    started: Arc<Notify>,
    release: Arc<Notify>,
}
async fn object(
    State(bucket): State<Bucket>,
    Path(key): Path<String>,
    request: axum::extract::Request,
) -> axum::response::Response {
    match request.method().as_str() {
        "PUT" => {
            let bytes = to_bytes(request.into_body(), 65536).await.unwrap();
            bucket.started.notify_one();
            bucket.release.notified().await;
            bucket.objects.lock().unwrap().insert(key, bytes);
            ([(header::ETAG, "\"fixture-etag\"")], "").into_response()
        }
        "DELETE" => {
            bucket.objects.lock().unwrap().remove(&key);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}
async fn upload_fixture(
    pool: &PgPool,
) -> anyhow::Result<(axum::Router, Bucket, tokio::task::JoinHandle<()>)> {
    let bucket = Bucket::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let service = axum::Router::new()
        .route("/{*key}", any(object))
        .with_state(bucket.clone());
    let server = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
    let store = Store::from_pool(pool.clone());
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let mut config = test_config();
    config.object_storage = ObjectStorageConfig::S3 {
        bucket: "uploads".into(),
        region: "us-east-1".into(),
        endpoint: Some(format!("http://{address}")),
        public_endpoint: None,
        access_key_id: "fixture-only".into(),
        secret_access_key: "fixture-only".into(),
        allow_http: true,
        virtual_hosted_style: false,
    };
    Ok((app::build(&config, store)?, bucket, server))
}
fn upload(
    router: axum::Router,
) -> tokio::task::JoinHandle<anyhow::Result<axum::response::Response>> {
    tokio::spawn(async move {
        Ok(router.oneshot(Request::builder().method("POST").uri("/api/v1/files").header(DEVELOPMENT_USER_HEADER,DEVELOPMENT_USER_ID.to_string()).header(header::CONTENT_TYPE,"multipart/form-data; boundary=fixture").body(Body::from("--fixture\r\nContent-Disposition: form-data; name=\"file\"; filename=\"sample.txt\"\r\nContent-Type: text/plain\r\n\r\ncontents\r\n--fixture--\r\n"))?).await?)
    })
}
async fn assert_no_untracked_objects(pool: &PgPool, bucket: &Bucket) -> anyhow::Result<()> {
    let tracked: Vec<String> = sqlx::query_scalar("SELECT object_key FROM files")
        .fetch_all(pool)
        .await?;
    let objects = bucket.objects.lock().unwrap();
    assert!(
        objects
            .keys()
            .all(|key| tracked.iter().any(|tracked| key.ends_with(tracked))),
        "a completed object write escaped metadata/quota accounting"
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn in_flight_upload_cannot_be_deleted_and_leave_an_uncharged_object(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, bucket, server) = upload_fixture(&pool).await?;
    let pending = upload(router.clone());
    tokio::time::timeout(std::time::Duration::from_secs(5), bucket.started.notified()).await?;
    let listing = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/files")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    let listing: serde_json::Value =
        serde_json::from_slice(&to_bytes(listing.into_body(), 65536).await?)?;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM files LIMIT 1")
        .fetch_one(&pool)
        .await?;
    let deleted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/files/{id}"))
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    let status = deleted.status();
    let _ = to_bytes(deleted.into_body(), 65536).await?;
    bucket.release.notify_one();
    let response = pending.await??;
    let _ = to_bytes(response.into_body(), 65536).await?;
    server.abort();
    assert_no_untracked_objects(&pool, &bucket).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(listing["items"], serde_json::json!([]));
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn account_erasure_during_upload_does_not_leave_unowned_storage(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, bucket, server) = upload_fixture(&pool).await?;
    let pending = upload(router.clone());
    tokio::time::timeout(std::time::Duration::from_secs(5), bucket.started.notified()).await?;
    let deleted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/account")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"confirmation":"DELETE"}"#))?,
        )
        .await?;
    let status = deleted.status();
    let _ = to_bytes(deleted.into_body(), 65536).await?;
    bucket.release.notify_one();
    let response = pending.await??;
    let _ = to_bytes(response.into_body(), 65536).await?;
    server.abort();
    assert_no_untracked_objects(&pool, &bucket).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn forwarded_chains_cannot_rotate_authentication_rate_limit_identity(
    pool: PgPool,
) -> anyhow::Result<()> {
    let mut config = test_config();
    config.trust_proxy_headers = true;
    let router = app::build(&config, Store::from_pool(pool))?;
    for n in 0..11 {
        let response=router.clone().oneshot(Request::builder().uri("/api/v1/auth/google/start?terms_version=outdated&privacy_version=outdated&age_confirmed=true").header("x-forwarded-for",format!("198.51.100.{n}, 10.0.0.1")).extension(axum::extract::ConnectInfo("203.0.113.7:54321".parse::<std::net::SocketAddr>()?)).body(Body::empty())?).await?;
        let status = response.status();
        let _ = to_bytes(response.into_body(), 65536).await?;
        assert_eq!(
            status,
            if n < 10 {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn metrics_requires_an_authenticated_operations_owner(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool);
    let owner = Uuid::now_v7();
    let member = Uuid::now_v7();
    store.ensure_development_user(owner).await?;
    store.ensure_development_user(member).await?;
    let mut config = test_config();
    config.admin_user_ids.insert(owner);
    let router = app::build(&config, store)?;
    for (user, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(member), StatusCode::FORBIDDEN),
        (Some(owner), StatusCode::OK),
    ] {
        let mut request = Request::builder().uri("/metrics");
        if let Some(user) = user {
            request = request.header(DEVELOPMENT_USER_HEADER, user.to_string());
        }
        let response = router.clone().oneshot(request.body(Body::empty())?).await?;
        let actual = response.status();
        let _ = to_bytes(response.into_body(), 65536).await?;
        assert_eq!(actual, status);
    }
    Ok(())
}
