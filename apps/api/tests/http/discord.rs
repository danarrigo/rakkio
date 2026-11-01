use super::*;
use aws_lc_rs::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use prosepect_api::discord::DiscordConfig;
use serde_json::{Value, json};
use uuid::Uuid;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn fixture(pool: &PgPool) -> anyhow::Result<(axum::Router, Store, Ed25519KeyPair, Uuid)> {
    let store = Store::from_pool(pool.clone());
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let other = Uuid::now_v7();
    store.ensure_development_user(other).await?;
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| anyhow::anyhow!("test signing key"))?;
    let key = Ed25519KeyPair::from_pkcs8(document.as_ref())
        .map_err(|_| anyhow::anyhow!("test signing key"))?;
    let mut config = test_config();
    config.discord = Some(DiscordConfig {
        application_id: "123456789012345678".into(),
        client_secret: "test-discord-client-secret".into(),
        public_key: hex(key.public_key().as_ref()),
        redirect_uri: "http://localhost:3000/api/v1/auth/discord/callback".into(),
    });
    let router = app::build(&config, store.clone())?;
    Ok((router, store, key, other))
}

async fn link(pool: &PgPool, user: Uuid, discord: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO discord_connections(id,user_id,discord_user_id,username,timezone,consent_version) VALUES($1,$2,$3,'test-user','Asia/Jakarta','2026-10-31')")
        .bind(id).bind(user).bind(discord).execute(pool).await?;
    Ok(id)
}

fn command(id: u64, actor: &str, name: &str, options: Value) -> Value {
    json!({"id":id.to_string(),"application_id":"123456789012345678","type":2,
        "token":"test-interaction-token","user":{"id":actor},
        "data":{"name":name,"type":1,"options":options}})
}

async fn send(
    router: &axum::Router,
    key: &Ed25519KeyPair,
    payload: Value,
) -> anyhow::Result<(StatusCode, Value)> {
    let raw = serde_json::to_vec(&payload)?;
    let timestamp = Utc::now().timestamp().to_string();
    let signed = [timestamp.as_bytes(), raw.as_slice()].concat();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhooks/discord")
                .header("x-signature-timestamp", timestamp)
                .header("x-signature-ed25519", hex(key.sign(&signed).as_ref()))
                .header("content-type", "application/json")
                .body(Body::from(raw))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

#[sqlx::test(migrations = "../../migrations")]
async fn signed_capture_is_private_literal_owner_bound_and_idempotent(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, store, key, other) = fixture(&pool).await?;
    link(&pool, DEVELOPMENT_USER_ID, "111111111111111111").await?;
    link(&pool, other, "222222222222222222").await?;
    let payload = command(
        100,
        "111111111111111111",
        "add",
        json!([
            {"name":"title","value":"@everyone #Project p1 tomorrow"},
            {"name":"due","value":"tomorrow"}
        ]),
    );
    let (status, first) = send(&router, &key, payload.clone()).await?;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["type"], 4);
    assert_eq!(first["data"]["flags"], 64);
    assert_eq!(first["data"]["allowed_mentions"]["parse"], json!([]));
    let (_, repeated) = send(&router, &key, payload).await?;
    assert_eq!(first, repeated);
    let tasks = store
        .list_tasks(DEVELOPMENT_USER_ID, None, None, 100)
        .await?
        .items;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "@everyone #Project p1 tomorrow");
    assert!(tasks[0].project_id.is_none());
    assert!(tasks[0].labels.is_empty());
    assert!(tasks[0].scheduled_start.is_none());
    assert_eq!(
        tasks[0].priority,
        prosepect_api::models::TaskPriority::Medium
    );
    let correct:bool=sqlx::query_scalar("SELECT (due_at AT TIME ZONE 'Asia/Jakarta')::time=TIME '23:59:00' AND (due_at AT TIME ZONE 'Asia/Jakarta')::date=(NOW() AT TIME ZONE 'Asia/Jakarta')::date+1 FROM tasks WHERE id=$1")
        .bind(tasks[0].id).fetch_one(&pool).await?;
    assert!(correct);
    assert!(
        store
            .list_tasks(other, None, None, 100)
            .await?
            .items
            .is_empty()
    );
    let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM discord_interactions")
        .fetch_one(&pool)
        .await?;
    assert_eq!(receipts, 1);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn today_and_completion_never_cross_accounts_or_rebase_stale_versions(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, store, key, other) = fixture(&pool).await?;
    let owner_link = link(&pool, DEVELOPMENT_USER_ID, "111111111111111111").await?;
    link(&pool, other, "222222222222222222").await?;
    send(
        &router,
        &key,
        command(
            101,
            "111111111111111111",
            "add",
            json!([
                {"name":"title","value":"Owner task"},{"name":"due","value":"today"}
            ]),
        ),
    )
    .await?;
    send(
        &router,
        &key,
        command(
            102,
            "222222222222222222",
            "add",
            json!([
                {"name":"title","value":"Other private task"},{"name":"due","value":"today"}
            ]),
        ),
    )
    .await?;
    let (_, today) = send(
        &router,
        &key,
        command(103, "111111111111111111", "today", json!([])),
    )
    .await?;
    let text = today["data"]["content"].as_str().unwrap();
    assert!(text.contains("Owner task"));
    assert!(!text.contains("Other private task"));
    let (_, menu) = send(
        &router,
        &key,
        command(104, "111111111111111111", "done", json!([])),
    )
    .await?;
    let select = &menu["data"]["components"][0]["components"][0];
    assert_eq!(select["options"].as_array().unwrap().len(), 1);
    let mut selected = json!({"id":"105","application_id":"123456789012345678","type":3,"token":"test-interaction-token",
        "user":{"id":"111111111111111111"},"data":{"component_type":3,"custom_id":select["custom_id"],"values":[select["options"][0]["value"]]}});
    let mut stolen = selected.clone();
    stolen["id"] = json!("106");
    stolen["user"]["id"] = json!("222222222222222222");
    let (_, rejected) = send(&router, &key, stolen).await?;
    assert!(
        rejected["data"]["content"]
            .as_str()
            .unwrap()
            .contains("expired or changed")
    );
    let task = store
        .list_tasks(DEVELOPMENT_USER_ID, None, None, 100)
        .await?
        .items
        .remove(0);
    sqlx::query("UPDATE tasks SET title='Edited in web',version=version+1 WHERE id=$1")
        .bind(task.id)
        .execute(&pool)
        .await?;
    let (_, stale) = send(&router, &key, selected.clone()).await?;
    assert!(
        stale["data"]["content"]
            .as_str()
            .unwrap()
            .contains("task changed")
    );
    assert_eq!(
        store
            .list_tasks(DEVELOPMENT_USER_ID, None, None, 100)
            .await?
            .items[0]
            .status,
        prosepect_api::models::TaskStatus::Todo
    );
    let (_, fresh) = send(
        &router,
        &key,
        command(107, "111111111111111111", "done", json!([])),
    )
    .await?;
    let select = &fresh["data"]["components"][0]["components"][0];
    selected["id"] = json!("108");
    selected["data"]["custom_id"] = select["custom_id"].clone();
    selected["data"]["values"] = json!([select["options"][0]["value"]]);
    let (_, completed) = send(&router, &key, selected.clone()).await?;
    assert!(
        completed["data"]["content"]
            .as_str()
            .unwrap()
            .contains("Completed: Edited in web")
    );
    let (_, retry) = send(&router, &key, selected).await?;
    assert_eq!(retry, completed);
    assert_eq!(
        store
            .list_tasks(DEVELOPMENT_USER_ID, None, None, 100)
            .await?
            .items[0]
            .status,
        prosepect_api::models::TaskStatus::Completed
    );
    store
        .disconnect_discord(DEVELOPMENT_USER_ID, owner_link)
        .await?;
    let (_, disconnected) = send(
        &router,
        &key,
        command(109, "111111111111111111", "today", json!([])),
    )
    .await?;
    assert!(
        disconnected["data"]["content"]
            .as_str()
            .unwrap()
            .contains("Connect your Discord")
    );
    assert_eq!(
        store.list_tasks(other, None, None, 100).await?.items[0].status,
        prosepect_api::models::TaskStatus::Todo
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn bad_signatures_stale_requests_and_wrong_app_cannot_mutate(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, _, key, _) = fixture(&pool).await?;
    link(&pool, DEVELOPMENT_USER_ID, "111111111111111111").await?;
    let mut payload = command(
        110,
        "111111111111111111",
        "add",
        json!([{ "name":"title","value":"Must not save" }]),
    );
    payload["application_id"] = json!("999999999999999999");
    assert_eq!(send(&router, &key, payload).await?.0, StatusCode::FORBIDDEN);
    for timestamp in [Utc::now().timestamp() - 301, i64::MIN, i64::MAX] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhooks/discord")
                    .header("x-signature-timestamp", timestamp.to_string())
                    .header("x-signature-ed25519", "00".repeat(64))
                    .body(Body::from("{}"))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhooks/discord")
                .header("x-signature-timestamp", Utc::now().timestamp().to_string())
                .header("x-signature-ed25519", "00".repeat(64))
                .body(Body::from("{}"))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&pool)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn linking_requires_csrf_explicit_consent_browser_and_current_owner(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (_, store, _, other) = fixture(&pool).await?;
    let user = store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let session = store.create_session(user).await?;
    let mut config = test_config();
    config.allow_insecure_dev_auth = false;
    config.discord = Some(DiscordConfig {
        application_id: "123456789012345678".into(),
        client_secret: "test-secret".into(),
        public_key: "00".repeat(32),
        redirect_uri: "http://localhost:3000/api/v1/auth/discord/callback".into(),
    });
    let router = app::build(&config, store.clone())?;
    let cookie = format!("prosepect_session={}", session.token);
    let body = json!({"timezone":"Asia/Jakarta","consent_version":"2026-10-31"}).to_string();
    let rejected = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/integrations/discord/connect")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.clone()))?,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/integrations/discord/connect")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &session.response.csrf_token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let browser = response.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?;
    let url = reqwest::Url::parse(body["authorization_url"].as_str().unwrap())?;
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(url.host_str(), Some("discord.com"));
    assert_eq!(params["scope"], "identify");
    assert_eq!(params["code_challenge_method"], "S256");
    let callback = format!(
        "/api/v1/auth/discord/callback?state={}&error=access_denied&iss=https%3A%2F%2Fdiscord.com",
        params["state"]
    );
    let missing = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&callback)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);
    let other_profile = store.ensure_development_user(other).await?;
    let other_session = store.create_session(other_profile).await?;
    let wrong = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&callback)
                .header(
                    header::COOKIE,
                    format!("prosepect_session={}; {browser}", other_session.token),
                )
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
    let combined = format!("{cookie}; {browser}");
    let cancelled = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&callback)
                .header(header::COOKIE, &combined)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(cancelled.status(), StatusCode::SEE_OTHER);
    let repeated = router
        .oneshot(
            Request::builder()
                .uri(&callback)
                .header(header::COOKIE, &combined)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(repeated.status(), StatusCode::FORBIDDEN);
    assert!(
        store
            .discord_connection(DEVELOPMENT_USER_ID)
            .await?
            .is_none()
    );
    Ok(())
}
