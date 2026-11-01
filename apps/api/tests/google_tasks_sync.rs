use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use prosepect_api::{
    config::GoogleOAuthConfig,
    google_auth::GoogleOAuth,
    google_tasks::TASKS_SCOPE,
    models::{Task, TaskStatus, UpdateTaskRequest},
    store::Store,
    sync_service::SyncService,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::{net::TcpListener, sync::Mutex};
use uuid::Uuid;

#[derive(Clone, Default)]
struct Provider {
    tasks: Arc<Mutex<HashMap<String, Value>>>,
    creates: Arc<AtomicUsize>,
    list_creates: Arc<AtomicUsize>,
    patches: Arc<AtomicUsize>,
    ambiguous_create: Arc<AtomicBool>,
    conflict_patch: Arc<AtomicBool>,
}
async fn list(State(provider): State<Provider>) -> Json<Value> {
    Json(json!({"items":provider.tasks.lock().await.values().cloned().collect::<Vec<_>>()}))
}
async fn create(State(provider): State<Provider>, Json(mut task): Json<Value>) -> Response {
    let sequence = provider.creates.fetch_add(1, Ordering::SeqCst);
    let id = format!("remote-{sequence}");
    task["id"] = json!(id);
    task["etag"] = json!("v1");
    provider.tasks.lock().await.insert(id, task.clone());
    if provider.ambiguous_create.swap(false, Ordering::SeqCst) {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    } else {
        Json(task).into_response()
    }
}
async fn get_task(State(provider): State<Provider>, Path(id): Path<String>) -> Response {
    match provider.tasks.lock().await.get(&id) {
        Some(task) => Json(task.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn update(
    State(provider): State<Provider>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    provider.patches.fetch_add(1, Ordering::SeqCst);
    let mut tasks = provider.tasks.lock().await;
    let Some(task) = tasks.get_mut(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if provider.conflict_patch.load(Ordering::SeqCst)
        || headers
            .get("if-match")
            .and_then(|value| value.to_str().ok())
            != task["etag"].as_str()
    {
        return StatusCode::PRECONDITION_FAILED.into_response();
    }
    for (key, value) in body.as_object().unwrap() {
        task[key] = value.clone();
    }
    task["etag"] = json!(Uuid::now_v7().to_string());
    Json(task.clone()).into_response()
}

async fn create_list(State(provider): State<Provider>) -> Json<Value> {
    provider.list_creates.fetch_add(1, Ordering::SeqCst);
    Json(json!({"id":"new-list","title":"prosepect"}))
}

struct Fixture {
    store: Store,
    service: SyncService,
    user: Uuid,
    task: Task,
    provider: Provider,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture(pool: &PgPool) -> anyhow::Result<Fixture> {
    let provider = Provider::default();
    let app = Router::new()
        .route(
            "/users/@me/lists",
            get(|| async { Json(json!({"items":[{"id":"list","title":"prosepect"}]})) })
                .post(create_list),
        )
        .route("/lists/list/tasks", get(list).post(create))
        .route("/lists/list/tasks/{id}", get(get_task).merge(patch(update)))
        .with_state(provider.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}/", listener.local_addr()?).parse()?;
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO users(id,email,display_name) VALUES($1,$2,'Tasks fixture')")
        .bind(user)
        .bind(format!("{user}@example.test"))
        .execute(pool)
        .await?;
    let key = [7_u8; 32];
    let nonce = [8_u8; 12];
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let mut token = nonce.to_vec();
    token.extend(
        cipher
            .encrypt(Nonce::from_slice(&nonce), b"fixture-token".as_ref())
            .unwrap(),
    );
    sqlx::query("INSERT INTO google_accounts(user_id,encrypted_access_token,access_token_expires_at,scopes) VALUES($1,$2,$3,$4)")
        .bind(user).bind(token).bind(Utc::now()+Duration::hours(1)).bind(vec![TASKS_SCOPE]).execute(pool).await?;
    let store = Store::from_pool(pool.clone());
    let task = store.create_task(user, serde_json::from_value(json!({
        "title":"Local task", "description":"Private local details", "due_at":"2026-10-02T08:30:00Z"
    }))?).await?;
    store
        .configure_google_tasks(user, true, Some(("list", "Asia/Jakarta")), 0)
        .await?;
    let google = GoogleOAuth::new(GoogleOAuthConfig {
        client_id: "fixture".into(),
        client_secret: "fixture".into(),
        redirect_uri: "http://localhost/callback".into(),
        token_encryption_key: STANDARD.encode(key),
    })?;
    let service = SyncService::new(store.clone(), google, None)?.with_tasks_api_base(base);
    Ok(Fixture {
        store,
        service,
        user,
        task,
        provider,
        server,
    })
}
async fn sync(f: &Fixture, key: &str) -> anyhow::Result<()> {
    f.store
        .enqueue_sync(f.user, None, "tasks_sync", key)
        .await?;
    assert!(f.service.run_once().await?);
    Ok(())
}
async fn current(f: &Fixture) -> anyhow::Result<Task> {
    Ok(f.store
        .list_tasks(f.user, None, None, 100)
        .await?
        .items
        .into_iter()
        .find(|task| task.id == f.task.id)
        .unwrap())
}
fn edit(task: &Task) -> UpdateTaskRequest {
    UpdateTaskRequest {
        project_id: task.project_id,
        parent_task_id: task.parent_task_id,
        title: task.title.clone(),
        description: task.description.clone(),
        due_at: task.due_at,
        scheduled_start: task.scheduled_start,
        scheduled_end: task.scheduled_end,
        status: task.status,
        priority: task.priority,
        recurrence: task.recurrence,
        labels: task.labels.clone(),
        remind_at: task.remind_at,
        expected_version: task.version,
    }
}

fn app_state(f: &Fixture) -> anyhow::Result<prosepect_api::app::AppState> {
    use prosepect_api::{
        config::ObjectStorageConfig, file_storage::FileStorage, rate_limit::LoginRateLimiter,
        sync_dispatcher::SyncDispatcher,
    };
    Ok(prosepect_api::app::AppState {
        store: f.store.clone(),
        allow_insecure_dev_auth: true,
        invite_only: false,
        trust_proxy_headers: false,
        login_rate_limiter: LoginRateLimiter::default(),
        action_rate_limiter: LoginRateLimiter::default(),
        request_limits: Default::default(),
        secure_cookies: false,
        app_url: "http://localhost".into(),
        google_oauth: None,
        discord: None,
        file_storage: FileStorage::new(&ObjectStorageConfig::Local {
            root: std::env::temp_dir()
                .join("prosepect-tasks-fixture")
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
        sync_dispatcher: SyncDispatcher::default(),
        metrics: prosepect_api::observability::initialize_metrics(),
    })
}

#[sqlx::test(migrations = "../../migrations")]
async fn tasks_settings_routes_verify_provider_list_and_claim_creation_once(
    pool: PgPool,
) -> anyhow::Result<()> {
    use axum::extract::State;
    use prosepect_api::{
        auth::CurrentUser,
        extract::ApiJson,
        google_tasks_routes::{self, GoogleTasksCreateListRequest, GoogleTasksSettingsRequest},
    };
    let f = fixture(&pool).await?;
    sqlx::query("DELETE FROM google_task_connections WHERE user_id=$1")
        .bind(f.user)
        .execute(&pool)
        .await?;
    let state = app_state(&f)?;
    let rejected = google_tasks_routes::configure(
        State(state.clone()),
        CurrentUser(f.user),
        ApiJson(GoogleTasksSettingsRequest {
            enabled: true,
            task_list_id: Some("other-users-list".into()),
            timezone: Some("Asia/Jakarta".into()),
            expected_version: 0,
        }),
    )
    .await;
    assert!(matches!(
        rejected,
        Err(prosepect_api::error::AppError::Forbidden(_))
    ));
    assert_eq!(f.store.google_tasks_status(f.user).await?.version, 0);
    let (status, created) = google_tasks_routes::create_list(
        State(state.clone()),
        CurrentUser(f.user),
        ApiJson(GoogleTasksCreateListRequest {
            expected_version: 0,
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created.0.id, "new-list");
    let saved = google_tasks_routes::configure(
        State(state.clone()),
        CurrentUser(f.user),
        ApiJson(GoogleTasksSettingsRequest {
            enabled: true,
            task_list_id: Some("list".into()),
            timezone: Some("Asia/Jakarta".into()),
            expected_version: 1,
        }),
    )
    .await?
    .0;
    assert_eq!(saved.version, 2);
    let job: String =
        sqlx::query_scalar("SELECT kind FROM sync_jobs WHERE user_id=$1 AND idempotency_key=$2")
            .bind(f.user)
            .bind(format!("tasks-enable:{}:2", f.user))
            .fetch_one(&pool)
            .await?;
    assert_eq!(job, "tasks_sync");
    let repeated = google_tasks_routes::create_list(
        State(state),
        CurrentUser(f.user),
        ApiJson(GoogleTasksCreateListRequest {
            expected_version: 2,
        }),
    )
    .await;
    assert!(matches!(
        repeated,
        Err(prosepect_api::error::AppError::Conflict(_))
    ));
    assert_eq!(f.provider.list_creates.load(Ordering::SeqCst), 1);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_creation_after_saving_disabled_settings_is_claimed_once(
    pool: PgPool,
) -> anyhow::Result<()> {
    use prosepect_api::{
        auth::CurrentUser,
        extract::ApiJson,
        google_tasks_routes::{self, GoogleTasksCreateListRequest, GoogleTasksSettingsRequest},
    };
    let f = fixture(&pool).await?;
    sqlx::query("DELETE FROM google_task_connections WHERE user_id=$1")
        .bind(f.user)
        .execute(&pool)
        .await?;
    let state = app_state(&f)?;
    let disabled = google_tasks_routes::configure(
        State(state.clone()),
        CurrentUser(f.user),
        ApiJson(GoogleTasksSettingsRequest {
            enabled: false,
            task_list_id: None,
            timezone: None,
            expected_version: 0,
        }),
    )
    .await?
    .0;
    assert_eq!(disabled.version, 1);
    assert!(!disabled.list_create_attempted);
    let stale = google_tasks_routes::create_list(
        State(state.clone()),
        CurrentUser(f.user),
        ApiJson(GoogleTasksCreateListRequest {
            expected_version: 0,
        }),
    )
    .await;
    assert!(stale.is_err());
    assert_eq!(f.provider.list_creates.load(Ordering::SeqCst), 0);
    let (first, concurrent) = tokio::join!(
        google_tasks_routes::create_list(
            State(state.clone()),
            CurrentUser(f.user),
            ApiJson(GoogleTasksCreateListRequest {
                expected_version: 1
            })
        ),
        google_tasks_routes::create_list(
            State(state.clone()),
            CurrentUser(f.user),
            ApiJson(GoogleTasksCreateListRequest {
                expected_version: 1
            })
        ),
    );
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(concurrent.is_ok()),
        1,
        "one current-version request must create the list: {first:?}, {concurrent:?}"
    );
    assert_eq!(f.provider.list_creates.load(Ordering::SeqCst), 1);
    let current = f.store.google_tasks_status(f.user).await?;
    assert!(current.list_create_attempted);
    assert_eq!(current.version, 2);
    assert!(
        google_tasks_routes::create_list(
            State(state),
            CurrentUser(f.user),
            ApiJson(GoogleTasksCreateListRequest {
                expected_version: 2
            })
        )
        .await
        .is_err()
    );
    assert_eq!(f.provider.list_creates.load(Ordering::SeqCst), 1);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn task_change_jobs_coalesce_without_losing_edits_during_running_sync(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    for number in 0..8 {
        let mut request = edit(&current(&f).await?);
        request.title = format!("Edit {number}");
        f.store.update_task(f.user, f.task.id, request).await?;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_jobs WHERE user_id=$1 AND kind='tasks_sync' AND status='pending'")
        .bind(f.user).fetch_one(&pool).await?;
    assert_eq!(
        count, 1,
        "a burst of edits needs one pending Tasks snapshot, not one per edit"
    );
    let running = f.store.claim_sync_job().await?.unwrap();
    assert_eq!(running.kind, "tasks_sync");
    // Private fields and workflow-only states never change shared Tasks fields.
    let mut request = edit(&current(&f).await?);
    request.description = "Private draft".into();
    request.status = TaskStatus::InProgress;
    f.store.update_task(f.user, f.task.id, request).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_jobs WHERE user_id=$1 AND kind='tasks_sync' AND status='pending'")
        .bind(f.user).fetch_one(&pool).await?;
    assert_eq!(count, 0, "private edits should not enqueue Tasks work");
    for number in 0..8 {
        let mut request = edit(&current(&f).await?);
        request.title = format!("During sync {number}");
        f.store.update_task(f.user, f.task.id, request).await?;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_jobs WHERE user_id=$1 AND kind='tasks_sync' AND status='pending'")
        .bind(f.user).fetch_one(&pool).await?;
    assert_eq!(
        count, 1,
        "edits after claim must retain one follow-up even while a job runs"
    );
    f.store.complete_sync_job(running.id).await?;
    assert!(f.service.run_once().await?);
    assert_eq!(
        f.provider.tasks.lock().await["remote-0"]["title"],
        "During sync 7"
    );
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    assert!(!f.service.run_once().await?);
    // A later edit can enqueue again, even though earlier jobs succeeded.
    let mut request = edit(&current(&f).await?);
    request.title = "Later edit".into();
    f.store.update_task(f.user, f.task.id, request).await?;
    assert!(f.service.run_once().await?);
    assert_eq!(
        f.provider.tasks.lock().await["remote-0"]["title"],
        "Later edit"
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn coalesced_task_change_cannot_be_claimed_before_its_writer_commits(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    let mut request = edit(&current(&f).await?);
    request.title = "Committed edit".into();
    f.store.update_task(f.user, f.task.id, request).await?;
    let mut writer = pool.begin().await?;
    sqlx::query(
        "UPDATE tasks SET title='Uncommitted edit',version=version+1 WHERE id=$1 AND user_id=$2",
    )
    .bind(f.task.id)
    .bind(f.user)
    .execute(&mut *writer)
    .await?;
    assert!(
        f.store.claim_sync_job().await?.is_none(),
        "a coalesced pending job must remain locked until the task writer commits"
    );
    writer.commit().await?;
    assert!(f.service.run_once().await?);
    assert_eq!(
        f.provider.tasks.lock().await["remote-0"]["title"],
        "Uncommitted edit"
    );
    assert!(!f.service.run_once().await?);
    let mut cancelled = pool.begin().await?;
    sqlx::query(
        "UPDATE tasks SET title='Rolled back edit',version=version+1 WHERE id=$1 AND user_id=$2",
    )
    .bind(f.task.id)
    .bind(f.user)
    .execute(&mut *cancelled)
    .await?;
    cancelled.rollback().await?;
    assert!(
        !f.service.run_once().await?,
        "rolled-back edits must not leave queued jobs"
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn tasks_sync_batches_resume_and_revisit_edits_behind_the_cursor(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    for number in 0..22 {
        f.store
            .create_task(
                f.user,
                serde_json::from_value(json!({"title":format!("Batch task {number}")}))?,
            )
            .await?;
        let id = format!("incoming-{number:02}");
        f.provider.tasks.lock().await.insert(id.clone(), json!({"id":id,"etag":"initial","title":format!("Incoming {number}"),"status":"needsAction"}));
    }
    assert!(f.service.run_once().await?);
    assert!(
        f.provider.creates.load(Ordering::SeqCst) <= 10,
        "one job must not create every task"
    );
    assert!(
        f.store
            .list_tasks(f.user, None, None, 100)
            .await?
            .items
            .len()
            <= 33,
        "one job must import at most ten tasks"
    );
    assert!(
        f.store
            .google_tasks_status(f.user)
            .await?
            .last_synced_at
            .is_none(),
        "partial progress is not a completed synchronization"
    );
    let mut edit_behind_cursor = edit(&current(&f).await?);
    edit_behind_cursor.title = "Changed behind the cursor".into();
    f.store
        .update_task(f.user, f.task.id, edit_behind_cursor)
        .await?;
    let mut finished = false;
    for _ in 0..40 {
        let before = f.provider.creates.load(Ordering::SeqCst);
        if !f.service.run_once().await? {
            finished = true;
            break;
        }
        assert!(f.provider.creates.load(Ordering::SeqCst) - before <= 10);
    }
    assert!(
        finished,
        "continuations must eventually drain, not loop forever"
    );
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 23);
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 100)
            .await?
            .items
            .len(),
        45
    );
    assert!(
        f.provider
            .tasks
            .lock()
            .await
            .values()
            .any(|task| task["title"] == "Changed behind the cursor")
    );
    assert!(
        f.store
            .google_tasks_status(f.user)
            .await?
            .last_synced_at
            .is_some()
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn tasks_sync_merges_both_directions_without_losing_deadline_time_or_private_fields(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    sync(&f, "initial").await?;
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    let remote = f.provider.tasks.lock().await["remote-0"].clone();
    assert_eq!(remote["due"], "2026-10-02T00:00:00.000Z");
    assert!(!remote["notes"].as_str().unwrap().contains("Private"));
    let mut request = edit(&f.task);
    request.title = "Local rename".into();
    f.store.update_task(f.user, f.task.id, request).await?;
    {
        let mut tasks = f.provider.tasks.lock().await;
        let remote = tasks.get_mut("remote-0").unwrap();
        remote["due"] = json!("2026-10-03T00:00:00.000Z");
        remote["status"] = json!("completed");
        remote["etag"] = json!("google-edit");
    }
    sync(&f, "merge").await?;
    let task = current(&f).await?;
    assert_eq!(task.title, "Local rename");
    assert_eq!(task.status, TaskStatus::Completed);
    assert_eq!(task.due_at, Some("2026-10-03T08:30:00Z".parse()?));
    assert_eq!(task.description, "Private local details");
    assert_eq!(
        f.provider.tasks.lock().await["remote-0"]["title"],
        "Local rename"
    );
    let patches = f.provider.patches.load(Ordering::SeqCst);
    sync(&f, "stable").await?;
    assert_eq!(f.provider.patches.load(Ordering::SeqCst), patches);
    f.provider.tasks.lock().await.get_mut("remote-0").unwrap()["status"] = json!("needsAction");
    sync(&f, "reopen").await?;
    assert_eq!(current(&f).await?.status, TaskStatus::Todo);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn tasks_sync_preserves_same_field_conflicts_and_conditional_write_rejection(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    sync(&f, "initial").await?;
    let mut request = edit(&f.task);
    request.title = "Local rename".into();
    f.store.update_task(f.user, f.task.id, request).await?;
    f.provider.tasks.lock().await.get_mut("remote-0").unwrap()["title"] = json!("Google rename");
    sync(&f, "conflict").await?;
    assert_eq!(current(&f).await?.title, "Local rename");
    assert_eq!(
        f.provider.tasks.lock().await["remote-0"]["title"],
        "Google rename"
    );
    assert_eq!(f.provider.patches.load(Ordering::SeqCst), 0);
    let errors: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM google_task_links WHERE user_id=$1 AND last_error IS NOT NULL",
    )
    .bind(f.user)
    .fetch_one(&pool)
    .await?;
    assert_eq!(errors, 1);
    f.provider.tasks.lock().await.get_mut("remote-0").unwrap()["title"] = json!("Local task");
    f.provider.conflict_patch.store(true, Ordering::SeqCst);
    sync(&f, "etag-conflict").await?;
    assert_eq!(
        f.provider.tasks.lock().await["remote-0"]["title"],
        "Local task"
    );
    assert_eq!(current(&f).await?.title, "Local rename");
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn conflict_choices_merge_independent_edits_and_reject_stale_snapshots(
    pool: PgPool,
) -> anyhow::Result<()> {
    use prosepect_api::google_tasks::TaskConflictChoice;
    let f = fixture(&pool).await?;
    sync(&f, "initial").await?;
    let mut request = edit(&current(&f).await?);
    request.title = "Local rename".into();
    f.store.update_task(f.user, f.task.id, request).await?;
    {
        let mut remote = f.provider.tasks.lock().await;
        remote.get_mut("remote-0").unwrap()["title"] = json!("Google rename");
        remote.get_mut("remote-0").unwrap()["due"] = json!("2026-10-03T00:00:00.000Z");
    }
    sync(&f, "conflict").await?;
    let conflicts = f.store.google_task_conflicts(f.user).await?;
    assert_eq!(conflicts.len(), 1);
    let conflict = &conflicts[0];
    assert!(
        f.store
            .google_task_conflicts(Uuid::now_v7())
            .await?
            .is_empty()
    );
    assert!(
        f.store
            .resolve_google_task_conflict(
                Uuid::now_v7(),
                conflict.link_id,
                conflict.id,
                TaskConflictChoice::Prosepect
            )
            .await
            .is_err()
    );
    f.store
        .resolve_google_task_conflict(
            f.user,
            conflict.link_id,
            conflict.id,
            TaskConflictChoice::Prosepect,
        )
        .await?;
    assert!(
        f.store
            .resolve_google_task_conflict(
                f.user,
                conflict.link_id,
                conflict.id,
                TaskConflictChoice::Google
            )
            .await
            .is_err()
    );
    sync(&f, "resolve").await?;
    assert_eq!(current(&f).await?.title, "Local rename");
    assert_eq!(
        current(&f).await?.due_at,
        Some("2026-10-03T08:30:00Z".parse()?)
    );
    assert_eq!(
        f.provider.tasks.lock().await["remote-0"]["title"],
        "Local rename"
    );
    assert!(f.store.google_task_conflicts(f.user).await?.is_empty());

    let mut request = edit(&current(&f).await?);
    request.title = "Second local rename".into();
    f.store.update_task(f.user, f.task.id, request).await?;
    f.provider.tasks.lock().await.get_mut("remote-0").unwrap()["title"] =
        json!("Second Google rename");
    sync(&f, "second-conflict").await?;
    let conflict = f.store.google_task_conflicts(f.user).await?.remove(0);
    f.store
        .resolve_google_task_conflict(
            f.user,
            conflict.link_id,
            conflict.id,
            TaskConflictChoice::Google,
        )
        .await?;
    f.provider.tasks.lock().await.get_mut("remote-0").unwrap()["title"] =
        json!("Newer Google rename");
    sync(&f, "stale-resolution").await?;
    assert_eq!(current(&f).await?.title, "Second local rename");
    let newer = f.store.google_task_conflicts(f.user).await?.remove(0);
    assert_ne!(newer.id, conflict.id);
    assert_eq!(newer.google.title, "Newer Google rename");
    assert!(
        f.store
            .resolve_google_task_conflict(
                f.user,
                conflict.link_id,
                conflict.id,
                TaskConflictChoice::Google
            )
            .await
            .is_err()
    );
    let mut local_edit = edit(&current(&f).await?);
    local_edit.description = "New private details".into();
    f.store.update_task(f.user, f.task.id, local_edit).await?;
    assert!(
        f.store
            .resolve_google_task_conflict(
                f.user,
                newer.link_id,
                newer.id,
                TaskConflictChoice::Google
            )
            .await
            .is_err()
    );
    sync(&f, "new-local-version").await?;
    let newest = f.store.google_task_conflicts(f.user).await?.remove(0);
    assert_ne!(newest.id, newer.id);
    f.store
        .resolve_google_task_conflict(
            f.user,
            newest.link_id,
            newest.id,
            TaskConflictChoice::Google,
        )
        .await?;
    sync(&f, "fresh-resolution").await?;
    assert_eq!(current(&f).await?.title, "Newer Google rename");
    assert!(f.store.google_task_conflicts(f.user).await?.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn uncertain_task_create_recovers_without_duplicate_posts_or_imports(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    f.provider.ambiguous_create.store(true, Ordering::SeqCst);
    sync(&f, "ambiguous").await?;
    let phase: String = sqlx::query_scalar("SELECT phase FROM google_task_links WHERE user_id=$1")
        .bind(f.user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(phase, "creating");
    sync(&f, "recover").await?;
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 100)
            .await?
            .items
            .len(),
        1
    );
    let phase: String = sqlx::query_scalar("SELECT phase FROM google_task_links WHERE user_id=$1")
        .bind(f.user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(phase, "linked");
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn uncertain_creation_can_be_left_unlinked_without_writes_or_reimport(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    f.provider.ambiguous_create.store(true, Ordering::SeqCst);
    sync(&f, "ambiguous").await?;
    let remote = f.provider.tasks.lock().await.remove("remote-0").unwrap();
    let recoveries = f.store.google_task_recoveries(f.user).await?;
    assert_eq!(recoveries.len(), 1);
    let recovery = &recoveries[0];
    assert_eq!(recovery.task_id, f.task.id);
    assert!(
        f.store
            .google_task_recoveries(Uuid::now_v7())
            .await?
            .is_empty()
    );
    assert!(
        f.store
            .leave_google_task_unlinked(Uuid::now_v7(), recovery.link_id)
            .await
            .is_err()
    );
    let mut worker = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("prosepect-sync:{}", f.user))
        .execute(&mut *worker)
        .await?;
    assert!(
        f.store
            .leave_google_task_unlinked(f.user, recovery.link_id)
            .await
            .is_err()
    );
    worker.rollback().await?;
    let receipt = f
        .store
        .delete_task_with_undo(f.user, f.task.id, f.task.version)
        .await?;
    assert!(
        f.store
            .leave_google_task_unlinked(f.user, recovery.link_id)
            .await
            .is_err()
    );
    f.store.undo_task_delete(f.user, receipt.id, None).await?;
    f.store
        .leave_google_task_unlinked(f.user, recovery.link_id)
        .await?;
    assert!(
        f.store
            .leave_google_task_unlinked(f.user, recovery.link_id)
            .await
            .is_err()
    );
    assert!(f.store.google_task_recoveries(f.user).await?.is_empty());
    // A delayed Google copy retains the marker. Do not recreate or import it.
    f.provider
        .tasks
        .lock()
        .await
        .insert("remote-0".into(), remote);
    sync(&f, "after-detach").await?;
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.provider.patches.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 100)
            .await?
            .items
            .len(),
        1
    );
    assert_eq!(current(&f).await?.title, f.task.title);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn uncertain_creation_never_retries_missing_or_duplicate_provenance(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    f.provider.ambiguous_create.store(true, Ordering::SeqCst);
    sync(&f, "ambiguous").await?;
    let remote = f.provider.tasks.lock().await.remove("remote-0").unwrap();
    sync(&f, "missing").await?;
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    let mut duplicate = remote.clone();
    duplicate["id"] = json!("duplicate");
    {
        let mut tasks = f.provider.tasks.lock().await;
        tasks.insert("remote-0".into(), remote);
        tasks.insert("duplicate".into(), duplicate);
    }
    sync(&f, "duplicate").await?;
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.provider.patches.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 100)
            .await?
            .items
            .len(),
        1
    );
    let phase: String = sqlx::query_scalar("SELECT phase FROM google_task_links WHERE user_id=$1")
        .bind(f.user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(phase, "creating");
    // Removing only the duplicate permits discovery without a new POST.
    f.provider.tasks.lock().await.remove("duplicate");
    sync(&f, "unique").await?;
    let phase: String = sqlx::query_scalar("SELECT phase FROM google_task_links WHERE user_id=$1")
        .bind(f.user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(phase, "linked");
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn uncertain_creation_recovers_later_edits_without_overwriting_either_app(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    f.provider.ambiguous_create.store(true, Ordering::SeqCst);
    sync(&f, "ambiguous").await?;
    let mut request = edit(&current(&f).await?);
    request.title = "Edited locally after lost response".into();
    f.store.update_task(f.user, f.task.id, request).await?;
    f.provider.tasks.lock().await.get_mut("remote-0").unwrap()["title"] =
        json!("Edited on Google after creation");
    sync(&f, "recover-edited").await?;
    let phase: String = sqlx::query_scalar("SELECT phase FROM google_task_links WHERE user_id=$1")
        .bind(f.user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        phase, "linked",
        "a unique provenance marker should recover identity, not require unchanged content"
    );
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.provider.patches.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 100)
            .await?
            .items
            .len(),
        1
    );
    sync(&f, "reconcile-recovered").await?;
    let conflicts = f.store.google_task_conflicts(f.user).await?;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].local.title,
        "Edited locally after lost response"
    );
    assert_eq!(conflicts[0].google.title, "Edited on Google after creation");
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.provider.patches.load(Ordering::SeqCst), 0);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn tasks_import_is_atomic_and_deletion_does_not_resurrect_or_destroy_either_copy(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    f.provider.tasks.lock().await.insert("incoming".into(), json!({
        "id":"incoming", "etag":"one", "title":"Google task", "due":"2026-10-04T00:00:00Z", "status":"completed", "hidden":true
    }));
    sync(&f, "initial").await?;
    let imported = f
        .store
        .list_tasks(f.user, None, None, 100)
        .await?
        .items
        .into_iter()
        .find(|task| task.title == "Google task")
        .unwrap();
    assert_eq!(imported.status, TaskStatus::Completed);
    assert_eq!(imported.due_at, Some("2026-10-04T16:59:59Z".parse()?));
    sync(&f, "stable").await?;
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 100)
            .await?
            .items
            .len(),
        2
    );
    // Google removal detaches without deleting or recreating the local task.
    f.provider.tasks.lock().await.remove("remote-0");
    sync(&f, "remote-delete").await?;
    assert_eq!(current(&f).await?.id, f.task.id);
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    f.store
        .delete_task(f.user, imported.id, imported.version)
        .await?;
    sync(&f, "local-delete").await?;
    assert!(f.provider.tasks.lock().await.contains_key("incoming"));
    assert_eq!(
        f.store
            .list_tasks(f.user, None, None, 100)
            .await?
            .items
            .len(),
        1
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn active_deletion_undo_holds_tasks_sync_and_restores_the_same_mapping(
    pool: PgPool,
) -> anyhow::Result<()> {
    let f = fixture(&pool).await?;
    sync(&f, "initial").await?;
    let receipt = f
        .store
        .delete_task_with_undo(f.user, f.task.id, f.task.version)
        .await?;
    sync(&f, "held").await?;
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.provider.patches.load(Ordering::SeqCst), 0);
    f.store.undo_task_delete(f.user, receipt.id, None).await?;
    sync(&f, "restored").await?;
    assert_eq!(f.provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(f.provider.patches.load(Ordering::SeqCst), 0);
    let phase: String = sqlx::query_scalar("SELECT phase FROM google_task_links WHERE user_id=$1")
        .bind(f.user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(phase, "linked");
    Ok(())
}
