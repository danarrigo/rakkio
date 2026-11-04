//! Group membership interface. Shared assignments/events are separate phases.
use crate::{
    app::AppState,
    auth::CurrentUser,
    error::{AppError, AppResult, ErrorResponse},
    extract::{ApiJson, ApiPath},
};
use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

pub const ROSTER_NOTICE_VERSION: &str = "2026-11-04";

#[derive(Serialize, ToSchema)]
pub struct GroupCapability {
    pub enabled: bool,
}

#[derive(Clone, Serialize, ToSchema, FromRow)]
pub struct GroupSummary {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub timezone: String,
    pub approval_required: bool,
    pub version: i32,
    pub my_role: String,
    pub my_membership_id: Uuid,
    pub my_membership_version: i32,
    pub member_count: i64,
    pub deleted_at: Option<DateTime<Utc>>,
    pub undo_until: Option<DateTime<Utc>>,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupMember {
    pub id: Uuid,
    pub user_id: Uuid,
    pub handle: String,
    pub display_name: String,
    pub role: String,
    pub state: String,
    pub version: i32,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupInvitation {
    pub id: Uuid,
    pub group_id: Uuid,
    pub group_name: String,
    pub kind: String,
    pub recipient_handle: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub max_uses: i32,
    pub uses: i32,
    pub version: i32,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct GroupJoinRequest {
    pub id: Uuid,
    pub user_id: Uuid,
    pub handle: String,
    pub display_name: String,
    pub version: i32,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct MyGroupJoinRequest {
    pub id: Uuid,
    pub group_id: Uuid,
    pub group_name: String,
    pub state: String,
    pub version: i32,
}
#[derive(Serialize, ToSchema)]
pub struct GroupInbox {
    pub invitations: Vec<GroupInvitation>,
    pub requests: Vec<MyGroupJoinRequest>,
}
#[derive(Serialize, ToSchema, FromRow)]
pub struct OwnershipOffer {
    pub id: Uuid,
    pub to_membership_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub group_version: i32,
}
#[derive(Serialize, ToSchema)]
pub struct GroupDetail {
    pub group: GroupSummary,
    pub members: Vec<GroupMember>,
    pub removed_members: Vec<GroupMember>,
    pub invitations: Vec<GroupInvitation>,
    pub join_requests: Vec<GroupJoinRequest>,
    pub ownership_offer: Option<OwnershipOffer>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGroup {
    pub expected_user_id: Uuid,
    pub roster_notice_version: String,
    pub confirm_roster_visibility: bool,
    pub name: String,
    pub timezone: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupMutation {
    pub expected_user_id: Uuid,
    pub expected_version: i32,
    pub action: GroupAction,
}
#[derive(Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GroupAction {
    Update {
        name: String,
        timezone: String,
        approval_required: bool,
    },
    SetRole {
        membership_id: Uuid,
        expected_membership_version: i32,
        role: String,
    },
    Remove {
        membership_id: Uuid,
        expected_membership_version: i32,
    },
    Leave {
        expected_membership_version: i32,
    },
    AllowRejoin {
        membership_id: Uuid,
        expected_membership_version: i32,
    },
    ReviewJoin {
        request_id: Uuid,
        expected_request_version: i32,
        approve: bool,
    },
    OfferOwnership {
        membership_id: Uuid,
    },
    AcceptOwnership {
        offer_id: Uuid,
    },
    CancelOwnership,
    Delete {
        confirmation: String,
    },
    Restore,
}
#[derive(Serialize, ToSchema)]
pub struct GroupMutationResult {
    pub group: Option<GroupSummary>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueGroupInvitation {
    pub expected_user_id: Uuid,
    pub expected_version: i32,
    pub kind: String,
    pub handle: Option<String>,
    pub max_uses: i32,
}
#[derive(Serialize, ToSchema)]
pub struct IssuedGroupInvitation {
    pub invitation: GroupInvitation,
    pub credential: Option<String>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeGroupInvitation {
    pub expected_user_id: Uuid,
    pub expected_version: i32,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupInviteReference {
    pub invitation_id: Option<Uuid>,
    pub credential: Option<String>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JoinGroup {
    pub expected_user_id: Uuid,
    pub roster_notice_version: String,
    pub invitation_id: Option<Uuid>,
    pub credential: Option<String>,
    pub confirm_roster_visibility: bool,
}
#[derive(Serialize, ToSchema)]
pub struct GroupInvitePreview {
    pub group_id: Uuid,
    pub group_name: String,
    pub approval_required: bool,
}
#[derive(Serialize, ToSchema)]
pub struct GroupJoinResult {
    pub status: String,
    pub group_id: Uuid,
}

fn available(state: &AppState) -> AppResult<()> {
    if !state.groups_enabled {
        return Err(AppError::NotConfigured("Groups"));
    }
    Ok(())
}
fn mutation(state: &AppState, user: Uuid, expected: Uuid) -> AppResult<()> {
    available(state)?;
    if user != expected {
        return Err(AppError::Forbidden(
            "The signed-in account changed. Reload before continuing.",
        ));
    }
    state
        .action_rate_limiter
        .check_key(&format!("groups:{user}"))
}

#[utoipa::path(get,operation_id="groups_capability",path="/api/v1/groups/capability",responses((status=200,body=GroupCapability),(status=401,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn capability(
    State(state): State<AppState>,
    CurrentUser(_user): CurrentUser,
) -> Json<GroupCapability> {
    Json(GroupCapability {
        enabled: state.groups_enabled,
    })
}
#[utoipa::path(get,operation_id="groups_list",path="/api/v1/groups",responses((status=200,body=Vec<GroupSummary>),(status=401,body=ErrorResponse),(status=503,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<Json<Vec<GroupSummary>>> {
    available(&state)?;
    Ok(Json(state.store.list_groups(user).await?))
}
#[utoipa::path(post,operation_id="groups_create",path="/api/v1/groups",request_body=CreateGroup,responses((status=200,body=GroupSummary),(status=401,body=ErrorResponse),(status=403,body=ErrorResponse),(status=409,body=ErrorResponse),(status=422,body=ErrorResponse),(status=503,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(body): ApiJson<CreateGroup>,
) -> AppResult<Json<GroupSummary>> {
    mutation(&state, user, body.expected_user_id)?;
    Ok(Json(state.store.create_group(user, body).await?))
}
#[utoipa::path(get,operation_id="groups_detail",path="/api/v1/groups/{group_id}",params(("group_id"=Uuid,Path)),responses((status=200,body=GroupDetail),(status=404,body=ErrorResponse),(status=503,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn detail(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(id): ApiPath<Uuid>,
) -> AppResult<Json<GroupDetail>> {
    available(&state)?;
    Ok(Json(state.store.group_detail(user, id).await?))
}
#[utoipa::path(post,operation_id="groups_mutate",path="/api/v1/groups/{group_id}/actions",params(("group_id"=Uuid,Path)),request_body=GroupMutation,responses((status=200,body=GroupMutationResult),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse),(status=422,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn mutate(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<GroupMutation>,
) -> AppResult<Json<GroupMutationResult>> {
    mutation(&state, user, body.expected_user_id)?;
    Ok(Json(state.store.mutate_group(user, id, body).await?))
}
#[utoipa::path(post,operation_id="groups_invite",path="/api/v1/groups/{group_id}/invitations",params(("group_id"=Uuid,Path)),request_body=IssueGroupInvitation,responses((status=200,body=IssuedGroupInvitation),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse),(status=422,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn invite(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<IssueGroupInvitation>,
) -> AppResult<Json<IssuedGroupInvitation>> {
    mutation(&state, user, body.expected_user_id)?;
    Ok(Json(
        state.store.issue_group_invitation(user, id, body).await?,
    ))
}
#[utoipa::path(post,operation_id="groups_revoke",path="/api/v1/groups/{group_id}/invitations/{invitation_id}/revoke",params(("group_id"=Uuid,Path),("invitation_id"=Uuid,Path)),request_body=RevokeGroupInvitation,responses((status=200,body=bool),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn revoke(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath((id, invitation)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<RevokeGroupInvitation>,
) -> AppResult<Json<bool>> {
    mutation(&state, user, body.expected_user_id)?;
    state
        .store
        .revoke_group_invitation(user, id, invitation, body.expected_version)
        .await?;
    Ok(Json(true))
}
#[utoipa::path(get,operation_id="groups_inbox",path="/api/v1/groups/invitations/inbox",responses((status=200,body=GroupInbox),(status=401,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn inbox(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<Json<GroupInbox>> {
    available(&state)?;
    Ok(Json(state.store.group_invitation_inbox(user).await?))
}
#[utoipa::path(post,operation_id="groups_decline",path="/api/v1/groups/invitations/{invitation_id}/decline",params(("invitation_id"=Uuid,Path)),request_body=RevokeGroupInvitation,responses((status=200,body=bool),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn decline(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<RevokeGroupInvitation>,
) -> AppResult<Json<bool>> {
    mutation(&state, user, body.expected_user_id)?;
    state
        .store
        .decline_group_invitation(user, id, body.expected_version)
        .await?;
    Ok(Json(true))
}
#[utoipa::path(post,operation_id="groups_preview",path="/api/v1/groups/join/preview",request_body=GroupInviteReference,responses((status=200,body=GroupInvitePreview),(status=404,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn preview(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(body): ApiJson<GroupInviteReference>,
) -> AppResult<Json<GroupInvitePreview>> {
    available(&state)?;
    state
        .action_rate_limiter
        .check_key(&format!("group-credential:{user}"))?;
    Ok(Json(
        state.store.preview_group_invitation(user, body).await?,
    ))
}
#[utoipa::path(post,operation_id="groups_join",path="/api/v1/groups/join",request_body=JoinGroup,responses((status=200,body=GroupJoinResult),(status=403,body=ErrorResponse),(status=404,body=ErrorResponse),(status=409,body=ErrorResponse),(status=422,body=ErrorResponse)),tag="groups",security(("session_cookie"=[]),("development_user"=[])))]
pub async fn join(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(body): ApiJson<JoinGroup>,
) -> AppResult<Json<GroupJoinResult>> {
    mutation(&state, user, body.expected_user_id)?;
    state
        .action_rate_limiter
        .check_key(&format!("group-credential:{user}"))?;
    Ok(Json(state.store.join_group(user, body).await?))
}
