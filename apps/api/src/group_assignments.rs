use crate::{
    app::AppState,
    auth::CurrentUser,
    error::{AppResult, ErrorResponse},
    extract::{ApiJson, ApiPath, ApiQuery},
    groups,
    models::PageQuery,
};
use axum::{Json, extract::State};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

pub const PROGRESS_NOTICE_VERSION: &str = "2026-11-04-progress";
#[derive(Default, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupAssignmentFilter {
    #[default]
    Open,
    Closed,
}
impl GroupAssignmentFilter {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}
#[derive(Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct GroupAssignmentQuery {
    pub cursor: Option<Uuid>,
    pub limit: Option<i64>,
    #[serde(default)]
    pub state: GroupAssignmentFilter,
}
#[derive(Serialize, ToSchema)]
pub struct AssignmentProgress {
    pub completed_at: Option<DateTime<Utc>>,
    pub version: i32,
    pub seen_assignment_version: Option<i32>,
}
#[derive(Serialize, ToSchema)]
pub struct GroupAssignment {
    pub id: Uuid,
    pub group_id: Uuid,
    pub title: String,
    pub description: String,
    pub due_on: Option<NaiveDate>,
    pub due_time: Option<String>,
    pub timezone: String,
    pub due_at: Option<DateTime<Utc>>,
    pub state: String,
    pub version: i32,
    pub my_progress: AssignmentProgress,
    pub changed_since_completion: bool,
}
#[derive(Serialize, ToSchema)]
pub struct GroupAssignmentPage {
    pub items: Vec<GroupAssignment>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGroupAssignment {
    pub expected_user_id: Uuid,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub due_on: Option<NaiveDate>,
    pub due_time: Option<String>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateGroupAssignment {
    pub expected_user_id: Uuid,
    pub expected_version: i32,
    pub title: String,
    pub description: String,
    pub due_on: Option<NaiveDate>,
    pub due_time: Option<String>,
    pub deadline_timezone: Option<String>,
    pub state: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupAssignmentVersion {
    pub expected_user_id: Uuid,
    pub expected_version: i32,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAssignmentProgress {
    pub expected_user_id: Uuid,
    pub expected_membership_id: Uuid,
    pub expected_assignment_version: i32,
    pub expected_progress_version: i32,
    pub completed: bool,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupProgressPreferences {
    pub membership_id: Uuid,
    pub version: i32,
    pub share_progress: bool,
    pub notice_version: Option<String>,
    pub shared_at: Option<DateTime<Utc>>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateGroupProgressPreferences {
    pub expected_user_id: Uuid,
    pub expected_membership_id: Uuid,
    pub expected_version: i32,
    pub share_progress: bool,
    pub notice_version: Option<String>,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct SharedAssignmentProgress {
    pub user_id: Uuid,
    pub handle: String,
    pub display_name: String,
    pub completed_at: Option<DateTime<Utc>>,
    pub seen_assignment_version: Option<i32>,
}
#[derive(Serialize, ToSchema)]
pub struct AssignmentProgressReport {
    pub items: Vec<SharedAssignmentProgress>,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupAssignmentUndo {
    pub id: Uuid,
    pub title: String,
    pub version: i32,
    pub undo_until: DateTime<Utc>,
}
#[derive(Serialize, ToSchema)]
pub struct GroupAssignmentUndoPage {
    pub items: Vec<GroupAssignmentUndo>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupContentUsage {
    pub used_bytes: i64,
    pub used_records: i64,
    pub max_bytes: i64,
    pub max_records: i64,
}

#[utoipa::path(get,operation_id="group_assignments_list",path="/api/v1/groups/{group_id}/assignments",params(("group_id"=Uuid,Path),GroupAssignmentQuery),responses((status=200,body=GroupAssignmentPage),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(page): ApiQuery<GroupAssignmentQuery>,
) -> AppResult<Json<GroupAssignmentPage>> {
    groups::available(&state)?;
    Ok(Json(state.store.group_assignments(user, id, page).await?))
}
#[utoipa::path(post,operation_id="group_assignments_create",path="/api/v1/groups/{group_id}/assignments",params(("group_id"=Uuid,Path)),request_body=CreateGroupAssignment,responses((status=200,body=GroupAssignment),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse),(status=422,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateGroupAssignment>,
) -> AppResult<Json<GroupAssignment>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state.store.create_group_assignment(user, id, body).await?,
    ))
}
#[utoipa::path(put,operation_id="group_assignments_update",path="/api/v1/groups/{group_id}/assignments/{assignment_id}",params(("group_id"=Uuid,Path),("assignment_id"=Uuid,Path)),request_body=UpdateGroupAssignment,responses((status=200,body=GroupAssignment),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<UpdateGroupAssignment>,
) -> AppResult<Json<GroupAssignment>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .update_group_assignment(user, group, id, body)
            .await?,
    ))
}
#[utoipa::path(post,operation_id="group_assignment_progress_update",path="/api/v1/groups/{group_id}/assignments/{assignment_id}/progress",params(("group_id"=Uuid,Path),("assignment_id"=Uuid,Path)),request_body=UpdateAssignmentProgress,responses((status=200,body=GroupAssignment),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn complete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<UpdateAssignmentProgress>,
) -> AppResult<Json<GroupAssignment>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .update_group_assignment_progress(user, group, id, body)
            .await?,
    ))
}
#[utoipa::path(get,operation_id="group_assignment_progress_report",path="/api/v1/groups/{group_id}/assignments/{assignment_id}/progress-report",params(("group_id"=Uuid,Path),("assignment_id"=Uuid,Path)),responses((status=200,body=AssignmentProgressReport),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn report(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
) -> AppResult<Json<AssignmentProgressReport>> {
    groups::available(&state)?;
    Ok(Json(
        state.store.group_assignment_report(user, group, id).await?,
    ))
}
#[utoipa::path(get,operation_id="group_progress_preferences_get",path="/api/v1/groups/{group_id}/preferences",params(("group_id"=Uuid,Path)),responses((status=200,body=GroupProgressPreferences),(status=404,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn preferences(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(group): ApiPath<Uuid>,
) -> AppResult<Json<GroupProgressPreferences>> {
    groups::available(&state)?;
    Ok(Json(
        state.store.group_progress_preferences(user, group).await?,
    ))
}
#[utoipa::path(post,operation_id="group_progress_preferences_update",path="/api/v1/groups/{group_id}/preferences",params(("group_id"=Uuid,Path)),request_body=UpdateGroupProgressPreferences,responses((status=200,body=GroupProgressPreferences),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse),(status=422,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn share(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(group): ApiPath<Uuid>,
    ApiJson(body): ApiJson<UpdateGroupProgressPreferences>,
) -> AppResult<Json<GroupProgressPreferences>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .update_group_progress_preferences(user, group, body)
            .await?,
    ))
}
#[utoipa::path(post,operation_id="group_assignments_delete",path="/api/v1/groups/{group_id}/assignments/{assignment_id}/delete-with-undo",params(("group_id"=Uuid,Path),("assignment_id"=Uuid,Path)),request_body=GroupAssignmentVersion,responses((status=200,body=GroupAssignmentUndo),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<GroupAssignmentVersion>,
) -> AppResult<Json<GroupAssignmentUndo>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .delete_group_assignment(user, group, id, body.expected_version)
            .await?,
    ))
}
#[utoipa::path(post,operation_id="group_assignments_restore",path="/api/v1/groups/{group_id}/assignments/{assignment_id}/restore",params(("group_id"=Uuid,Path),("assignment_id"=Uuid,Path)),request_body=GroupAssignmentVersion,responses((status=200,body=GroupAssignment),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn restore(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<GroupAssignmentVersion>,
) -> AppResult<Json<GroupAssignment>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .restore_group_assignment(user, group, id, body.expected_version)
            .await?,
    ))
}
#[utoipa::path(get,operation_id="group_assignment_undos",path="/api/v1/groups/{group_id}/assignment-undos",params(("group_id"=Uuid,Path),PageQuery),responses((status=200,body=GroupAssignmentUndoPage),(status=403,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn undos(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(group): ApiPath<Uuid>,
    ApiQuery(page): ApiQuery<PageQuery>,
) -> AppResult<Json<GroupAssignmentUndoPage>> {
    groups::available(&state)?;
    Ok(Json(
        state
            .store
            .group_assignment_undos(user, group, page)
            .await?,
    ))
}
#[utoipa::path(get,operation_id="group_content_usage",path="/api/v1/groups/{group_id}/content-usage",params(("group_id"=Uuid,Path)),responses((status=200,body=GroupContentUsage),(status=404,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn usage(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(group): ApiPath<Uuid>,
) -> AppResult<Json<GroupContentUsage>> {
    groups::available(&state)?;
    Ok(Json(state.store.group_content_usage(user, group).await?))
}
