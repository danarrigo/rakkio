//! Owner-only import of Edunex (ITB LMS) assignments as rakkio tasks.
//!
//! Edunex has no public API or OAuth. The owner pastes the bearer token the
//! Edunex web app uses; it is stored encrypted and only ever sent to Edunex.

use std::time::Duration;

use anyhow::anyhow;
use axum::{extract::State, http::StatusCode, response::Json};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeZone, Utc};
use reqwest::{Client, Url, header};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgConnection, Row};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::CurrentUser,
    error::{AppError, AppResult, ErrorResponse},
    extract::ApiJson,
    models::{
        CreateTaskRequest, Task, TaskPriority, TaskRecurrence, TaskStatus, UpdateTaskRequest,
    },
    store::Store,
};

const API_BASE: &str = "https://api-edunex.cognisia.id/";
const WEB_ORIGIN: &str = "https://edunex.itb.ac.id";
const PAGE_SIZE: usize = 50;
const MAX_PAGES: usize = 50;
/// Assignments whose deadline passed longer ago than this are not imported.
const IMPORT_WINDOW_DAYS: i64 = 14;
/// Hourly worker runs refresh a connection at most this often.
const PERIODIC_INTERVAL_HOURS: i32 = 3;
const LABEL: &str = "edunex";
const MAX_TITLE_CHARS: usize = 240;
const REJECTED_TOKEN: &str = "Edunex rejected the saved token. Paste a fresh one in Settings.";
const UNAVAILABLE: &str = "Edunex could not be reached. rakkio will try again later.";

#[derive(Debug, thiserror::Error)]
pub enum EdunexError {
    #[error("Edunex rejected the token")]
    Unauthorized,
    #[error("Edunex is unavailable")]
    Unavailable(#[source] anyhow::Error),
}

/// An Edunex assignment, reduced to what rakkio imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdunexTask {
    pub id: String,
    pub title: String,
    pub course_name: String,
    pub due_at: DateTime<Utc>,
    pub submitted: bool,
}

#[derive(Clone)]
pub struct EdunexClient {
    http: Client,
    base: Url,
}

impl EdunexClient {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .build()?,
            base: Url::parse(API_BASE)?,
        })
    }

    pub fn with_api_base(mut self, base: Url) -> Self {
        self.base = base;
        self
    }

    /// Every active assignment visible to the token's owner.
    pub async fn tasks(&self, token: &str) -> Result<Vec<EdunexTask>, EdunexError> {
        let mut tasks = Vec::new();
        let mut offset = 0_usize;
        for _ in 0..MAX_PAGES {
            let mut url = self
                .base
                .join("course/tasks")
                .map_err(|error| EdunexError::Unavailable(error.into()))?;
            url.query_pairs_mut()
                .append_pair("filter[is_active][is]", "1")
                .append_pair("sort", "-start_at")
                .append_pair("page[limit]", &PAGE_SIZE.to_string())
                .append_pair("page[offset]", &offset.to_string());
            let response = self
                .http
                .get(url)
                .bearer_auth(token)
                .header(header::ACCEPT, "application/vnd.api+json")
                .header(header::ORIGIN, WEB_ORIGIN)
                .send()
                .await
                .map_err(|error| EdunexError::Unavailable(error.into()))?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(EdunexError::Unauthorized);
            }
            if !status.is_success() {
                return Err(EdunexError::Unavailable(anyhow!(
                    "Edunex returned {status}"
                )));
            }
            let page: Page = response.json().await.map_err(|error| {
                EdunexError::Unavailable(
                    anyhow::Error::new(error).context("Edunex returned an unexpected response"),
                )
            })?;
            let count = page.data.len();
            tasks.extend(page.data.into_iter().filter_map(Resource::into_task));
            offset += count;
            let total = page.meta.and_then(|meta| meta.total);
            let finished = match total {
                Some(total) => offset as u64 >= total,
                None => count < PAGE_SIZE,
            };
            if count == 0 || finished {
                break;
            }
        }
        Ok(tasks)
    }
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    data: Vec<Resource>,
    meta: Option<Meta>,
}

#[derive(Deserialize)]
struct Meta {
    total: Option<u64>,
}

#[derive(Deserialize)]
struct Resource {
    id: Value,
    attributes: Attributes,
}

#[derive(Deserialize)]
struct Attributes {
    name: String,
    #[serde(default)]
    course_code: Option<String>,
    #[serde(default)]
    course_name: Option<String>,
    #[serde(default)]
    end_at: Option<String>,
    #[serde(default)]
    answers: Vec<Answer>,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    is_sent: Value,
}

impl Resource {
    fn into_task(self) -> Option<EdunexTask> {
        let id = match self.id {
            Value::String(id) => id,
            Value::Number(id) => id.to_string(),
            _ => return None,
        };
        if id.is_empty() || id.len() > 64 {
            return None;
        }
        let attributes = self.attributes;
        let due_at = DateTime::parse_from_rfc3339(attributes.end_at.as_deref()?)
            .ok()?
            .with_timezone(&Utc);
        let name = attributes.name.trim();
        let code = attributes.course_code.as_deref().unwrap_or("").trim();
        let title = if code.is_empty() {
            name.to_owned()
        } else {
            format!("[{code}] {name}")
        };
        let title: String = title.chars().take(MAX_TITLE_CHARS).collect();
        if title.trim().is_empty() {
            return None;
        }
        Some(EdunexTask {
            id,
            title,
            course_name: attributes.course_name.unwrap_or_default().trim().to_owned(),
            due_at,
            submitted: attributes
                .answers
                .iter()
                .any(|answer| truthy(&answer.is_sent)),
        })
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !matches!(value.trim(), "" | "0" | "false"),
        _ => false,
    }
}

/// Reads the expiry of a JWT without verifying it; only used for display and
/// to reject an obviously expired paste.
pub fn token_expiry(token: &str) -> Option<DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    Utc.timestamp_opt(claims.get("exp")?.as_i64()?, 0).single()
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct EdunexSyncSummary {
    pub created: u32,
    pub updated: u32,
    pub completed: u32,
}

enum Outcome {
    Unchanged,
    Created,
    Updated,
    Completed,
}

/// Applies fetched assignments: new ones become tasks with the `edunex` label,
/// Edunex-side title or deadline changes are copied, and a submission completes
/// the task once. Deleted rakkio tasks stay deleted.
pub async fn apply(
    store: &Store,
    user: Uuid,
    remote: &[EdunexTask],
    now: DateTime<Utc>,
) -> AppResult<EdunexSyncSummary> {
    let mut summary = EdunexSyncSummary::default();
    for item in remote {
        let mut transaction = store.pool.begin().await?;
        match apply_one(store, &mut transaction, user, item, now).await {
            Ok(outcome) => {
                transaction.commit().await?;
                match outcome {
                    Outcome::Unchanged => {}
                    Outcome::Created => summary.created += 1,
                    Outcome::Updated => summary.updated += 1,
                    Outcome::Completed => summary.completed += 1,
                }
            }
            // A concurrent edit or task the user changed into an invalid state
            // is left alone; the next synchronization tries again.
            Err(AppError::Conflict(_) | AppError::Validation(_)) => {
                transaction.rollback().await?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(summary)
}

async fn apply_one(
    store: &Store,
    transaction: &mut PgConnection,
    user: Uuid,
    item: &EdunexTask,
    now: DateTime<Utc>,
) -> AppResult<Outcome> {
    let link = sqlx::query(
        "SELECT task_id, synced_title, synced_due_at, synced_submitted FROM edunex_task_links \
         WHERE user_id = $1 AND edunex_task_id = $2 FOR UPDATE",
    )
    .bind(user)
    .bind(&item.id)
    .fetch_optional(&mut *transaction)
    .await?;

    let Some(link) = link else {
        if item.submitted || item.due_at < now - chrono::Duration::days(IMPORT_WINDOW_DAYS) {
            return Ok(Outcome::Unchanged);
        }
        let task = store
            .create_task_in(
                transaction,
                user,
                CreateTaskRequest {
                    project_id: None,
                    parent_task_id: None,
                    title: item.title.clone(),
                    description: item.course_name.clone(),
                    due_at: Some(item.due_at),
                    scheduled_start: None,
                    scheduled_end: None,
                    status: TaskStatus::Todo,
                    priority: TaskPriority::Medium,
                    recurrence: TaskRecurrence::None,
                    labels: vec![LABEL.to_owned()],
                    remind_at: None,
                },
            )
            .await?;
        sqlx::query(
            "INSERT INTO edunex_task_links \
             (user_id, edunex_task_id, task_id, synced_title, synced_due_at, synced_submitted) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(user)
        .bind(&item.id)
        .bind(task.id)
        .bind(&item.title)
        .bind(item.due_at)
        .bind(item.submitted)
        .execute(&mut *transaction)
        .await?;
        return Ok(Outcome::Created);
    };

    let Some(task_id) = link.try_get::<Option<Uuid>, _>("task_id")? else {
        return Ok(Outcome::Unchanged);
    };
    let synced_title: String = link.try_get("synced_title")?;
    let synced_due_at: DateTime<Utc> = link.try_get("synced_due_at")?;
    let synced_submitted: bool = link.try_get("synced_submitted")?;
    let title_changed = item.title != synced_title;
    let due_changed = item.due_at != synced_due_at;
    let newly_submitted = item.submitted && !synced_submitted;
    if !title_changed && !due_changed && item.submitted == synced_submitted {
        return Ok(Outcome::Unchanged);
    }

    let task = sqlx::query_as::<_, Task>(
        "SELECT id, project_id, parent_task_id, title, description, due_at, scheduled_start, \
         scheduled_end, status, priority, recurrence, labels, remind_at, position, completed_at, \
         created_at, updated_at, version FROM tasks WHERE id = $1 AND user_id = $2",
    )
    .bind(task_id)
    .bind(user)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(task) = task else {
        return Ok(Outcome::Unchanged);
    };
    let complete = newly_submitted && task.status != TaskStatus::Completed;
    if title_changed || due_changed || complete {
        store
            .update_task_scoped_in(
                transaction,
                user,
                task_id,
                UpdateTaskRequest {
                    project_id: task.project_id,
                    parent_task_id: task.parent_task_id,
                    title: if title_changed {
                        item.title.clone()
                    } else {
                        task.title
                    },
                    description: task.description,
                    due_at: if due_changed {
                        Some(item.due_at)
                    } else {
                        task.due_at
                    },
                    scheduled_start: task.scheduled_start,
                    scheduled_end: task.scheduled_end,
                    status: if complete {
                        TaskStatus::Completed
                    } else {
                        task.status
                    },
                    priority: task.priority,
                    recurrence: task.recurrence,
                    labels: task.labels,
                    remind_at: task.remind_at,
                    expected_version: task.version,
                },
                None,
            )
            .await?;
    }
    sqlx::query(
        "UPDATE edunex_task_links SET synced_title = $3, synced_due_at = $4, synced_submitted = $5 \
         WHERE user_id = $1 AND edunex_task_id = $2",
    )
    .bind(user)
    .bind(&item.id)
    .bind(&item.title)
    .bind(item.due_at)
    .bind(item.submitted)
    .execute(&mut *transaction)
    .await?;
    Ok(if complete {
        Outcome::Completed
    } else if title_changed || due_changed {
        Outcome::Updated
    } else {
        Outcome::Unchanged
    })
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EdunexStatus {
    pub connected: bool,
    pub token_expires_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub linked_tasks: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EdunexSyncReport {
    pub status: EdunexStatus,
    pub summary: EdunexSyncSummary,
}

#[derive(Deserialize, ToSchema)]
pub struct EdunexConnectRequest {
    /// The bearer token from the Edunex web app, with or without `Bearer `.
    pub token: String,
}

async fn status_for(store: &Store, user: Uuid) -> AppResult<EdunexStatus> {
    let row = sqlx::query(
        "SELECT token_expires_at, last_synced_at, last_error FROM edunex_connections WHERE user_id = $1",
    )
    .bind(user)
    .fetch_optional(&store.pool)
    .await?;
    let linked_tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM edunex_task_links WHERE user_id = $1 AND task_id IS NOT NULL",
    )
    .bind(user)
    .fetch_one(&store.pool)
    .await?;
    Ok(match row {
        Some(row) => EdunexStatus {
            connected: true,
            token_expires_at: row.try_get("token_expires_at")?,
            last_synced_at: row.try_get("last_synced_at")?,
            last_error: row.try_get("last_error")?,
            linked_tasks,
        },
        None => EdunexStatus {
            connected: false,
            token_expires_at: None,
            last_synced_at: None,
            last_error: None,
            linked_tasks,
        },
    })
}

async fn record_attempt(store: &Store, user: Uuid, error: Option<&str>) -> AppResult<()> {
    sqlx::query(
        "UPDATE edunex_connections SET last_attempt_at = now(), last_error = $2, \
         last_synced_at = CASE WHEN $2::TEXT IS NULL THEN now() ELSE last_synced_at END \
         WHERE user_id = $1",
    )
    .bind(user)
    .bind(error)
    .execute(&store.pool)
    .await?;
    Ok(())
}

/// Fetches, applies and records the outcome for one connected user.
async fn run(store: &Store, user: Uuid, token: &str) -> AppResult<EdunexSyncSummary> {
    let client = EdunexClient::new().map_err(AppError::Integration)?;
    let remote = match client.tasks(token).await {
        Ok(remote) => remote,
        Err(EdunexError::Unauthorized) => {
            record_attempt(store, user, Some(REJECTED_TOKEN)).await?;
            return Err(AppError::Validation(REJECTED_TOKEN.to_owned()));
        }
        Err(EdunexError::Unavailable(error)) => {
            record_attempt(store, user, Some(UNAVAILABLE)).await?;
            return Err(AppError::Integration(error));
        }
    };
    let summary = apply(store, user, &remote, Utc::now()).await?;
    record_attempt(store, user, None).await?;
    Ok(summary)
}

fn require_owner(state: &AppState, user: Uuid) -> AppResult<()> {
    if state.admin_user_ids.contains(&user) {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "Edunex sync is restricted to the owner",
        ))
    }
}

async fn saved_token(state: &AppState, user: Uuid) -> AppResult<String> {
    let cipher = state
        .google_oauth
        .as_ref()
        .ok_or(AppError::NotConfigured("credential encryption"))?;
    let encrypted: Vec<u8> =
        sqlx::query_scalar("SELECT encrypted_token FROM edunex_connections WHERE user_id = $1")
            .bind(user)
            .fetch_optional(&state.store.pool)
            .await?
            .ok_or(AppError::NotFound("Edunex connection"))?;
    let token = cipher.decrypt(&encrypted).map_err(AppError::Integration)?;
    String::from_utf8(token).map_err(|error| AppError::Integration(error.into()))
}

#[utoipa::path(get, path = "/api/v1/integrations/edunex",
    operation_id = "edunex_status", responses((status = 200, body = EdunexStatus), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse)), security(("session_cookie" = []), ("development_user" = [])), tag = "synchronization")]
pub async fn status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<Json<EdunexStatus>> {
    require_owner(&state, user)?;
    Ok(Json(status_for(&state.store, user).await?))
}

#[utoipa::path(put, path = "/api/v1/integrations/edunex",
    operation_id = "edunex_connect", request_body = EdunexConnectRequest, responses((status = 200, body = EdunexSyncReport), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse), (status = 422, body = ErrorResponse), (status = 502, body = ErrorResponse)), security(("session_cookie" = []), ("development_user" = [])), tag = "synchronization")]
pub async fn connect(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(request): ApiJson<EdunexConnectRequest>,
) -> AppResult<Json<EdunexSyncReport>> {
    require_owner(&state, user)?;
    state
        .action_rate_limiter
        .check_key(&format!("edunex:{user}"))?;
    let token = request.token.trim();
    let token = token
        .strip_prefix("Bearer ")
        .or_else(|| token.strip_prefix("bearer "))
        .unwrap_or(token)
        .trim();
    if token.is_empty() || token.len() > 8192 || token.chars().any(char::is_whitespace) {
        return Err(AppError::Validation(
            "Paste the Edunex token exactly as copied.".to_owned(),
        ));
    }
    let expires_at = token_expiry(token);
    if expires_at.is_some_and(|expires_at| expires_at <= Utc::now()) {
        return Err(AppError::Validation(
            "This Edunex token has expired. Sign in to Edunex again and copy a fresh one."
                .to_owned(),
        ));
    }
    let cipher = state
        .google_oauth
        .as_ref()
        .ok_or(AppError::NotConfigured("credential encryption"))?;
    // Verify before saving so a typo never replaces a working token.
    let client = EdunexClient::new().map_err(AppError::Integration)?;
    match client.tasks(token).await {
        Ok(_) => {}
        Err(EdunexError::Unauthorized) => {
            return Err(AppError::Validation(
                "Edunex rejected this token. Copy a fresh one from Edunex.".to_owned(),
            ));
        }
        Err(EdunexError::Unavailable(error)) => return Err(AppError::Integration(error)),
    }
    let encrypted = cipher
        .encrypt(token.as_bytes())
        .map_err(AppError::Integration)?;
    sqlx::query(
        "INSERT INTO edunex_connections (user_id, encrypted_token, token_expires_at) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id) DO UPDATE SET encrypted_token = EXCLUDED.encrypted_token, \
         token_expires_at = EXCLUDED.token_expires_at, last_error = NULL",
    )
    .bind(user)
    .bind(encrypted)
    .bind(expires_at)
    .execute(&state.store.pool)
    .await?;
    let summary = run(&state.store, user, token).await?;
    Ok(Json(EdunexSyncReport {
        status: status_for(&state.store, user).await?,
        summary,
    }))
}

#[utoipa::path(post, path = "/api/v1/integrations/edunex/sync",
    operation_id = "edunex_synchronize", responses((status = 200, body = EdunexSyncReport), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 422, body = ErrorResponse), (status = 502, body = ErrorResponse)), security(("session_cookie" = []), ("development_user" = [])), tag = "synchronization")]
pub async fn synchronize(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<Json<EdunexSyncReport>> {
    require_owner(&state, user)?;
    state
        .action_rate_limiter
        .check_key(&format!("edunex:{user}"))?;
    let token = saved_token(&state, user).await?;
    let summary = run(&state.store, user, &token).await?;
    Ok(Json(EdunexSyncReport {
        status: status_for(&state.store, user).await?,
        summary,
    }))
}

#[utoipa::path(delete, path = "/api/v1/integrations/edunex",
    operation_id = "edunex_disconnect", responses((status = 204), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse)), security(("session_cookie" = []), ("development_user" = [])), tag = "synchronization")]
pub async fn disconnect(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<StatusCode> {
    require_owner(&state, user)?;
    // Imported tasks stay; only the token and the import bookkeeping go.
    let mut transaction = state.store.pool.begin().await?;
    sqlx::query("DELETE FROM edunex_task_links WHERE user_id = $1")
        .bind(user)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM edunex_connections WHERE user_id = $1")
        .bind(user)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Called by the hourly hosted worker trigger. Failures are recorded on the
/// connection and never fail the trigger.
pub async fn synchronize_due(state: &AppState) {
    let Some(cipher) = state.google_oauth.as_ref() else {
        return;
    };
    let due = sqlx::query(
        "SELECT user_id, encrypted_token FROM edunex_connections \
         WHERE last_attempt_at IS NULL OR last_attempt_at < now() - make_interval(hours => $1)",
    )
    .bind(PERIODIC_INTERVAL_HOURS)
    .fetch_all(&state.store.pool)
    .await;
    let due = match due {
        Ok(due) => due,
        Err(error) => {
            tracing::warn!(error = ?error, "Edunex connections could not be listed");
            return;
        }
    };
    for row in due {
        let (Ok(user), Ok(encrypted)) = (
            row.try_get::<Uuid, _>("user_id"),
            row.try_get::<Vec<u8>, _>("encrypted_token"),
        ) else {
            continue;
        };
        if !state.admin_user_ids.contains(&user) {
            continue;
        }
        let token = match cipher
            .decrypt(&encrypted)
            .and_then(|token| String::from_utf8(token).map_err(Into::into))
        {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(error = ?error, user_id = %user, "Edunex token could not be decrypted");
                continue;
            }
        };
        if let Err(error) = run(&state.store, user, &token).await {
            tracing::warn!(error = %error, user_id = %user, "Edunex synchronization failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assignments_and_submission_state() {
        let page: Page = serde_json::from_value(serde_json::json!({
            "meta": {"count": 3, "total": 3},
            "data": [
                {"type": "tasks", "id": "101", "attributes": {
                    "name": " Tugas 1 ", "course_code": "IF2110", "course_name": "Algoritma",
                    "end_at": "2026-11-20T23:59:00+07:00",
                    "answers": [{"is_sent": 1, "sent_at": "2026-11-19T10:00:00+07:00"}]}},
                {"type": "tasks", "id": 102, "attributes": {
                    "name": "Draft only", "course_code": null, "course_name": "Kalkulus",
                    "end_at": "2026-11-21T12:00:00+07:00", "answers": [{"is_sent": 0}]}},
                {"type": "tasks", "id": "103", "attributes": {"name": "No deadline", "answers": []}}
            ]
        }))
        .unwrap();
        let tasks: Vec<_> = page
            .data
            .into_iter()
            .filter_map(Resource::into_task)
            .collect();
        assert_eq!(
            tasks,
            vec![
                EdunexTask {
                    id: "101".into(),
                    title: "[IF2110] Tugas 1".into(),
                    course_name: "Algoritma".into(),
                    due_at: "2026-11-20T16:59:00Z".parse().unwrap(),
                    submitted: true,
                },
                EdunexTask {
                    id: "102".into(),
                    title: "Draft only".into(),
                    course_name: "Kalkulus".into(),
                    due_at: "2026-11-21T05:00:00Z".parse().unwrap(),
                    submitted: false,
                },
            ]
        );
    }

    #[test]
    fn reads_jwt_expiry_without_verification() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"exp":1795142545,"sub":"1"}"#);
        let token = format!("eyJhbGciOiJSUzI1NiJ9.{payload}.signature");
        assert_eq!(
            token_expiry(&token),
            Utc.timestamp_opt(1_795_142_545, 0).single()
        );
        assert_eq!(token_expiry("opaque-token"), None);
    }
}
