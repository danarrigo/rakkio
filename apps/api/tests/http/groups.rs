use super::*;
use serde_json::{Value, json};

#[sqlx::test(migrations = "../../migrations")]
async fn groups_are_authenticated_and_unavailable_until_server_rollout(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let router = app::build(&test_config(), store)?;
    let anonymous = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/groups/capability")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let status = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/groups/capability")
                .header(DEVELOPMENT_USER_HEADER, DEVELOPMENT_USER_ID.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(status.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&to_bytes(status.into_body(), 65536).await?)?;
    assert_eq!(body, json!({"enabled":false}));
    let create=router.oneshot(Request::builder().method("POST").uri("/api/v1/groups")
        .header(DEVELOPMENT_USER_HEADER,DEVELOPMENT_USER_ID.to_string()).header(header::CONTENT_TYPE,"application/json")
        .body(Body::from(json!({"expected_user_id":DEVELOPMENT_USER_ID,"name":"Class","timezone":"Asia/Jakarta"}).to_string()))?).await?;
    assert_eq!(create.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}
