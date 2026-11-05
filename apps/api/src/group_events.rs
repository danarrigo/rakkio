use crate::{
    app::AppState,
    auth::CurrentUser,
    error::{AppResult, ErrorResponse},
    extract::{ApiJson, ApiPath, ApiQuery},
    groups,
    models::{EventRecurrence, PageQuery},
};
use axum::{Json, extract::State};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, ToSchema, FromRow)]
#[serde(deny_unknown_fields)]
pub struct GroupEventFields {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub location: String,
    #[schema(value_type=String,example="2027-03-07T09:00:00")]
    pub starts_local: NaiveDateTime,
    #[schema(value_type=String,example="2027-03-07T10:00:00")]
    pub ends_local: NaiveDateTime,
    pub all_day: bool,
    pub timezone: String,
    #[serde(default)]
    pub recurrence: EventRecurrence,
    pub recurrence_until: Option<NaiveDate>,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupEvent {
    pub id: Uuid,
    pub group_id: Uuid,
    #[sqlx(flatten)]
    pub event: GroupEventFields,
    pub version: i32,
}
#[derive(Serialize, ToSchema)]
pub struct GroupEventPage {
    pub items: Vec<GroupEvent>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGroupEvent {
    pub expected_user_id: Uuid,
    pub event: GroupEventFields,
}
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupEventScope {
    Series,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateGroupEvent {
    pub expected_user_id: Uuid,
    pub expected_version: i32,
    pub scope: GroupEventScope,
    pub event: GroupEventFields,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupEventVersion {
    pub expected_user_id: Uuid,
    pub expected_version: i32,
    pub scope: GroupEventScope,
}
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in=Query)]
#[serde(deny_unknown_fields)]
pub struct GroupEventRangeQuery {
    pub from: NaiveDate,
    /// Exclusive end of the viewer's civil-date window (at most 42 days).
    pub until: NaiveDate,
    pub timezone: String,
    pub cursor: Option<Uuid>,
    pub limit: Option<i64>,
}
#[derive(Serialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum GroupEventOrigin {
    GroupEvent,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupEventOccurrence {
    pub origin: GroupEventOrigin,
    pub event_id: Uuid,
    pub group_id: Uuid,
    pub group_name: String,
    pub can_edit: bool,
    pub original_date: NaiveDate,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub all_day: bool,
    pub timezone: String,
    pub event_version: i32,
}
#[derive(Serialize, ToSchema)]
pub struct GroupEventOccurrencePage {
    pub items: Vec<GroupEventOccurrence>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupEventUndo {
    pub id: Uuid,
    pub title: String,
    pub version: i32,
    pub undo_until: DateTime<Utc>,
}
#[derive(Serialize, ToSchema)]
pub struct GroupEventUndoPage {
    pub items: Vec<GroupEventUndo>,
    pub next_cursor: Option<Uuid>,
}

#[utoipa::path(get,operation_id="group_events_list",path="/api/v1/groups/{group_id}/events",params(("group_id"=Uuid,Path),PageQuery),responses((status=200,body=GroupEventPage),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(group): ApiPath<Uuid>,
    ApiQuery(page): ApiQuery<PageQuery>,
) -> AppResult<Json<GroupEventPage>> {
    groups::available(&state)?;
    Ok(Json(state.store.group_events(user, group, page).await?))
}
#[utoipa::path(get,operation_id="group_events_get",path="/api/v1/groups/{group_id}/events/{event_id}",params(("group_id"=Uuid,Path),("event_id"=Uuid,Path)),responses((status=200,body=GroupEvent),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn get(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
) -> AppResult<Json<GroupEvent>> {
    groups::available(&state)?;
    Ok(Json(state.store.get_group_event(user, group, id).await?))
}
#[utoipa::path(post,operation_id="group_events_create",path="/api/v1/groups/{group_id}/events",params(("group_id"=Uuid,Path)),request_body=CreateGroupEvent,responses((status=200,body=GroupEvent),(status=403,body=ErrorResponse),(status=422,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(group): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateGroupEvent>,
) -> AppResult<Json<GroupEvent>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .create_group_event(user, group, body.event)
            .await?,
    ))
}
#[utoipa::path(put,operation_id="group_events_update",path="/api/v1/groups/{group_id}/events/{event_id}",params(("group_id"=Uuid,Path),("event_id"=Uuid,Path)),request_body=UpdateGroupEvent,responses((status=200,body=GroupEvent),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<UpdateGroupEvent>,
) -> AppResult<Json<GroupEvent>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .update_group_event(user, group, id, body.expected_version, body.event)
            .await?,
    ))
}
#[utoipa::path(get,operation_id="group_event_occurrences",path="/api/v1/groups/{group_id}/event-occurrences",params(("group_id"=Uuid,Path),GroupEventRangeQuery),responses((status=200,body=GroupEventOccurrencePage),(status=401,body=ErrorResponse),(status=404,body=ErrorResponse),(status=422,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn occurrences(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(group): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<GroupEventRangeQuery>,
) -> AppResult<Json<GroupEventOccurrencePage>> {
    groups::available(&state)?;
    Ok(Json(
        state
            .store
            .group_event_occurrences(user, group, query)
            .await?,
    ))
}
#[utoipa::path(post,operation_id="group_events_delete",path="/api/v1/groups/{group_id}/events/{event_id}/delete-with-undo",params(("group_id"=Uuid,Path),("event_id"=Uuid,Path)),request_body=GroupEventVersion,responses((status=200,body=GroupEventUndo),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn delete(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<GroupEventVersion>,
) -> AppResult<Json<GroupEventUndo>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .delete_group_event(user, group, id, body.expected_version)
            .await?,
    ))
}
#[utoipa::path(post,operation_id="group_events_restore",path="/api/v1/groups/{group_id}/events/{event_id}/restore",params(("group_id"=Uuid,Path),("event_id"=Uuid,Path)),request_body=GroupEventVersion,responses((status=200,body=GroupEvent),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn restore(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((group, id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<GroupEventVersion>,
) -> AppResult<Json<GroupEvent>> {
    groups::mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state
            .store
            .restore_group_event(user, group, id, body.expected_version)
            .await?,
    ))
}
#[utoipa::path(get,operation_id="group_event_undos",path="/api/v1/groups/{group_id}/event-undos",params(("group_id"=Uuid,Path),PageQuery),responses((status=200,body=GroupEventUndoPage),(status=403,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn undos(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(group): ApiPath<Uuid>,
    ApiQuery(page): ApiQuery<PageQuery>,
) -> AppResult<Json<GroupEventUndoPage>> {
    groups::available(&state)?;
    Ok(Json(
        state.store.group_event_undos(user, group, page).await?,
    ))
}
