use super::*;
use serde_json::{Value, json};
use uuid::Uuid;

async fn claim(
    router: axum::Router,
    user: Uuid,
    handle: &str,
) -> anyhow::Result<(StatusCode, Value)> {
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profile/handle")
                .header(DEVELOPMENT_USER_HEADER, user.to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"handle":handle}).to_string()))?,
        )
        .await?;
    let status = response.status();
    Ok((
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?,
    ))
}

#[sqlx::test(migrations = "../../migrations")]
async fn handles_are_case_insensitive_immutable_private_and_not_recycled(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let owner = DEVELOPMENT_USER_ID;
    let other = Uuid::now_v7();
    store.ensure_development_user(owner).await?;
    store.ensure_development_user(other).await?;
    let router = app::build(&test_config(), store.clone())?;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/profile/handle")
                .header(DEVELOPMENT_USER_HEADER, owner.to_string())
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?;
    assert_eq!(body, json!({"handle":null}));
    let (status, body) = claim(router.clone(), owner, " @Daniel ").await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"handle":"daniel"}));
    assert_eq!(
        claim(router.clone(), owner, "DANIEL").await?.0,
        StatusCode::OK
    );
    assert_eq!(
        claim(router.clone(), owner, "different").await?.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        claim(router.clone(), other, "@DANIEL").await?.0,
        StatusCode::CONFLICT
    );
    let export: Value = serde_json::from_slice(&store.export_json(owner).await?)?;
    assert_eq!(export["handle"], "daniel");
    let other_export: Value = serde_json::from_slice(&store.export_json(other).await?)?;
    assert!(other_export["handle"].is_null());
    store.delete_account(owner, "DELETE").await?;
    assert_eq!(
        claim(router, other, "daniel").await?.0,
        StatusCode::CONFLICT
    );
    let retired: (Option<Uuid>, Option<String>) =
        sqlx::query_as("SELECT user_id,handle FROM user_handles")
            .fetch_one(&pool)
            .await?;
    assert_eq!(retired, (None, None));
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn concurrent_handle_claims_have_one_winner(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let a = Uuid::now_v7();
    let b = Uuid::now_v7();
    store.ensure_development_user(a).await?;
    store.ensure_development_user(b).await?;
    let router = app::build(&test_config(), store)?;
    let (a, b) = tokio::join!(
        claim(router.clone(), a, "Classmate"),
        claim(router, b, "@CLASSMATE")
    );
    let mut statuses = [a?.0.as_u16(), b?.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn handle_claims_require_auth_csrf_and_strict_valid_input(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool);
    let owner = DEVELOPMENT_USER_ID;
    store.ensure_development_user(owner).await?;
    let router = app::build(&test_config(), store)?;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/profile/handle")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for value in [
        "ab",
        "a".repeat(31).as_str(),
        "admIN",
        "rakkio",
        "everyone",
        "alice.smith",
        "two words",
        "Ａlice",
        "@@alice",
    ] {
        assert_eq!(
            claim(router.clone(), owner, value).await?.0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{value}"
        );
    }
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
    let session: Value = serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?;
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/api/v1/profile/handle")
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/json")
    };
    let response = router
        .clone()
        .oneshot(request().body(Body::from(json!({"handle":"classmate"}).to_string()))?)
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = router
        .clone()
        .oneshot(
            request()
                .header("x-csrf-token", session["csrf_token"].as_str().unwrap())
                .body(Body::from(
                    json!({"handle":"classmate","user_id":owner}).to_string(),
                ))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = router
        .oneshot(
            request()
                .header("x-csrf-token", session["csrf_token"].as_str().unwrap())
                .body(Body::from(json!({"handle":"classmate"}).to_string()))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}
