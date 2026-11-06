use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use prosepect_api::{
    config::GoogleOAuthConfig,
    google_auth::GoogleOAuth,
    models::{CreateTaskRequest, TaskPriority, TaskRecurrence, TaskStatus, UpdateTaskRequest},
    store::Store,
    sync_dispatcher::SyncDispatcher,
    sync_service::SyncService,
};
use serde_json::json;
use sqlx::PgPool;
use tokio::net::TcpListener;
use uuid::Uuid;

#[derive(Clone)]
struct MockGoogleState {
    requests: Arc<AtomicUsize>,
}

#[derive(Clone, Default)]
struct MutationState {
    creates: Arc<AtomicUsize>,
    updates: Arc<AtomicUsize>,
    deletes: Arc<AtomicUsize>,
    remote_change: Arc<AtomicBool>,
    remote_delete: Arc<AtomicBool>,
}

#[sqlx::test(migrations = "../../migrations")]
async fn scheduled_task_time_blocks_follow_the_google_event_lifecycle(
    pool: PgPool,
) -> anyhow::Result<()> {
    let state = MutationState::default();
    let app = Router::new()
        .route(
            "/calendars/{calendar_id}/events",
            get(mock_empty_google_events).post(mock_create_google_event),
        )
        .route(
            "/calendars/{calendar_id}/events/{event_id}",
            put(mock_update_google_event).delete(mock_delete_google_event),
        )
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });

    let user_id = create_user(&pool, "task-lifecycle@example.com").await?;
    let calendar_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO calendars (
            id, user_id, name, color, source, external_id, selected, is_default,
            provider_primary, access_role
        ) VALUES ($1, $2, 'Primary', '#4285f4', 'google', 'remote-calendar', TRUE, FALSE,
                  TRUE, 'owner')
        "#,
    )
    .bind(calendar_id)
    .bind(user_id)
    .execute(&pool)
    .await?;
    let encryption_key = [31_u8; 32];
    sqlx::query(
        r#"
        INSERT INTO google_accounts (
            user_id, encrypted_access_token, access_token_expires_at, scopes
        ) VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(encrypt_token(&encryption_key, b"provider-access-token")?)
    .bind(Utc::now() + Duration::hours(1))
    .bind(vec!["https://www.googleapis.com/auth/calendar.events"])
    .execute(&pool)
    .await?;
    let starts_at = Utc::now() + Duration::hours(2);
    let store = Store::from_pool(pool.clone());
    let task = store
        .create_task(
            user_id,
            CreateTaskRequest {
                project_id: None,
                parent_task_id: None,
                title: "Provider time block".to_owned(),
                description: "Mirrored scheduled work".to_owned(),
                due_at: None,
                scheduled_start: Some(starts_at),
                scheduled_end: Some(starts_at + Duration::hours(1)),
                status: TaskStatus::Todo,
                priority: TaskPriority::Medium,
                recurrence: TaskRecurrence::None,
                labels: Vec::new(),
                remind_at: None,
            },
        )
        .await?;
    let google = GoogleOAuth::new(GoogleOAuthConfig {
        client_id: "test-client".to_owned(),
        client_secret: "test-secret".to_owned(),
        redirect_uri: "http://localhost/callback".to_owned(),
        token_encryption_key: STANDARD.encode(encryption_key),
    })?;
    let service =
        SyncService::new(store.clone(), google, None)?.with_api_base(format!("http://{address}"));

    assert!(service.run_once().await?);
    assert_eq!(state.creates.load(Ordering::SeqCst), 1);
    assert_eq!(state.updates.load(Ordering::SeqCst), 0);
    assert_eq!(state.deletes.load(Ordering::SeqCst), 0);

    state.remote_change.store(true, Ordering::SeqCst);
    store
        .enqueue_sync(
            user_id,
            Some(calendar_id),
            "calendar_sync",
            "provider-remote-task-update",
        )
        .await?;
    assert!(service.run_once().await?);
    let task = store
        .list_tasks(user_id, None, None, 10)
        .await?
        .items
        .into_iter()
        .find(|candidate| candidate.id == task.id)
        .expect("mirrored task");
    assert_eq!(task.title, "Remotely moved task");
    assert_eq!(task.scheduled_start, Some("2026-09-03T11:00:00Z".parse()?));
    state.remote_change.store(false, Ordering::SeqCst);

    let completed = store
        .update_task(
            user_id,
            task.id,
            update_task_request(&task, TaskStatus::Completed, true),
        )
        .await?;
    assert!(service.run_once().await?);
    assert_eq!(state.updates.load(Ordering::SeqCst), 1);
    assert_eq!(state.deletes.load(Ordering::SeqCst), 0);

    store
        .update_task(
            user_id,
            task.id,
            update_task_request(&completed, TaskStatus::Completed, false),
        )
        .await?;
    assert!(service.run_once().await?);
    assert_eq!(state.deletes.load(Ordering::SeqCst), 1);
    let mappings: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM external_event_mappings WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(mappings, 0);

    // A dirty local task and a Google deletion must require a decision first.
    let unscheduled = store
        .list_tasks(user_id, None, None, 10)
        .await?
        .items
        .remove(0);
    let mut reschedule = update_task_request(&unscheduled, TaskStatus::Completed, false);
    reschedule.scheduled_start = Some(starts_at);
    reschedule.scheduled_end = Some(starts_at + Duration::hours(1));
    let rescheduled = store.update_task(user_id, task.id, reschedule).await?;
    assert!(service.run_once().await?);
    let mut local_edit = update_task_request(&rescheduled, TaskStatus::Completed, true);
    local_edit.title = "Keep my local work".to_owned();
    store.update_task(user_id, task.id, local_edit).await?;
    state.remote_delete.store(true, Ordering::SeqCst);
    assert!(service.run_once().await?);
    let conflicts = store.list_sync_conflicts(user_id).await?.items;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].allowed_resolutions,
        ["google", "prosepect", "latest"]
    );
    let before_decision = store
        .list_tasks(user_id, None, None, 10)
        .await?
        .items
        .remove(0);
    assert!(before_decision.scheduled_start.is_some());
    let resolved = store
        .resolve_sync_conflict(user_id, conflicts[0].id, "google")
        .await?;
    assert_eq!(
        resolved.allowed_resolutions,
        ["google", "prosepect", "latest"]
    );
    assert!(service.run_once().await?);
    let after_deletion = store
        .list_tasks(user_id, None, None, 10)
        .await?
        .items
        .remove(0);
    assert_eq!(after_deletion.id, task.id);
    assert_eq!(after_deletion.status, TaskStatus::Completed);
    assert!(after_deletion.scheduled_start.is_none());
    assert!(after_deletion.scheduled_end.is_none());
    assert!(store.list_sync_conflicts(user_id).await?.items.is_empty());
    assert_eq!(state.deletes.load(Ordering::SeqCst), 1);

    server.abort();
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn synchronization_creates_a_renewable_google_watch_channel(
    pool: PgPool,
) -> anyhow::Result<()> {
    let app = Router::new().route(
        "/calendars/{calendar_id}/events/watch",
        post(mock_google_watch),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });

    let user_id = create_user(&pool, "watch-provider@example.com").await?;
    let calendar_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO calendars (
            id, user_id, name, color, source, external_id, selected, is_default,
            provider_primary, access_role
        ) VALUES ($1, $2, 'Google', '#4285f4', 'google', 'remote-calendar', TRUE, FALSE,
                  TRUE, 'owner')
        "#,
    )
    .bind(calendar_id)
    .bind(user_id)
    .execute(&pool)
    .await?;
    let encryption_key = [24_u8; 32];
    sqlx::query(
        r#"
        INSERT INTO google_accounts (
            user_id, encrypted_access_token, access_token_expires_at, scopes
        ) VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(encrypt_token(&encryption_key, b"provider-access-token")?)
    .bind(Utc::now() + Duration::hours(1))
    .bind(vec!["https://www.googleapis.com/auth/calendar.events"])
    .execute(&pool)
    .await?;
    let old_channel_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO google_watch_channels (
            channel_id, user_id, calendar_id, resource_id, token_hash, expires_at
        ) VALUES ($1, $2, $3, 'old-resource', $4, $5)
        "#,
    )
    .bind(old_channel_id)
    .bind(user_id)
    .bind(calendar_id)
    .bind(vec![1_u8; 32])
    .bind(Utc::now() + Duration::hours(12))
    .execute(&pool)
    .await?;
    let store = Store::from_pool(pool.clone());
    store
        .enqueue_sync(
            user_id,
            Some(calendar_id),
            "calendar_watch",
            "watch-provider-test",
        )
        .await?;
    let google = GoogleOAuth::new(GoogleOAuthConfig {
        client_id: "test-client".to_owned(),
        client_secret: "test-secret".to_owned(),
        redirect_uri: "http://localhost/callback".to_owned(),
        token_encryption_key: STANDARD.encode(encryption_key),
    })?;
    let service = SyncService::new(
        store.clone(),
        google,
        Some("https://api.prosepect.test/webhooks/google/calendar".to_owned()),
    )?
    .with_api_base(format!("http://{address}"));

    assert!(service.run_once().await?);
    let channels: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM google_watch_channels WHERE user_id = $1 AND calendar_id = $2",
    )
    .bind(user_id)
    .bind(calendar_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(channels, 1);
    let active_channel: Uuid = sqlx::query_scalar(
        "SELECT channel_id FROM google_watch_channels WHERE user_id = $1 AND calendar_id = $2",
    )
    .bind(user_id)
    .bind(calendar_id)
    .fetch_one(&pool)
    .await?;
    assert_ne!(active_channel, old_channel_id);
    let read_only_calendar_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO calendars (
            id, user_id, name, color, source, external_id, selected, access_role
        ) VALUES ($1, $2, 'Read only', '#4285f4', 'google', 'read-only-calendar', TRUE, 'reader')
        "#,
    )
    .bind(read_only_calendar_id)
    .bind(user_id)
    .execute(&pool)
    .await?;
    let enqueued = store
        .enqueue_expiring_calendar_watches(Some(user_id))
        .await?;
    assert_eq!(enqueued, 0);
    let pending_syncs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sync_jobs WHERE user_id = $1 AND calendar_id = $2 AND kind = 'calendar_sync' AND status = 'pending'",
    )
    .bind(user_id)
    .bind(calendar_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(pending_syncs, 1);

    server.abort();
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn synchronization_recovers_an_expired_token_and_imports_a_remote_event(
    pool: PgPool,
) -> anyhow::Result<()> {
    let state = MockGoogleState {
        requests: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/calendars/{calendar_id}/events", get(mock_google_events))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });

    let user_id = create_user(&pool, "sync-provider@example.com").await?;
    let calendar_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO calendars (
            id, user_id, name, color, source, external_id, selected, is_default, sync_token
        ) VALUES ($1, $2, 'Google', '#4285f4', 'google', 'remote-calendar', TRUE, FALSE, 'expired-token')
        "#,
    )
    .bind(calendar_id)
    .bind(user_id)
    .execute(&pool)
    .await?;

    let encryption_key = [42_u8; 32];
    sqlx::query(
        r#"
        INSERT INTO google_accounts (
            user_id, encrypted_access_token, access_token_expires_at, scopes
        ) VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(encrypt_token(&encryption_key, b"provider-access-token")?)
    .bind(Utc::now() + Duration::hours(1))
    .bind(vec!["https://www.googleapis.com/auth/calendar.events"])
    .execute(&pool)
    .await?;

    let store = Store::from_pool(pool.clone());
    store
        .enqueue_sync(
            user_id,
            Some(calendar_id),
            "calendar_sync",
            "provider-recovery-test",
        )
        .await?;
    let google = GoogleOAuth::new(GoogleOAuthConfig {
        client_id: "test-client".to_owned(),
        client_secret: "test-secret".to_owned(),
        redirect_uri: "http://localhost/callback".to_owned(),
        token_encryption_key: STANDARD.encode(encryption_key),
    })?;
    let service = SyncService::new(store, google, None)?.with_api_base(format!("http://{address}"));

    assert!(service.run_once().await?);

    let imported: (String, String) = sqlx::query_as(
        "SELECT title, location FROM calendar_events WHERE user_id = $1 AND calendar_id = $2",
    )
    .bind(user_id)
    .bind(calendar_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(imported.0, "Provider planning session");
    assert_eq!(imported.1, "Remote room");
    let sync_token: Option<String> =
        sqlx::query_scalar("SELECT sync_token FROM calendars WHERE id = $1")
            .bind(calendar_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(sync_token.as_deref(), Some("replacement-token"));
    assert_eq!(state.requests.load(Ordering::SeqCst), 2);
    let completed_jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sync_jobs WHERE user_id = $1 AND status = 'succeeded'",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(completed_jobs, 1);

    server.abort();
    Ok(())
}

async fn mock_empty_google_events(
    State(state): State<MutationState>,
    headers: HeaderMap,
) -> Response {
    if !has_provider_access_token(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if state.remote_delete.load(Ordering::SeqCst) {
        return Json(json!({
            "items": [{"id": "provider-task-event", "status": "cancelled", "etag": "deleted"}],
            "nextSyncToken": "provider-token-after-deletion"
        }))
        .into_response();
    }
    if state.remote_change.load(Ordering::SeqCst) {
        return Json(json!({
            "items": [{
                "id": "provider-task-event",
                "etag": "etag-remote-update",
                "status": "confirmed",
                "summary": "Remotely moved task",
                "description": "Changed through Google Calendar",
                "start": { "dateTime": "2026-09-03T11:00:00Z", "timeZone": "UTC" },
                "end": { "dateTime": "2026-09-03T12:00:00Z", "timeZone": "UTC" },
                "updated": "2026-09-03T10:30:00Z"
            }],
            "nextSyncToken": "provider-token-after-update"
        }))
        .into_response();
    }
    Json(json!({ "items": [], "nextSyncToken": "provider-token" })).into_response()
}

async fn mock_create_google_event(
    State(state): State<MutationState>,
    headers: HeaderMap,
    Json(mut event): Json<serde_json::Value>,
) -> Response {
    if !has_provider_access_token(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.creates.fetch_add(1, Ordering::SeqCst);
    event["id"] = json!("provider-task-event");
    event["etag"] = json!("etag-created");
    event["status"] = json!("confirmed");
    event["updated"] = json!(Utc::now());
    Json(event).into_response()
}

async fn mock_update_google_event(
    State(state): State<MutationState>,
    headers: HeaderMap,
    Json(mut event): Json<serde_json::Value>,
) -> Response {
    if !has_provider_access_token(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.updates.fetch_add(1, Ordering::SeqCst);
    event["id"] = json!("provider-task-event");
    event["etag"] = json!("etag-updated");
    event["status"] = json!("confirmed");
    event["updated"] = json!(Utc::now());
    Json(event).into_response()
}

async fn mock_delete_google_event(
    State(state): State<MutationState>,
    headers: HeaderMap,
) -> Response {
    if !has_provider_access_token(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.deletes.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT.into_response()
}

fn has_provider_access_token(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some("Bearer provider-access-token")
}

async fn mock_google_watch(headers: HeaderMap) -> Response {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer provider-access-token")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({
        "id": Uuid::new_v4().to_string(),
        "resourceId": "watch-resource-1",
        "expiration": (Utc::now() + Duration::days(6)).timestamp_millis().to_string()
    }))
    .into_response()
}

async fn mock_google_events(
    State(state): State<MockGoogleState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer provider-access-token")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.requests.fetch_add(1, Ordering::SeqCst);
    if query.contains_key("syncToken") {
        return StatusCode::GONE.into_response();
    }
    Json(json!({
        "items": [{
            "id": "remote-event-1",
            "etag": "etag-1",
            "status": "confirmed",
            "summary": "Provider planning session",
            "description": "Imported through the provider mock",
            "start": { "dateTime": "2026-09-01T09:00:00Z", "timeZone": "UTC" },
            "end": { "dateTime": "2026-09-01T10:00:00Z", "timeZone": "UTC" },
            "location": "Remote room",
            "attendees": [{ "email": "person@example.com" }],
            "updated": "2026-08-31T12:00:00Z"
        }],
        "nextSyncToken": "replacement-token"
    }))
    .into_response()
}

fn update_task_request(
    task: &prosepect_api::models::Task,
    status: TaskStatus,
    keep_schedule: bool,
) -> UpdateTaskRequest {
    UpdateTaskRequest {
        project_id: task.project_id,
        parent_task_id: task.parent_task_id,
        title: task.title.clone(),
        description: task.description.clone(),
        due_at: task.due_at,
        scheduled_start: keep_schedule.then_some(task.scheduled_start).flatten(),
        scheduled_end: keep_schedule.then_some(task.scheduled_end).flatten(),
        status,
        priority: task.priority,
        recurrence: task.recurrence,
        labels: task.labels.clone(),
        remind_at: task.remind_at,
        expected_version: task.version,
    }
}

fn encrypt_token(key: &[u8; 32], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let nonce_bytes = [7_u8; 12];
    let encrypted = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|_| anyhow::anyhow!("test token encryption failed"))?;
    let mut value = nonce_bytes.to_vec();
    value.extend(encrypted);
    Ok(value)
}

async fn create_user(pool: &PgPool, email: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, email, display_name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(email)
        .bind(email)
        .execute(pool)
        .await?;
    Ok(id)
}

struct DispatcherFixture {
    service: SyncService,
    store: Store,
    user_id: Uuid,
    calendar_id: Uuid,
    requests: Arc<AtomicUsize>,
    first_response: Arc<tokio::sync::Semaphore>,
    server: tokio::task::JoinHandle<()>,
}

impl DispatcherFixture {
    fn storage_root(&self) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rakkio-dispatcher-{}", self.user_id))
    }
    fn start(&self) -> anyhow::Result<SyncDispatcher> {
        let storage = prosepect_api::file_storage::FileStorage::new(
            &prosepect_api::config::ObjectStorageConfig::Local {
                root: self.storage_root().to_string_lossy().into_owned(),
            },
        )?;
        Ok(SyncDispatcher::start(self.service.clone(), storage))
    }
}
impl Drop for DispatcherFixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(self.storage_root());
    }
}

async fn dispatcher_fixture(pool: &PgPool, failures: usize) -> anyhow::Result<DispatcherFixture> {
    let requests = Arc::new(AtomicUsize::new(0));
    let calls = requests.clone();
    let first_response = Arc::new(tokio::sync::Semaphore::new(1));
    let response_gate = first_response.clone();
    let app = Router::new().route(
        "/calendars/{calendar_id}/events",
        get(move || {
            let calls = calls.clone();
            let response_gate = response_gate.clone();
            async move {
                let attempt = calls.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    // Tests can hold the first response without sleeping or real provider traffic.
                    let _permit = response_gate.acquire().await.unwrap();
                }
                if attempt < failures {
                    // Non-5xx failure: test the durable job retry, not HTTP-level retries.
                    StatusCode::BAD_REQUEST.into_response()
                } else {
                    Json(json!({ "items": [], "nextSyncToken": "dispatcher-token" }))
                        .into_response()
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let user_id = create_user(pool, "dispatcher@example.com").await?;
    let calendar_id = Uuid::now_v7();
    // Keep startup periodic enqueue out of the scenario: only the seeded jobs should run.
    sqlx::query(
        "INSERT INTO calendars (id, user_id, name, color, source, external_id, selected, last_synced_at) VALUES ($1, $2, 'Google', '#4285f4', 'google', 'dispatcher-calendar', TRUE, NOW())",
    )
    .bind(calendar_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    let encryption_key = [43_u8; 32];
    sqlx::query(
        "INSERT INTO google_accounts (user_id, encrypted_access_token, access_token_expires_at, scopes) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(encrypt_token(&encryption_key, b"provider-access-token")?)
    .bind(Utc::now() + Duration::hours(1))
    .bind(vec!["https://www.googleapis.com/auth/calendar.events"])
    .execute(pool)
    .await?;
    let store = Store::from_pool(pool.clone());
    let google = GoogleOAuth::new(GoogleOAuthConfig {
        client_id: "test-client".to_owned(),
        client_secret: "test-secret".to_owned(),
        redirect_uri: "http://localhost/callback".to_owned(),
        token_encryption_key: STANDARD.encode(encryption_key),
    })?;
    let service =
        SyncService::new(store.clone(), google, None)?.with_api_base(format!("http://{address}"));
    Ok(DispatcherFixture {
        service,
        store,
        user_id,
        calendar_id,
        requests,
        first_response,
        server,
    })
}

async fn dispatcher_job(fixture: &DispatcherFixture, key: &str) -> anyhow::Result<Uuid> {
    Ok(fixture
        .store
        .enqueue_sync(
            fixture.user_id,
            Some(fixture.calendar_id),
            "calendar_sync",
            key,
        )
        .await?
        .id)
}

async fn wait_for_dispatcher_job(pool: &PgPool, id: Uuid, status: &str) -> anyhow::Result<i32> {
    // PostgreSQL uses wall time. Do not pause Tokio time or advance it independently of SQL.
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(25));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            poll.tick().await;
            let row: (String, i32) =
                sqlx::query_as("SELECT status, attempt_count FROM sync_jobs WHERE id = $1")
                    .bind(id)
                    .fetch_one(pool)
                    .await?;
            if row.0 == status {
                return Ok::<_, anyhow::Error>(row.1);
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("dispatcher did not reach {status} for {id} without a wake"))?
}

#[sqlx::test(migrations = "../../migrations")]
async fn dispatcher_retries_future_failure_without_manual_wake(pool: PgPool) -> anyhow::Result<()> {
    let fixture = dispatcher_fixture(&pool, 1).await?;
    let job = dispatcher_job(&fixture, "fail-once").await?;
    let dispatcher = fixture.start()?;
    assert_eq!(wait_for_dispatcher_job(&pool, job, "failed").await?, 1);
    let future: bool =
        sqlx::query_scalar("SELECT available_at > NOW() FROM sync_jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&pool)
            .await?;
    assert!(future, "the first failure must schedule a future retry");
    // No dispatcher.wake(), run_once(), frontend request, or SQL retry-time rewrite.
    assert_eq!(wait_for_dispatcher_job(&pool, job, "succeeded").await?, 2);
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    drop(dispatcher);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn dispatcher_startup_drains_due_retries_but_never_resurrects_terminal_jobs(
    pool: PgPool,
) -> anyhow::Result<()> {
    let fixture = dispatcher_fixture(&pool, 0).await?;
    let due = dispatcher_job(&fixture, "due-on-restart").await?;
    let terminal = dispatcher_job(&fixture, "exhausted").await?;
    let succeeded = dispatcher_job(&fixture, "already-succeeded").await?;
    let later = dispatcher_job(&fixture, "later-without-wake").await?;
    sqlx::query("UPDATE sync_jobs SET status = 'failed', attempt_count = 7 WHERE id = $1")
        .bind(due)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE sync_jobs SET status = 'failed', attempt_count = 8 WHERE id = $1")
        .bind(terminal)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE sync_jobs SET status = 'succeeded', attempt_count = 1 WHERE id = $1")
        .bind(succeeded)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE sync_jobs SET available_at = NOW() + INTERVAL '2 seconds' WHERE id = $1")
        .bind(later)
        .execute(&pool)
        .await?;
    let dispatcher = fixture.start()?;
    assert_eq!(wait_for_dispatcher_job(&pool, due, "succeeded").await?, 8);
    // A later timer cycle must neither replay the successful eighth attempt nor revive failures.
    assert_eq!(wait_for_dispatcher_job(&pool, later, "succeeded").await?, 1);
    assert_eq!(wait_for_dispatcher_job(&pool, terminal, "failed").await?, 8);
    assert_eq!(
        wait_for_dispatcher_job(&pool, succeeded, "succeeded").await?,
        1
    );
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    drop(dispatcher);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn dispatcher_final_failure_does_not_promise_another_retry(
    pool: PgPool,
) -> anyhow::Result<()> {
    let fixture = dispatcher_fixture(&pool, usize::MAX).await?;
    let job = dispatcher_job(&fixture, "final-failure").await?;
    sqlx::query("UPDATE sync_jobs SET status = 'failed', attempt_count = 7 WHERE id = $1")
        .bind(job)
        .execute(&pool)
        .await?;
    let dispatcher = fixture.start()?;
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(25));
        loop {
            poll.tick().await;
            let message: Option<String> = sqlx::query_scalar("SELECT message FROM activity_entries WHERE user_id = $1 AND kind = 'synchronization_failed'")
                .bind(fixture.user_id).fetch_optional(&pool).await?;
            if let Some(message) = message {
                assert!(!message.contains("will be retried"), "terminal failure must not promise an automatic retry");
                return Ok::<_, anyhow::Error>(());
            }
        }
    }).await??;
    assert_eq!(wait_for_dispatcher_job(&pool, job, "failed").await?, 8);
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    drop(dispatcher);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn dispatcher_waits_after_database_error_then_recovers_without_wake(
    pool: PgPool,
) -> anyhow::Result<()> {
    let fixture = dispatcher_fixture(&pool, 0).await?;
    let job = dispatcher_job(&fixture, "claim-error").await?;
    // Sequence increments survive rollback: exactly the first claim fails in this disposable DB.
    sqlx::raw_sql(
        "CREATE SEQUENCE dispatcher_claim_attempts;
         CREATE FUNCTION fail_first_dispatcher_claim() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.status = 'running' AND nextval('dispatcher_claim_attempts') = 1 THEN
                 RAISE EXCEPTION 'test transient claim error';
             END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER dispatcher_claim_error BEFORE UPDATE ON sync_jobs
         FOR EACH ROW EXECUTE FUNCTION fail_first_dispatcher_claim();",
    )
    .execute(&pool)
    .await?;
    let started = std::time::Instant::now();
    let dispatcher = fixture.start()?;
    assert_eq!(wait_for_dispatcher_job(&pool, job, "succeeded").await?, 1);
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(5),
        "errors must wait before polling again"
    );
    let claims: i64 = sqlx::query_scalar("SELECT last_value FROM dispatcher_claim_attempts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(claims, 2);
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    drop(dispatcher);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn dispatcher_shutdown_finishes_current_job_without_starting_another(
    pool: PgPool,
) -> anyhow::Result<()> {
    let fixture = dispatcher_fixture(&pool, 0).await?;
    let first = dispatcher_job(&fixture, "shutdown-first").await?;
    let second = dispatcher_job(&fixture, "shutdown-second").await?;
    let blocked_response = fixture.first_response.acquire().await?;
    let dispatcher = fixture.start()?;
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(25));
        while fixture.requests.load(Ordering::SeqCst) == 0 {
            poll.tick().await;
        }
    })
    .await?;
    // Queue many coalesced wake signals while a real run_once is blocked in mock HTTP.
    for _ in 0..100 {
        dispatcher.wake();
    }
    drop(dispatcher);
    drop(blocked_response);
    assert_eq!(wait_for_dispatcher_job(&pool, first, "succeeded").await?, 1);
    // Observe an entire retry-poll window. Shutdown must suppress both buffered wakes and timers.
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(6), async {
            let mut poll = tokio::time::interval(std::time::Duration::from_millis(25));
            while fixture.requests.load(Ordering::SeqCst) == 1 {
                poll.tick().await;
            }
        })
        .await
        .is_err()
    );
    assert_eq!(wait_for_dispatcher_job(&pool, second, "pending").await?, 0);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn dispatcher_coalesces_wakes_and_bounds_each_drain(pool: PgPool) -> anyhow::Result<()> {
    let fixture = dispatcher_fixture(&pool, 0).await?;
    let mut jobs = Vec::new();
    for index in 0..33 {
        jobs.push(dispatcher_job(&fixture, &format!("bounded-drain-{index}")).await?);
    }
    // Block startup cleanup so the receiver cannot consume a signal during this burst.
    let mut startup_lock = pool.begin().await?;
    sqlx::query("LOCK TABLE oauth_login_attempts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *startup_lock)
        .await?;
    let dispatcher = fixture.start()?;
    for _ in 0..100 {
        dispatcher.wake();
    }
    startup_lock.commit().await?;
    assert_eq!(
        wait_for_dispatcher_job(&pool, jobs[32], "succeeded").await?,
        1
    );
    let waited_between_batches: bool = sqlx::query_scalar(
        "SELECT later.updated_at >= earlier.updated_at + INTERVAL '5 seconds'
         FROM sync_jobs earlier, sync_jobs later WHERE earlier.id = $1 AND later.id = $2",
    )
    .bind(jobs[31])
    .bind(jobs[32])
    .fetch_one(&pool)
    .await?;
    assert!(waited_between_batches, "a drain must yield after 32 jobs");
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 33);
    drop(dispatcher);
    Ok(())
}

// Deterministic fake provider for reversible deletion. No live Google calls.
#[derive(Clone, Default)]
struct UndoGoogle {
    remote: Arc<tokio::sync::Mutex<serde_json::Value>>,
    writes: Arc<tokio::sync::Mutex<Vec<String>>>,
    get_status: Arc<AtomicUsize>,
    empty_list: Arc<AtomicBool>,
    paginated: Arc<AtomicBool>,
    gone_once: Arc<AtomicBool>,
    pause_get: Arc<AtomicBool>,
    started: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    race_delete: Arc<AtomicBool>,
}

async fn undo_google_list(
    State(state): State<UndoGoogle>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if query.contains_key("syncToken") && state.gone_once.swap(false, Ordering::SeqCst) {
        return StatusCode::GONE.into_response();
    }
    if state.paginated.load(Ordering::SeqCst) && !query.contains_key("pageToken") {
        return Json(json!({"items":[],"nextPageToken":"second-page"})).into_response();
    }
    let remote = state.remote.lock().await.clone();
    let items = if remote.is_null() || state.empty_list.load(Ordering::SeqCst) {
        vec![]
    } else {
        vec![remote]
    };
    Json(json!({"items":items,"nextSyncToken":"after-pull"})).into_response()
}
async fn undo_google_get(State(state): State<UndoGoogle>) -> Response {
    // Snapshot the response before pausing so tests can change provider/database state
    // between the observed GET and final transactional revalidation.
    let remote = state.remote.lock().await.clone();
    if state.pause_get.load(Ordering::SeqCst) {
        state.started.notify_one();
        state.resume.notified().await;
    }
    let status = state.get_status.load(Ordering::SeqCst);
    if status != 0 {
        return StatusCode::from_u16(status as u16).unwrap().into_response();
    }
    if remote.is_null() {
        StatusCode::NOT_FOUND.into_response()
    } else {
        Json(remote).into_response()
    }
}
async fn undo_google_create(
    State(state): State<UndoGoogle>,
    Json(mut body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.writes.lock().await.push("POST".into());
    body["etag"] = json!("clean-etag");
    body["status"] = json!("confirmed");
    *state.remote.lock().await = body.clone();
    Json(body)
}
async fn undo_google_update(State(state): State<UndoGoogle>) -> StatusCode {
    state.writes.lock().await.push("PUT".into());
    StatusCode::PRECONDITION_FAILED
}
async fn undo_google_delete(State(state): State<UndoGoogle>, headers: HeaderMap) -> StatusCode {
    state.writes.lock().await.push("DELETE".into());
    let mut remote = state.remote.lock().await;
    if state.race_delete.load(Ordering::SeqCst) {
        remote["etag"] = json!("raced-etag");
    }
    if headers.get("if-match").and_then(|h| h.to_str().ok()) != remote["etag"].as_str() {
        return StatusCode::PRECONDITION_FAILED;
    }
    *remote = serde_json::Value::Null;
    StatusCode::NO_CONTENT
}
async fn undo_google_discovery() -> Json<serde_json::Value> {
    Json(
        json!({"items":[{"id":"undo-calendar","summary":"Undo calendar","primary":true,"selected":true,"accessRole":"owner"}]}),
    )
}

struct UndoFixture {
    store: Store,
    service: SyncService,
    user: Uuid,
    calendar: Uuid,
    task: prosepect_api::models::Task,
    state: UndoGoogle,
    api_base: String,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Drop for UndoFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn undo_fixture(pool: &PgPool) -> anyhow::Result<UndoFixture> {
    let state = UndoGoogle::default();
    let app = Router::new()
        .route("/users/me/calendarList", get(undo_google_discovery))
        .route(
            "/calendars/{calendar}/events",
            get(undo_google_list).post(undo_google_create),
        )
        .route(
            "/calendars/{calendar}/events/{event}",
            get(undo_google_get)
                .put(undo_google_update)
                .delete(undo_google_delete),
        )
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let user = create_user(pool, "undo-provider@example.test").await?;
    let calendar = Uuid::now_v7();
    sqlx::query("INSERT INTO calendars(id,user_id,name,source,external_id,selected,provider_primary,access_role) VALUES ($1,$2,'Undo calendar','google','undo-calendar',TRUE,TRUE,'owner')")
        .bind(calendar).bind(user).execute(pool).await?;
    let key = [41_u8; 32];
    sqlx::query("INSERT INTO google_accounts(user_id,encrypted_access_token,access_token_expires_at,scopes) VALUES ($1,$2,$3,$4)")
        .bind(user).bind(encrypt_token(&key,b"fake-token")?).bind(Utc::now()+Duration::hours(1))
        .bind(vec!["https://www.googleapis.com/auth/calendar.events"]).execute(pool).await?;
    let store = Store::from_pool(pool.clone());
    let task=store.create_task(user,serde_json::from_value(json!({"title":"Clean mapped task","description":"Undo content","scheduled_start":"2026-09-10T10:00:00Z","scheduled_end":"2026-09-10T11:00:00Z"}))?).await?;
    let google = GoogleOAuth::new(GoogleOAuthConfig {
        client_id: "fake".into(),
        client_secret: "fake".into(),
        redirect_uri: "http://localhost/callback".into(),
        token_encryption_key: STANDARD.encode(key),
    })?;
    let service =
        SyncService::new(store.clone(), google, None)?.with_api_base(format!("http://{address}"));
    Ok(UndoFixture {
        store,
        service,
        user,
        calendar,
        task,
        state,
        api_base: format!("http://{address}"),
        server,
    })
}
async fn undo_initial_push(f: &UndoFixture) -> anyhow::Result<()> {
    assert!(f.service.run_once().await?);
    assert_eq!(*f.state.writes.lock().await, vec!["POST"]);
    f.state.writes.lock().await.clear();
    Ok(())
}
async fn undo_sync(f: &UndoFixture, key: &str) -> anyhow::Result<()> {
    f.store
        .enqueue_sync(f.user, Some(f.calendar), "calendar_sync", key)
        .await?;
    assert!(f.service.run_once().await?);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_hold_restore_expiry_and_discovery_are_write_free(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    undo_initial_push(&f).await?;
    let event: Uuid = sqlx::query_scalar("SELECT id FROM calendar_events WHERE linked_task_id=$1")
        .bind(f.task.id)
        .fetch_one(&pool)
        .await?;
    let mapping: Uuid =
        sqlx::query_scalar("SELECT id FROM external_event_mappings WHERE canonical_event_id=$1")
            .bind(event)
            .fetch_one(&pool)
            .await?;
    let receipt = f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
    let expiry_job: (chrono::DateTime<Utc>, String) =
        sqlx::query_as("SELECT available_at,status FROM sync_jobs WHERE idempotency_key=$1")
            .bind(format!("task-delete-expiry:{}", receipt.id))
            .fetch_one(&pool)
            .await?;
    assert_eq!(expiry_job.0, receipt.expires_at);
    assert_eq!(expiry_job.1, "pending");
    for key in ["manual-hold", "webhook-hold", "unrelated-hold"] {
        undo_sync(&f, key).await?;
    }
    assert!(f.state.writes.lock().await.is_empty());
    f.store
        .undo_task_delete(f.user, receipt.id, Some(&f.service))
        .await?;
    let restored:(Uuid,bool,bool,bool)=sqlx::query_as("SELECT canonical_event_id,local_dirty,local_deleted,reversible_tombstone FROM external_event_mappings WHERE id=$1").bind(mapping).fetch_one(&pool).await?;
    assert_eq!(restored, (event, false, false, false));
    sqlx::query("UPDATE sync_jobs SET available_at=clock_timestamp() WHERE idempotency_key=$1")
        .bind(format!("task-delete-expiry:{}", receipt.id))
        .execute(&pool)
        .await?;
    assert!(f.service.run_once().await?);
    f.store
        .enqueue_sync(f.user, None, "calendar_discovery", "discover-after-undo")
        .await?;
    assert!(f.service.run_once().await?);
    undo_sync(&f, "sync-after-discovery").await?;
    assert!(f.state.writes.lock().await.is_empty());
    // A subsequent remote cancellation of the CLEAN restore never becomes POST.
    {
        let mut remote = f.state.remote.lock().await;
        remote["status"] = json!("cancelled");
        remote["etag"] = json!("cancelled-etag");
    }
    undo_sync(&f, "cancel-clean-restore").await?;
    assert!(f.state.writes.lock().await.is_empty());
    assert!(
        f.store.list_tasks(f.user, None, None, 10).await?.items[0]
            .scheduled_start
            .is_none()
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_refuses_uncertain_dirty_nonpreferred_and_incomplete_baselines(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    assert!(matches!(
        f.store.delete_task_with_undo(f.user, f.task.id, 1).await,
        Err(prosepect_api::error::AppError::Conflict(_))
    ));
    undo_initial_push(&f).await?;
    for update in [
        "UPDATE external_event_mappings SET local_dirty=TRUE WHERE user_id=$1",
        "UPDATE external_event_mappings SET external_etag=NULL WHERE user_id=$1",
        "UPDATE external_event_mappings SET base_fingerprint=NULL WHERE user_id=$1",
        "UPDATE external_event_mappings SET base_fingerprint='incomplete' WHERE user_id=$1",
        "UPDATE external_event_mappings SET pending_resolution='google' WHERE user_id=$1",
        "UPDATE external_event_mappings SET conflict_state='unresolved' WHERE user_id=$1",
    ] {
        let mut tx = pool.begin().await?;
        // Apply and commit each mutation, then restore the complete exact baseline.
        let before: serde_json::Value = sqlx::query_scalar(
            "SELECT to_jsonb(m) FROM external_event_mappings m WHERE user_id=$1",
        )
        .bind(f.user)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(update).bind(f.user).execute(&mut *tx).await?;
        tx.commit().await?;
        assert!(
            matches!(
                f.store.delete_task_with_undo(f.user, f.task.id, 1).await,
                Err(prosepect_api::error::AppError::Conflict(_))
            ),
            "{update}"
        );
        sqlx::query("DELETE FROM external_event_mappings WHERE user_id=$1")
            .bind(f.user)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO external_event_mappings SELECT * FROM jsonb_populate_record(NULL::external_event_mappings,$1)").bind(before).execute(&pool).await?;
    }
    sqlx::query("UPDATE calendars SET provider_primary=FALSE WHERE id=$1")
        .bind(f.calendar)
        .execute(&pool)
        .await?;
    assert!(matches!(
        f.store.delete_task_with_undo(f.user, f.task.id, 1).await,
        Err(prosepect_api::error::AppError::Conflict(_))
    ));
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 10)
            .await?
            .items
            .len(),
        1
    );
    assert!(f.state.writes.lock().await.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_pull_matrix_never_automatically_resurrects(
    pool: PgPool,
) -> anyhow::Result<()> {
    for policy in ["ask", "google", "latest", "prosepect"] {
        for cancelled in [false, true] {
            for expired in [false, true] {
                let f = undo_fixture(&pool).await?;
                undo_initial_push(&f).await?;
                sqlx::query("INSERT INTO user_settings(user_id,sync_conflict_policy) VALUES ($1,$2) ON CONFLICT(user_id) DO UPDATE SET sync_conflict_policy=$2").bind(f.user).bind(policy).execute(&pool).await?;
                let receipt = f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
                if expired {
                    sqlx::query(
                        "UPDATE task_delete_undos SET expires_at=clock_timestamp() WHERE id=$1",
                    )
                    .bind(receipt.id)
                    .execute(&pool)
                    .await?;
                    sqlx::query("UPDATE external_event_mappings SET deletion_hold_until=clock_timestamp() WHERE user_id=$1").bind(f.user).execute(&pool).await?;
                }
                {
                    let mut remote = f.state.remote.lock().await;
                    remote["etag"] = json!("changed-etag");
                    remote["summary"] = json!("Changed remotely");
                    if cancelled {
                        remote["status"] = json!("cancelled");
                    }
                }
                f.state.paginated.store(true, Ordering::SeqCst);
                f.state.gone_once.store(true, Ordering::SeqCst);
                undo_sync(&f, &format!("matrix-{policy}-{cancelled}-{expired}")).await?;
                assert!(f.state.writes.lock().await.is_empty());
                let events: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM calendar_events WHERE user_id=$1")
                        .bind(f.user)
                        .fetch_one(&pool)
                        .await?;
                assert_eq!(events, 0, "{policy}/{cancelled}/{expired}");
                assert!(
                    f.store
                        .undo_task_delete(f.user, receipt.id, Some(&f.service))
                        .await
                        .is_err()
                );
                let conflicts = f.store.list_sync_conflicts(f.user).await?.items;
                assert_eq!(conflicts.len(), usize::from(!cancelled));
                if !cancelled {
                    let etag: String = sqlx::query_scalar(
                        "SELECT external_etag FROM external_event_mappings WHERE user_id=$1",
                    )
                    .bind(f.user)
                    .fetch_one(&pool)
                    .await?;
                    assert_eq!(etag, "clean-etag");
                }
            }
        }
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_fresh_validation_fails_closed_without_consumption(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    undo_initial_push(&f).await?;
    let receipt = f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
    for status in [404, 410, 401, 503] {
        f.state.get_status.store(status, Ordering::SeqCst);
        assert!(
            f.store
                .undo_task_delete(f.user, receipt.id, Some(&f.service))
                .await
                .is_err()
        );
        assert_eq!(
            f.store.list_task_delete_undos(f.user).await?.items[0].expires_at,
            receipt.expires_at
        );
    }
    f.state.get_status.store(0, Ordering::SeqCst);
    for field in ["etag", "status", "id", "description"] {
        let before = f.state.remote.lock().await.clone();
        f.state.remote.lock().await[field] = json!(if field == "status" {
            "cancelled"
        } else {
            "changed"
        });
        assert!(
            f.store
                .undo_task_delete(f.user, receipt.id, Some(&f.service))
                .await
                .is_err(),
            "{field}"
        );
        *f.state.remote.lock().await = before;
    }
    f.state.pause_get.store(true, Ordering::SeqCst);
    let timed = f
        .store
        .undo_task_delete(f.user, receipt.id, Some(&f.service))
        .await;
    assert!(matches!(
        timed,
        Err(prosepect_api::error::AppError::InvalidRequest {
            status: StatusCode::SERVICE_UNAVAILABLE,
            ..
        })
    ));
    f.state.pause_get.store(false, Ordering::SeqCst);
    f.state.resume.notify_one();
    assert_eq!(
        f.store.list_task_delete_undos(f.user).await?.items[0].expires_at,
        receipt.expires_at
    );
    assert!(f.state.writes.lock().await.is_empty());
    f.store
        .undo_task_delete(f.user, receipt.id, Some(&f.service))
        .await?;
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_preflight_releases_single_pool_and_revalidates_preferences(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    undo_initial_push(&f).await?;
    let receipt = f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
    let single = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await?;
    let store = Store::from_pool(single.clone());
    let google = GoogleOAuth::new(GoogleOAuthConfig {
        client_id: "fake".into(),
        client_secret: "fake".into(),
        redirect_uri: "http://localhost/callback".into(),
        token_encryption_key: STANDARD.encode([41_u8; 32]),
    })?;
    let service = SyncService::new(store.clone(), google, None)?.with_api_base(f.api_base.clone());
    f.state.pause_get.store(true, Ordering::SeqCst);
    let attempt = store.undo_task_delete(f.user, receipt.id, Some(&service));
    let change = async {
        f.state.started.notified().await;
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query("UPDATE calendars SET selected=FALSE WHERE id=$1")
                .bind(f.calendar)
                .execute(&single),
        )
        .await??;
        f.state.resume.notify_one();
        anyhow::Ok(())
    };
    let (result, changed) = tokio::join!(attempt, change);
    changed?;
    assert!(matches!(
        result,
        Err(prosepect_api::error::AppError::Conflict(_))
    ));
    assert_eq!(f.store.list_task_delete_undos(f.user).await?.items.len(), 1);
    assert!(f.state.writes.lock().await.is_empty());
    single.close().await;
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_persistence_gap_is_not_treated_as_never_pushed(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    sqlx::raw_sql("CREATE FUNCTION fail_mapping() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected persistence gap'; END $$; CREATE TRIGGER fail_mapping BEFORE INSERT ON external_event_mappings FOR EACH ROW EXECUTE FUNCTION fail_mapping();").execute(&pool).await?;
    assert!(f.service.run_once().await?);
    assert!(!f.state.remote.lock().await.is_null());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM external_event_mappings WHERE user_id=$1")
            .bind(f.user)
            .fetch_one(&pool)
            .await?;
    assert_eq!(count, 0);
    assert!(matches!(
        f.store.delete_task_with_undo(f.user, f.task.id, 1).await,
        Err(prosepect_api::error::AppError::Conflict(_))
    ));
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 10)
            .await?
            .items
            .len(),
        1
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_sync_completion_releases_exclusion_before_return(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    // Reserve an independent connection: reusing the sync connection would flush
    // its queued rollback and mask a lock still held after run_once returns.
    let mut observer = pool.begin().await?;
    undo_initial_push(&f).await?;
    let released: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("prosepect-sync:{}", f.user))
            .fetch_one(&mut *observer)
            .await?;
    assert!(
        released,
        "completed sync must release user exclusion before returning"
    );
    observer.rollback().await?;
    f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_expiry_is_conditional_and_legacy_delete_is_immediate(
    pool: PgPool,
) -> anyhow::Result<()> {
    for race in [false, true] {
        let f = undo_fixture(&pool).await?;
        undo_initial_push(&f).await?;
        let receipt = f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
        sqlx::query("UPDATE task_delete_undos SET expires_at=clock_timestamp() WHERE id=$1")
            .bind(receipt.id)
            .execute(&pool)
            .await?;
        sqlx::query("UPDATE external_event_mappings SET deletion_hold_until=clock_timestamp() WHERE user_id=$1").bind(f.user).execute(&pool).await?;
        sqlx::query("UPDATE sync_jobs SET available_at=clock_timestamp() WHERE idempotency_key=$1")
            .bind(format!("task-delete-expiry:{}", receipt.id))
            .execute(&pool)
            .await?;
        f.state.race_delete.store(race, Ordering::SeqCst);
        assert!(f.service.run_once().await?);
        assert_eq!(*f.state.writes.lock().await, vec!["DELETE"]);
        assert_eq!(f.state.remote.lock().await.is_null(), !race);
        assert!(
            f.store
                .list_task_delete_undos(f.user)
                .await?
                .items
                .is_empty()
        );
    }
    let f = undo_fixture(&pool).await?;
    undo_initial_push(&f).await?;
    f.store.delete_task(f.user, f.task.id, 1).await?;
    assert!(f.service.run_once().await?);
    assert_eq!(*f.state.writes.lock().await, vec!["DELETE"]);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_explicit_keep_google_disarms_deletion_and_reconciles(
    pool: PgPool,
) -> anyhow::Result<()> {
    for remote in ["live", "cancelled", "missing", "outage"] {
        let f = undo_fixture(&pool).await?;
        undo_initial_push(&f).await?;
        let receipt = f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
        {
            let mut event = f.state.remote.lock().await;
            event["etag"] = json!("changed-etag");
            event["summary"] = json!("Keep Google");
        }
        undo_sync(&f, &format!("conflict-{remote}")).await?;
        let conflict = f.store.list_sync_conflicts(f.user).await?.items.remove(0);
        assert_eq!(conflict.allowed_resolutions, ["google"]);
        let before: serde_json::Value = sqlx::query_scalar(
            "SELECT to_jsonb(m) FROM external_event_mappings m WHERE user_id=$1",
        )
        .bind(f.user)
        .fetch_one(&pool)
        .await?;
        for refused in ["prosepect", "latest"] {
            assert!(matches!(
                f.store
                    .resolve_sync_conflict(f.user, conflict.id, refused)
                    .await,
                Err(prosepect_api::error::AppError::Conflict(_))
            ));
            let after: serde_json::Value = sqlx::query_scalar(
                "SELECT to_jsonb(m) FROM external_event_mappings m WHERE user_id=$1",
            )
            .bind(f.user)
            .fetch_one(&pool)
            .await?;
            assert_eq!(before, after);
        }
        let mut sync = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("prosepect-sync:{}", f.user))
            .execute(&mut *sync)
            .await?;
        assert!(matches!(
            f.store
                .resolve_sync_conflict(f.user, conflict.id, "google")
                .await,
            Err(prosepect_api::error::AppError::Conflict(_))
        ));
        sync.rollback().await?;
        let resolved = f
            .store
            .resolve_sync_conflict(f.user, conflict.id, "google")
            .await?;
        assert_eq!(resolved.allowed_resolutions, ["google"]);
        f.state.empty_list.store(true, Ordering::SeqCst);
        match remote {
            "cancelled" => f.state.remote.lock().await["status"] = json!("cancelled"),
            "missing" => *f.state.remote.lock().await = serde_json::Value::Null,
            "outage" => f.state.get_status.store(503, Ordering::SeqCst),
            _ => {}
        }
        assert!(f.service.run_once().await?);
        sqlx::query("UPDATE sync_jobs SET available_at=clock_timestamp() WHERE idempotency_key=$1")
            .bind(format!("task-delete-expiry:{}", receipt.id))
            .execute(&pool)
            .await?;
        assert!(f.service.run_once().await?);
        assert!(f.state.writes.lock().await.is_empty(), "{remote}");
        assert!(
            f.store
                .list_tasks(f.user, None, None, 10)
                .await?
                .items
                .is_empty()
        );
        assert!(
            f.store
                .undo_task_delete(f.user, receipt.id, Some(&f.service))
                .await
                .is_err()
        );
        let events: i64 =
            sqlx::query_scalar("SELECT count(*) FROM calendar_events WHERE user_id=$1")
                .bind(f.user)
                .fetch_one(&pool)
                .await?;
        assert_eq!(events, i64::from(remote == "live"));
    }
    Ok(())
}

// Exercise the existing route/service wiring, so legacy repair is automatic on deletion.
async fn delete_with_provider(
    f: &UndoFixture,
) -> anyhow::Result<prosepect_api::models::TaskDeleteUndo> {
    use prosepect_api::{
        app::AppState,
        auth::CurrentUser,
        extract::{ApiJson, ApiPath},
        file_storage::FileStorage,
        rate_limit::LoginRateLimiter,
    };
    let dispatcher = SyncDispatcher::default();
    let state = AppState {
        store: f.store.clone(),
        allow_insecure_dev_auth: true,
        invite_only: false,
        groups_enabled: false,
        trust_proxy_headers: false,
        login_rate_limiter: LoginRateLimiter::default(),
        action_rate_limiter: LoginRateLimiter::default(),
        request_limits: Default::default(),
        secure_cookies: false,
        app_url: "http://localhost".into(),
        google_oauth: None,
        discord: None,
        file_storage: FileStorage::new(&prosepect_api::config::ObjectStorageConfig::Local {
            root: std::env::temp_dir()
                .join("prosepect-sync-tests")
                .to_string_lossy()
                .into_owned(),
        })?,
        max_file_size_bytes: 1024,
        max_user_file_storage_bytes: 1024,
        max_total_file_storage_bytes: 1024,
        max_user_accounts: None,
        admin_user_ids: Default::default(),
        worker_trigger_token: None,
        sync_service: Some(f.service.clone()),
        sync_dispatcher: dispatcher.clone(),
        metrics: prosepect_api::observability::initialize_metrics(),
    };
    let result = prosepect_api::task_delete_routes::delete_task_with_undo(
        State(state),
        CurrentUser(f.user),
        ApiPath(f.task.id),
        ApiJson(serde_json::from_value(
            json!({"expected_version":f.task.version}),
        )?),
    )
    .await;
    Ok(result?.0)
}

async fn seed_legacy_fingerprint(pool: &PgPool, f: &UndoFixture) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    // Actual pre-Undo six-field format, including chrono Display and enum Debug.
    let (title, description, start, end, location, recurrence): (String, String, chrono::DateTime<Utc>, chrono::DateTime<Utc>, String, prosepect_api::models::EventRecurrence) = sqlx::query_as(
        "SELECT title,description,starts_at,ends_at,location,recurrence FROM calendar_events WHERE linked_task_id=$1"
    ).bind(f.task.id).fetch_one(pool).await?;
    let legacy = format!(
        "{:x}",
        Sha256::digest(
            format!("{title}|{description}|{start}|{end}|{location}|{recurrence:?}").as_bytes()
        )
    );
    sqlx::query("UPDATE external_event_mappings SET base_fingerprint=$2 WHERE user_id=$1")
        .bind(f.user)
        .bind(&legacy)
        .execute(pool)
        .await?;
    Ok(legacy)
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_legacy_fingerprint_read_only_repair(pool: PgPool) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    undo_initial_push(&f).await?;
    let legacy = seed_legacy_fingerprint(&pool, &f).await?;
    undo_sync(&f, "unchanged-legacy-pull").await?;
    let receipt = delete_with_provider(&f).await?;
    let upgraded: String =
        sqlx::query_scalar("SELECT base_fingerprint FROM external_event_mappings WHERE user_id=$1")
            .bind(f.user)
            .fetch_one(&pool)
            .await?;
    assert_ne!(upgraded, legacy);
    f.store
        .undo_task_delete(f.user, receipt.id, Some(&f.service))
        .await?;
    undo_sync(&f, "legacy-repaired-restore").await?;
    assert!(f.state.writes.lock().await.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_legacy_repair_refuses_remote_mismatch_and_unavailability(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    undo_initial_push(&f).await?;
    let legacy = seed_legacy_fingerprint(&pool, &f).await?;
    for status in [404, 410, 401, 503] {
        f.state.get_status.store(status, Ordering::SeqCst);
        assert!(
            f.store
                .delete_task_with_undo_validated(f.user, f.task.id, 1, Some(&f.service))
                .await
                .is_err()
        );
    }
    f.state.get_status.store(0, Ordering::SeqCst);
    let original = f.state.remote.lock().await.clone();
    for (field, value) in [
        ("id", json!("wrong-identity")),
        ("etag", json!("changed-etag")),
        ("status", json!("cancelled")),
        ("description", json!("changed content")),
        ("attendees", json!([{"email":"new@example.test"}])),
        (
            "recurrence",
            json!(["RRULE:FREQ=DAILY;UNTIL=20261001T090000Z"]),
        ),
        (
            "start",
            json!({"dateTime":"2026-09-10T10:00:00Z","timeZone":"Europe/London"}),
        ),
    ] {
        f.state.remote.lock().await[field] = value;
        assert!(
            matches!(
                f.store
                    .delete_task_with_undo_validated(f.user, f.task.id, 1, Some(&f.service))
                    .await,
                Err(prosepect_api::error::AppError::Conflict(_))
            ),
            "{field}"
        );
        *f.state.remote.lock().await = original.clone();
    }
    f.state.pause_get.store(true, Ordering::SeqCst);
    assert!(matches!(
        f.store
            .delete_task_with_undo_validated(f.user, f.task.id, 1, Some(&f.service))
            .await,
        Err(prosepect_api::error::AppError::InvalidRequest {
            status: StatusCode::SERVICE_UNAVAILABLE,
            ..
        })
    ));
    f.state.pause_get.store(false, Ordering::SeqCst);
    f.state.resume.notify_one();
    let baseline: String =
        sqlx::query_scalar("SELECT base_fingerprint FROM external_event_mappings WHERE user_id=$1")
            .bind(f.user)
            .fetch_one(&pool)
            .await?;
    assert_eq!(baseline, legacy);
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 10)
            .await?
            .items
            .len(),
        1
    );
    assert!(
        f.store
            .list_task_delete_undos(f.user)
            .await?
            .items
            .is_empty()
    );
    assert!(f.state.writes.lock().await.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_legacy_repair_revalidates_after_connection_free_get(
    pool: PgPool,
) -> anyhow::Result<()> {
    for change in [
        "preference",
        "dirty",
        "etag",
        "content",
        "conflict",
        "synchronization",
    ] {
        let f = undo_fixture(&pool).await?;
        undo_initial_push(&f).await?;
        let legacy = seed_legacy_fingerprint(&pool, &f).await?;
        let single = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*pool.connect_options()).clone())
            .await?;
        let store = Store::from_pool(single.clone());
        let google = GoogleOAuth::new(GoogleOAuthConfig {
            client_id: "fake".into(),
            client_secret: "fake".into(),
            redirect_uri: "http://localhost/callback".into(),
            token_encryption_key: STANDARD.encode([41_u8; 32]),
        })?;
        let service =
            SyncService::new(store.clone(), google, None)?.with_api_base(f.api_base.clone());
        f.state.pause_get.store(true, Ordering::SeqCst);
        let attempt = store.delete_task_with_undo_validated(f.user, f.task.id, 1, Some(&service));
        let mutate = async {
            f.state.started.notified().await;
            // The one-connection pool must be available during provider validation.
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                sqlx::query("SELECT 1").execute(&single),
            )
            .await??;
            let mut sync = pool.begin().await?;
            match change {
                "preference" => {
                    sqlx::query("UPDATE calendars SET selected=FALSE WHERE user_id=$1")
                        .bind(f.user)
                        .execute(&single)
                        .await?;
                }
                "dirty" => {
                    sqlx::query(
                        "UPDATE external_event_mappings SET local_dirty=TRUE WHERE user_id=$1",
                    )
                    .bind(f.user)
                    .execute(&single)
                    .await?;
                }
                "etag" => {
                    sqlx::query("UPDATE external_event_mappings SET external_etag='changed' WHERE user_id=$1").bind(f.user).execute(&single).await?;
                }
                "content" => {
                    sqlx::query(
                        "UPDATE calendar_events SET description='changed' WHERE user_id=$1",
                    )
                    .bind(f.user)
                    .execute(&single)
                    .await?;
                }
                "conflict" => {
                    sqlx::query("UPDATE external_event_mappings SET conflict_state='unresolved' WHERE user_id=$1").bind(f.user).execute(&single).await?;
                }
                "synchronization" => {
                    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                        .bind(format!("prosepect-sync:{}", f.user))
                        .execute(&mut *sync)
                        .await?;
                }
                _ => unreachable!(),
            }
            f.state.resume.notify_one();
            anyhow::Ok(sync)
        };
        let (result, held) = tokio::join!(attempt, mutate);
        held?.rollback().await?;
        assert!(
            matches!(result, Err(prosepect_api::error::AppError::Conflict(_))),
            "{change}"
        );
        let baseline: String = sqlx::query_scalar(
            "SELECT base_fingerprint FROM external_event_mappings WHERE user_id=$1",
        )
        .bind(f.user)
        .fetch_one(&pool)
        .await?;
        assert_eq!(baseline, legacy, "{change}");
        assert_eq!(
            f.store
                .list_tasks(f.user, None, None, 10)
                .await?
                .items
                .len(),
            1
        );
        assert!(
            f.store
                .list_task_delete_undos(f.user)
                .await?
                .items
                .is_empty()
        );
        assert!(f.state.writes.lock().await.is_empty());
        single.close().await;
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_google_paused_preflight_crosses_real_deadline(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    undo_initial_push(&f).await?;
    let receipt = f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
    // Shorten both durable deadlines together, then let clock_timestamp pass them
    // while GET is paused. No mutation artificially expires the in-flight receipt.
    let deadline: chrono::DateTime<Utc> = sqlx::query_scalar("UPDATE task_delete_undos SET expires_at=clock_timestamp()+INTERVAL '1 second' WHERE id=$1 RETURNING expires_at")
        .bind(receipt.id).fetch_one(&pool).await?;
    sqlx::query("UPDATE external_event_mappings SET deletion_hold_until=$2 WHERE user_id=$1")
        .bind(f.user)
        .bind(deadline)
        .execute(&pool)
        .await?;
    f.state.pause_get.store(true, Ordering::SeqCst);
    let attempt = f
        .store
        .undo_task_delete(f.user, receipt.id, Some(&f.service));
    let expire = async {
        f.state.started.notified().await;
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        f.state.resume.notify_one();
    };
    let (result, ()) = tokio::join!(attempt, expire);
    assert!(matches!(
        result,
        Err(prosepect_api::error::AppError::Conflict(_))
    ));
    let retained: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT expires_at FROM task_delete_undos WHERE id=$1")
            .bind(receipt.id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(retained, deadline);
    assert!(
        f.store
            .list_tasks(f.user, None, None, 10)
            .await?
            .items
            .is_empty()
    );
    let held: bool = sqlx::query_scalar("SELECT local_deleted AND reversible_tombstone AND canonical_event_id IS NULL FROM external_event_mappings WHERE user_id=$1")
        .bind(f.user).fetch_one(&pool).await?;
    assert!(held);
    assert!(f.state.writes.lock().await.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_delete_undo_post_get_remote_changes_remain_write_free(
    pool: PgPool,
) -> anyhow::Result<()> {
    for cancelled in [false, true] {
        let f = undo_fixture(&pool).await?;
        undo_initial_push(&f).await?;
        let receipt = f.store.delete_task_with_undo(f.user, f.task.id, 1).await?;
        f.state.pause_get.store(true, Ordering::SeqCst);
        let attempt = f
            .store
            .undo_task_delete(f.user, receipt.id, Some(&f.service));
        let change = async {
            f.state.started.notified().await;
            let mut remote = f.state.remote.lock().await;
            remote["etag"] = json!("after-get");
            if cancelled {
                remote["status"] = json!("cancelled");
            } else {
                remote["description"] = json!("Edited after GET snapshot");
            }
            drop(remote);
            f.state.resume.notify_one();
        };
        let (result, ()) = tokio::join!(attempt, change);
        result?;
        f.state.pause_get.store(false, Ordering::SeqCst);
        assert!(f.state.writes.lock().await.is_empty());
        undo_sync(&f, "after-get-race").await?;
        assert!(f.state.writes.lock().await.is_empty());
        let tasks = f.store.list_tasks(f.user, None, None, 10).await?.items;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, f.task.id);
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn sync_conflict_capabilities_do_not_infer_tombstones_from_missing_canonical_event(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = undo_fixture(&pool).await?;
    undo_initial_push(&f).await?;
    f.store.delete_task(f.user, f.task.id, 1).await?;
    let conflict = Uuid::now_v7();
    sqlx::query("INSERT INTO sync_conflicts(id,user_id,mapping_id,canonical_event_id,title) SELECT $1,user_id,id,NULL,'Ordinary deletion' FROM external_event_mappings WHERE user_id=$2")
        .bind(conflict).bind(f.user).execute(&pool).await?;
    sqlx::query("UPDATE external_event_mappings SET conflict_state='unresolved' WHERE user_id=$1")
        .bind(f.user)
        .execute(&pool)
        .await?;
    let listed = f.store.list_sync_conflicts(f.user).await?.items.remove(0);
    assert_eq!(listed.canonical_event_id, None);
    assert_eq!(
        listed.allowed_resolutions,
        ["google", "prosepect", "latest"]
    );
    let resolved = f
        .store
        .resolve_sync_conflict(f.user, conflict, "prosepect")
        .await?;
    assert_eq!(
        resolved.allowed_resolutions,
        ["google", "prosepect", "latest"]
    );
    assert!(f.state.writes.lock().await.is_empty());
    Ok(())
}
