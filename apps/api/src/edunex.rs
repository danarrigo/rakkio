//! Owner-only import of Edunex (ITB LMS) assignments as rakkio tasks.
//!
//! Edunex has no public API or OAuth, and it refuses connections from hosted
//! servers. The owner's browser fetches assignments with the Edunex token,
//! which never reaches rakkio, and posts the normalized list here.

use axum::{extract::State, http::StatusCode, response::Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

/// Assignments whose deadline passed longer ago than this are not imported.
const IMPORT_WINDOW_DAYS: i64 = 14;
const LABEL: &str = "edunex";
const MAX_ASSIGNMENTS: usize = 2000;
const MAX_TITLE_CHARS: usize = 240;
const MAX_COURSE_CHARS: usize = 500;

/// An Edunex assignment, normalized by the browser.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
pub struct EdunexAssignment {
    /// The Edunex task id.
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub course_name: String,
    pub due_at: DateTime<Utc>,
    /// Whether an answer has been sent on Edunex.
    pub submitted: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct EdunexImportRequest {
    pub assignments: Vec<EdunexAssignment>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct EdunexSyncSummary {
    pub created: u32,
    pub updated: u32,
    pub completed: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EdunexStatus {
    pub connected: bool,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub linked_tasks: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EdunexSyncReport {
    pub status: EdunexStatus,
    pub summary: EdunexSyncSummary,
}

enum Outcome {
    Unchanged,
    Created,
    Updated,
    Completed,
}

fn validate(assignments: &[EdunexAssignment]) -> AppResult<()> {
    if assignments.len() > MAX_ASSIGNMENTS {
        return Err(AppError::Validation("Too many Edunex assignments.".into()));
    }
    for assignment in assignments {
        let title = assignment.title.trim();
        if assignment.id.is_empty()
            || assignment.id.len() > 64
            || title.is_empty()
            || title.chars().count() > MAX_TITLE_CHARS
            || assignment.course_name.chars().count() > MAX_COURSE_CHARS
        {
            return Err(AppError::Validation(
                "An Edunex assignment was not in the expected format.".into(),
            ));
        }
    }
    Ok(())
}

/// Applies assignments: new ones become tasks with the `edunex` label,
/// Edunex-side title or deadline changes are copied, and a submission completes
/// the task once. Deleted rakkio tasks stay deleted.
pub async fn apply(
    store: &Store,
    user: Uuid,
    remote: &[EdunexAssignment],
    now: DateTime<Utc>,
) -> AppResult<EdunexSyncSummary> {
    validate(remote)?;
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
    item: &EdunexAssignment,
    now: DateTime<Utc>,
) -> AppResult<Outcome> {
    let title = item.title.trim();
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
                    title: title.to_owned(),
                    description: item.course_name.trim().to_owned(),
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
        .bind(title)
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
    let title_changed = title != synced_title;
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
                        title.to_owned()
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
    .bind(title)
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

async fn status_for(store: &Store, user: Uuid) -> AppResult<EdunexStatus> {
    let last_synced_at: Option<Option<DateTime<Utc>>> =
        sqlx::query_scalar("SELECT last_synced_at FROM edunex_connections WHERE user_id = $1")
            .bind(user)
            .fetch_optional(&store.pool)
            .await?;
    let linked_tasks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM edunex_task_links WHERE user_id = $1 AND task_id IS NOT NULL",
    )
    .bind(user)
    .fetch_one(&store.pool)
    .await?;
    Ok(EdunexStatus {
        connected: last_synced_at.is_some(),
        last_synced_at: last_synced_at.flatten(),
        linked_tasks,
    })
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

#[utoipa::path(get, path = "/api/v1/integrations/edunex", operation_id = "edunex_status", responses((status = 200, body = EdunexStatus), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse)), security(("session_cookie" = []), ("development_user" = [])), tag = "synchronization")]
pub async fn status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<Json<EdunexStatus>> {
    require_owner(&state, user)?;
    Ok(Json(status_for(&state.store, user).await?))
}

#[utoipa::path(post, path = "/api/v1/integrations/edunex/import", operation_id = "edunex_import", request_body = EdunexImportRequest, responses((status = 200, body = EdunexSyncReport), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse), (status = 422, body = ErrorResponse)), security(("session_cookie" = []), ("development_user" = [])), tag = "synchronization")]
pub async fn import(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(request): ApiJson<EdunexImportRequest>,
) -> AppResult<Json<EdunexSyncReport>> {
    require_owner(&state, user)?;
    state
        .action_rate_limiter
        .check_key(&format!("edunex:{user}"))?;
    let summary = apply(&state.store, user, &request.assignments, Utc::now()).await?;
    sqlx::query(
        "INSERT INTO edunex_connections (user_id, last_synced_at) VALUES ($1, now()) \
         ON CONFLICT (user_id) DO UPDATE SET last_synced_at = now()",
    )
    .bind(user)
    .execute(&state.store.pool)
    .await?;
    Ok(Json(EdunexSyncReport {
        status: status_for(&state.store, user).await?,
        summary,
    }))
}

#[utoipa::path(delete, path = "/api/v1/integrations/edunex", operation_id = "edunex_disconnect", responses((status = 204), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse)), security(("session_cookie" = []), ("development_user" = [])), tag = "synchronization")]
pub async fn disconnect(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<StatusCode> {
    require_owner(&state, user)?;
    // Imported tasks stay; only the import bookkeeping goes.
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
