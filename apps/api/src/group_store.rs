use crate::{
    error::{AppError, AppResult},
    groups::*,
    store::Store,
};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

const SUMMARY: &str = "SELECT g.id,g.owner_id,g.name,g.timezone,g.approval_required,g.version,CASE WHEN g.owner_id=$1 THEN 'owner' ELSE m.role END AS my_role,m.id AS my_membership_id,m.version AS my_membership_version,(SELECT COUNT(*) FROM group_memberships c WHERE c.group_id=g.id AND c.state='active') AS member_count,g.deleted_at,g.undo_until FROM groups g JOIN group_memberships m ON m.group_id=g.id AND m.user_id=$1 AND m.state='active'";
const INVITATION: &str = "SELECT i.id,i.group_id,g.name AS group_name,i.kind,h.handle AS recipient_handle,i.expires_at,i.max_uses,i.uses,i.version FROM group_invitations i JOIN groups g ON g.id=i.group_id LEFT JOIN user_handles h ON h.user_id=i.recipient_id";
const MEMBER: &str = "SELECT m.id,m.user_id,h.handle,LEFT(u.display_name,120) AS display_name,CASE WHEN g.owner_id=m.user_id THEN 'owner' ELSE m.role END AS role,m.state,m.version FROM group_memberships m JOIN groups g ON g.id=m.group_id JOIN users u ON u.id=m.user_id JOIN user_handles h ON h.user_id=m.user_id";

#[derive(FromRow)]
struct Access {
    owner_id: Uuid,
    version: i32,
    deleted_at: Option<DateTime<Utc>>,
    my_role: String,
    my_membership_id: Uuid,
}
impl Access {
    fn owner(&self, user: Uuid) -> AppResult<()> {
        if self.owner_id != user {
            return Err(AppError::Forbidden("Only the group owner can do that."));
        }
        Ok(())
    }
    fn leader(&self, user: Uuid) -> AppResult<()> {
        if self.owner_id != user && self.my_role != "leader" {
            return Err(AppError::Forbidden("A group leader is required."));
        }
        Ok(())
    }
    fn version(&self, version: i32) -> AppResult<()> {
        if self.version != version {
            return Err(AppError::Conflict(
                "The group changed. Refresh before continuing.".into(),
            ));
        }
        Ok(())
    }
}
async fn lock_users(c: &mut PgConnection, mut users: Vec<Uuid>) -> AppResult<()> {
    users.sort();
    users.dedup();
    let found = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE id=ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(&users)
    .fetch_all(&mut *c)
    .await?;
    if found.len() != users.len() {
        return Err(AppError::Conflict(
            "An account is no longer available. Refresh before continuing.".into(),
        ));
    }
    let blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=ANY($1) AND group_admission_blocked)",
    )
    .bind(&users)
    .fetch_one(c)
    .await?;
    if blocked {
        return Err(AppError::Forbidden(
            "Account deletion is in progress. Retry account deletion before making group changes.",
        ));
    }
    Ok(())
}
async fn access(c: &mut PgConnection, user: Uuid, group: Uuid) -> AppResult<Access> {
    sqlx::query_as("SELECT g.owner_id,g.version,g.deleted_at,m.role AS my_role,m.id AS my_membership_id FROM groups g JOIN group_memberships m ON m.group_id=g.id AND m.user_id=$1 AND m.state='active' WHERE g.id=$2 AND (g.deleted_at IS NULL OR g.owner_id=$1) FOR UPDATE OF g")
        .bind(user).bind(group).fetch_optional(c).await?.ok_or(AppError::NotFound("group"))
}
async fn summary(c: &mut PgConnection, user: Uuid, group: Uuid) -> AppResult<GroupSummary> {
    sqlx::query_as(&format!(
        "{SUMMARY} WHERE g.id=$2 AND (g.deleted_at IS NULL OR g.owner_id=$1)"
    ))
    .bind(user)
    .bind(group)
    .fetch_optional(c)
    .await?
    .ok_or(AppError::NotFound("group"))
}
async fn valid_group(c: &mut PgConnection, name: &str, timezone: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(AppError::Validation(
            "Group names must contain 1-120 characters on one line.".into(),
        ));
    }
    if timezone.len() > 64
        || !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name=$1)",
        )
        .bind(timezone)
        .fetch_one(c)
        .await?
    {
        return Err(AppError::Validation("Choose a valid IANA timezone.".into()));
    }
    Ok(name.into())
}
async fn require_handle(c: &mut PgConnection, user: Uuid) -> AppResult<()> {
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM user_handles WHERE user_id=$1)")
        .bind(user)
        .fetch_one(c)
        .await?
    {
        return Err(AppError::Validation(
            "Choose your @handle in Settings before using groups.".into(),
        ));
    }
    Ok(())
}
async fn membership_capacity(
    c: &mut PgConnection,
    user: Uuid,
    group: Option<Uuid>,
) -> AppResult<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM group_memberships WHERE user_id=$1 AND state='active'",
    )
    .bind(user)
    .fetch_one(&mut *c)
    .await?;
    if count >= 20 {
        return Err(AppError::Forbidden("Membership limit reached (20 groups)."));
    }
    if let Some(group) = group {
        let (active,records):(i64,i64)=sqlx::query_as("SELECT COUNT(*) FILTER(WHERE state='active'),COUNT(*) FILTER(WHERE user_id<>$2) FROM group_memberships WHERE group_id=$1").bind(group).bind(user).fetch_one(c).await?;
        if active >= 100 || records >= 500 {
            return Err(AppError::Forbidden(
                "This group has reached its membership capacity.",
            ));
        }
    }
    Ok(())
}
async fn activate(c: &mut PgConnection, user: Uuid, group: Uuid) -> AppResult<()> {
    require_handle(c, user).await?;
    let state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM group_memberships WHERE group_id=$1 AND user_id=$2",
    )
    .bind(group)
    .bind(user)
    .fetch_optional(&mut *c)
    .await?;
    if state.as_deref() == Some("active") {
        return Ok(());
    }
    if state.as_deref() == Some("removed") {
        return Err(AppError::Forbidden(
            "The owner must allow a new invitation after removal.",
        ));
    }
    membership_capacity(c, user, Some(group)).await?;
    sqlx::query("INSERT INTO group_memberships(id,group_id,user_id) VALUES($1,$2,$3) ON CONFLICT(group_id,user_id) DO UPDATE SET id=EXCLUDED.id,state='active',role='member',share_progress=FALSE,discord_enabled=FALSE,version=group_memberships.version+1")
        .bind(Uuid::now_v7()).bind(group).bind(user).execute(c).await?;
    Ok(())
}
async fn leaders_capacity(c: &mut PgConnection, group: Uuid) -> AppResult<()> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM group_memberships m JOIN groups g ON g.id=m.group_id WHERE m.group_id=$1 AND m.state='active' AND (m.role='leader' OR m.user_id=g.owner_id)").bind(group).fetch_one(c).await?;
    if count >= 10 {
        return Err(AppError::Forbidden(
            "Leader limit reached (10, including the owner).",
        ));
    }
    Ok(())
}
async fn deactivate(
    c: &mut PgConnection,
    group: Uuid,
    membership: Uuid,
    version: i32,
    state: &str,
) -> AppResult<()> {
    let user=sqlx::query_scalar::<_,Uuid>("UPDATE group_memberships SET state=$4,role='member',share_progress=FALSE,discord_enabled=FALSE,version=version+1 WHERE group_id=$1 AND id=$2 AND version=$3 AND state='active' RETURNING user_id")
        .bind(group).bind(membership).bind(version).bind(state).fetch_optional(&mut *c).await?.ok_or(AppError::Conflict("Membership changed. Refresh before continuing.".into()))?;
    sqlx::query("DELETE FROM group_ownership_offers WHERE group_id=$1 AND to_membership_id=$2")
        .bind(group)
        .bind(membership)
        .execute(&mut *c)
        .await?;
    sqlx::query("UPDATE group_invitations SET revoked_at=NOW(),version=version+1 WHERE group_id=$1 AND issuer_id=$2 AND revoked_at IS NULL").bind(group).bind(user).execute(&mut *c).await?;
    sqlx::query("DELETE FROM group_join_requests WHERE group_id=$1 AND user_id=$2")
        .bind(group)
        .bind(user)
        .execute(c)
        .await?;
    Ok(())
}

impl Store {
    pub(crate) async fn cleanup_groups(&self) -> AppResult<()> {
        sqlx::query("DELETE FROM groups WHERE id IN (SELECT id FROM groups WHERE undo_until<=NOW() ORDER BY undo_until LIMIT 25 FOR UPDATE SKIP LOCKED)").execute(&self.pool).await?;
        sqlx::query("DELETE FROM group_invitations WHERE id IN (SELECT id FROM group_invitations WHERE expires_at<=NOW() OR revoked_at<NOW()-INTERVAL '15 minutes' ORDER BY expires_at LIMIT 100 FOR UPDATE SKIP LOCKED)").execute(&self.pool).await?;
        sqlx::query("DELETE FROM group_ownership_offers WHERE expires_at<=NOW()")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn list_groups(&self, user: Uuid) -> AppResult<Vec<GroupSummary>> {
        Ok(sqlx::query_as(&format!(
            "{SUMMARY} WHERE g.deleted_at IS NULL OR g.owner_id=$1 ORDER BY g.name,g.id LIMIT 25"
        ))
        .bind(user)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn create_group(&self, user: Uuid, body: CreateGroup) -> AppResult<GroupSummary> {
        self.cleanup_groups().await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(1886547816,2)")
            .execute(&mut *tx)
            .await?;
        lock_users(&mut tx, vec![user]).await?;
        require_handle(&mut tx, user).await?;
        let name = valid_group(&mut tx, &body.name, &body.timezone).await?;
        let (total, owned): (i64, i64) =
            sqlx::query_as("SELECT COUNT(*),COUNT(*) FILTER(WHERE owner_id=$1) FROM groups")
                .bind(user)
                .fetch_one(&mut *tx)
                .await?;
        if total >= 100 || owned >= 5 {
            return Err(AppError::Forbidden(
                "Group creation limit reached (5 owned, 100 hosted).",
            ));
        }
        membership_capacity(&mut tx, user, None).await?;
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,owner_id,name,timezone) VALUES($1,$2,$3,$4)")
            .bind(id)
            .bind(user)
            .bind(name)
            .bind(body.timezone)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO group_memberships(id,group_id,user_id,role) VALUES($1,$2,$3,'leader')",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(user)
        .execute(&mut *tx)
        .await?;
        let result = summary(&mut tx, user, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn group_detail(&self, user: Uuid, id: Uuid) -> AppResult<GroupDetail> {
        // One read snapshot prevents removal between authorizing and projecting
        // roster/invitation rows. No progress fields or emails are selected.
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let group = summary(&mut tx, user, id).await?;
        if group.deleted_at.is_some() {
            return Err(AppError::NotFound("group"));
        }
        let members = sqlx::query_as(&format!(
            "{MEMBER} WHERE m.group_id=$1 AND m.state='active' ORDER BY h.handle LIMIT 100"
        ))
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let leader = group.my_role != "member";
        let owner = group.my_role == "owner";
        let removed_members = if owner {
            sqlx::query_as(&format!(
                "{MEMBER} WHERE m.group_id=$1 AND m.state='removed' ORDER BY h.handle LIMIT 500"
            ))
            .bind(id)
            .fetch_all(&mut *tx)
            .await?
        } else {
            vec![]
        };
        let invitations = if leader {
            sqlx::query_as(&format!("{INVITATION} WHERE i.group_id=$1 AND i.revoked_at IS NULL AND i.expires_at>NOW() AND i.uses<i.max_uses ORDER BY i.expires_at LIMIT 50")).bind(id).fetch_all(&mut *tx).await?
        } else {
            vec![]
        };
        let join_requests = if leader {
            sqlx::query_as("SELECT r.id,r.user_id,h.handle,LEFT(u.display_name,120) AS display_name,r.version FROM group_join_requests r JOIN users u ON u.id=r.user_id JOIN user_handles h ON h.user_id=r.user_id JOIN group_invitations i ON i.id=r.invitation_id AND i.group_id=r.group_id WHERE r.group_id=$1 AND r.state='pending' AND i.version=r.invitation_version AND i.revoked_at IS NULL AND i.expires_at>NOW() AND i.uses<i.max_uses ORDER BY r.updated_at LIMIT 100").bind(id).fetch_all(&mut *tx).await?
        } else {
            vec![]
        };
        let ownership_offer=sqlx::query_as("SELECT o.id,o.to_membership_id,o.expires_at,o.group_version FROM group_ownership_offers o JOIN group_memberships m ON m.id=o.to_membership_id WHERE o.group_id=$1 AND o.expires_at>NOW() AND ($2 OR m.user_id=$3)").bind(id).bind(owner).bind(user).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(GroupDetail {
            group,
            members,
            removed_members,
            invitations,
            join_requests,
            ownership_offer,
        })
    }
    pub async fn mutate_group(
        &self,
        user: Uuid,
        id: Uuid,
        body: GroupMutation,
    ) -> AppResult<GroupMutationResult> {
        let mut tx = self.pool.begin().await?;
        // Admission/ownership may reference a second account. Lock all affected
        // users in UUID order before the group, matching account-erasure ordering.
        let mut users = vec![user];
        match &body.action {
            GroupAction::ReviewJoin { request_id, .. } => {
                if let Some(target) = sqlx::query_scalar::<_, Uuid>(
                    "SELECT user_id FROM group_join_requests WHERE id=$1 AND group_id=$2",
                )
                .bind(request_id)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                {
                    users.push(target);
                }
            }
            GroupAction::AcceptOwnership { .. } => {
                if let Some(owner) =
                    sqlx::query_scalar::<_, Uuid>("SELECT owner_id FROM groups WHERE id=$1")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await?
                {
                    users.push(owner);
                }
            }
            _ => {}
        }
        lock_users(&mut tx, users.clone()).await?;
        let a = access(&mut tx, user, id).await?;
        a.version(body.expected_version)?;
        if matches!(&body.action, GroupAction::AcceptOwnership { .. })
            && !users.contains(&a.owner_id)
        {
            return Err(AppError::Conflict(
                "Ownership changed. Refresh before accepting.".into(),
            ));
        }
        if a.deleted_at.is_some() && !matches!(&body.action, GroupAction::Restore) {
            return Err(AppError::Conflict(
                "Restore the group before making changes.".into(),
            ));
        }
        let mut departed = false;
        match body.action {
            GroupAction::Update {
                name,
                timezone,
                approval_required,
            } => {
                a.owner(user)?;
                let name = valid_group(&mut tx, &name, &timezone).await?;
                sqlx::query("UPDATE groups SET name=$2,timezone=$3,approval_required=$4,version=version+1 WHERE id=$1").bind(id).bind(name).bind(timezone).bind(approval_required).execute(&mut *tx).await?;
            }
            GroupAction::SetRole {
                membership_id,
                expected_membership_version,
                role,
            } => {
                a.owner(user)?;
                if !matches!(role.as_str(), "member" | "leader") {
                    return Err(AppError::Validation("Choose member or leader.".into()));
                }
                let target:(Uuid,String)=sqlx::query_as("SELECT user_id,role FROM group_memberships WHERE group_id=$1 AND id=$2 AND version=$3 AND state='active'").bind(id).bind(membership_id).bind(expected_membership_version).fetch_optional(&mut *tx).await?.ok_or(AppError::Conflict("Membership changed. Refresh before continuing.".into()))?;
                if target.0 == a.owner_id {
                    return Err(AppError::Conflict(
                        "Transfer ownership instead of changing the owner's role.".into(),
                    ));
                }
                if role == "leader" && target.1 != "leader" {
                    leaders_capacity(&mut tx, id).await?;
                }
                sqlx::query("UPDATE group_memberships SET role=$3,version=version+1 WHERE group_id=$1 AND id=$2").bind(id).bind(membership_id).bind(&role).execute(&mut *tx).await?;
                if role == "member" {
                    sqlx::query("UPDATE group_invitations SET revoked_at=NOW(),version=version+1 WHERE group_id=$1 AND issuer_id=$2 AND revoked_at IS NULL").bind(id).bind(target.0).execute(&mut *tx).await?;
                }
            }
            GroupAction::Remove {
                membership_id,
                expected_membership_version,
            } => {
                a.owner(user)?;
                if membership_id == a.my_membership_id {
                    return Err(AppError::Conflict(
                        "Transfer ownership before leaving.".into(),
                    ));
                }
                deactivate(
                    &mut tx,
                    id,
                    membership_id,
                    expected_membership_version,
                    "removed",
                )
                .await?;
            }
            GroupAction::Leave {
                expected_membership_version,
            } => {
                if a.owner_id == user {
                    return Err(AppError::Conflict(
                        "Transfer ownership before leaving.".into(),
                    ));
                }
                deactivate(
                    &mut tx,
                    id,
                    a.my_membership_id,
                    expected_membership_version,
                    "left",
                )
                .await?;
                departed = true;
            }
            GroupAction::AllowRejoin {
                membership_id,
                expected_membership_version,
            } => {
                a.owner(user)?;
                let changed=sqlx::query("UPDATE group_memberships SET state='left',version=version+1 WHERE group_id=$1 AND id=$2 AND version=$3 AND state='removed'").bind(id).bind(membership_id).bind(expected_membership_version).execute(&mut *tx).await?;
                if changed.rows_affected() != 1 {
                    return Err(AppError::Conflict(
                        "Membership changed. Refresh before continuing.".into(),
                    ));
                }
            }
            GroupAction::ReviewJoin {
                request_id,
                expected_request_version,
                approve,
            } => {
                a.leader(user)?;
                let (target,invite,version):(Uuid,Uuid,i32)=sqlx::query_as("SELECT user_id,invitation_id,invitation_version FROM group_join_requests WHERE group_id=$1 AND id=$2 AND version=$3 AND state='pending'").bind(id).bind(request_id).bind(expected_request_version).fetch_optional(&mut *tx).await?.ok_or(AppError::Conflict("Join request changed. Refresh before continuing.".into()))?;
                if approve {
                    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM group_invitations WHERE id=$1 AND group_id=$2 AND version=$3 AND revoked_at IS NULL AND expires_at>NOW() AND uses<max_uses)").bind(invite).bind(id).bind(version).fetch_one(&mut *tx).await?;
                    if !valid {
                        return Err(AppError::Conflict(
                            "The invitation is no longer valid.".into(),
                        ));
                    }
                    activate(&mut tx, target, id).await?;
                    consume_invite(&mut tx, invite).await?;
                }
                sqlx::query("UPDATE group_join_requests SET state=$2,version=version+1,updated_at=NOW() WHERE id=$1").bind(request_id).bind(if approve{"approved"}else{"denied"}).execute(&mut *tx).await?;
            }
            GroupAction::OfferOwnership { membership_id } => {
                a.owner(user)?;
                let target:Uuid=sqlx::query_scalar("SELECT user_id FROM group_memberships WHERE group_id=$1 AND id=$2 AND state='active'").bind(id).bind(membership_id).fetch_optional(&mut *tx).await?.ok_or(AppError::NotFound("member"))?;
                if target == user {
                    return Err(AppError::Validation("Choose another active member.".into()));
                }
                sqlx::query("INSERT INTO group_ownership_offers(id,group_id,from_user_id,to_membership_id,group_version,expires_at) VALUES($1,$2,$3,$4,$5,NOW()+INTERVAL '1 day') ON CONFLICT(group_id) DO UPDATE SET id=EXCLUDED.id,from_user_id=EXCLUDED.from_user_id,to_membership_id=EXCLUDED.to_membership_id,group_version=EXCLUDED.group_version,expires_at=EXCLUDED.expires_at").bind(Uuid::now_v7()).bind(id).bind(user).bind(membership_id).bind(a.version).execute(&mut *tx).await?;
            }
            GroupAction::AcceptOwnership { offer_id } => {
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM group_ownership_offers WHERE group_id=$1 AND id=$2 AND from_user_id=$3 AND to_membership_id=$4 AND group_version=$5 AND expires_at>NOW())").bind(id).bind(offer_id).bind(a.owner_id).bind(a.my_membership_id).bind(a.version).fetch_one(&mut *tx).await?;
                if !valid {
                    return Err(AppError::NotFound("ownership offer"));
                }
                if a.my_role != "leader" {
                    leaders_capacity(&mut tx, id).await?;
                }
                sqlx::query(
                    "UPDATE group_memberships SET role='leader',version=version+1 WHERE id=$1",
                )
                .bind(a.my_membership_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE groups SET owner_id=$2,version=version+1 WHERE id=$1")
                    .bind(id)
                    .bind(user)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("DELETE FROM group_ownership_offers WHERE group_id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            GroupAction::CancelOwnership => {
                a.owner(user)?;
                sqlx::query("DELETE FROM group_ownership_offers WHERE group_id=$1")
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
            }
            GroupAction::Delete { confirmation } => {
                a.owner(user)?;
                if confirmation != "DELETE GROUP" {
                    return Err(AppError::Validation("Confirm with DELETE GROUP.".into()));
                }
                sqlx::query("UPDATE groups SET deleted_at=NOW(),undo_until=NOW()+INTERVAL '60 seconds',deleted_membership_epoch=membership_epoch,version=version+1 WHERE id=$1").bind(id).execute(&mut *tx).await?;
            }
            GroupAction::Restore => {
                a.owner(user)?;
                let changed=sqlx::query("UPDATE groups SET deleted_at=NULL,undo_until=NULL,deleted_membership_epoch=NULL,version=version+1 WHERE id=$1 AND undo_until>NOW() AND deleted_membership_epoch=membership_epoch").bind(id).execute(&mut *tx).await?;
                if changed.rows_affected() != 1 {
                    return Err(AppError::Conflict("Undo expired or group membership changed. The group cannot be restored safely.".into()));
                }
            }
        }
        let group = if departed {
            None
        } else {
            Some(summary(&mut tx, user, id).await?)
        };
        tx.commit().await?;
        Ok(GroupMutationResult { group })
    }
}

#[derive(FromRow)]
struct InvitationAccess {
    id: Uuid,
    group_id: Uuid,
    kind: String,
    version: i32,
    valid: bool,
    group_name: String,
    approval_required: bool,
}
async fn invitation(
    c: &mut PgConnection,
    user: Uuid,
    reference: GroupInviteReference,
) -> AppResult<InvitationAccess> {
    let (id, digest) = match (reference.invitation_id, reference.credential) {
        (Some(id), None) => (Some(id), None),
        (None, Some(value)) => {
            let value = value.trim();
            if value.len() > 128 {
                return Err(AppError::NotFound("invitation"));
            }
            let canonical = if value.len() == 43
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                value.to_owned()
            } else {
                let code: String = value
                    .chars()
                    .filter(|c| !matches!(c, ' ' | '-'))
                    .map(|c| c.to_ascii_uppercase())
                    .collect();
                if code.len() != 10
                    || !code
                        .bytes()
                        .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
                {
                    return Err(AppError::NotFound("invitation"));
                }
                code
            };
            (None, Some(Sha256::digest(canonical.as_bytes()).to_vec()))
        }
        _ => {
            return Err(AppError::Validation(
                "Provide one invitation ID or credential.".into(),
            ));
        }
    };
    sqlx::query_as("SELECT i.id,i.group_id,i.kind,i.version,(i.revoked_at IS NULL AND i.expires_at>NOW() AND i.uses<i.max_uses) AS valid,g.name AS group_name,g.approval_required FROM group_invitations i JOIN groups g ON g.id=i.group_id WHERE g.deleted_at IS NULL AND (($1::uuid IS NOT NULL AND i.id=$1 AND i.kind='direct' AND i.recipient_id=$3) OR ($2::bytea IS NOT NULL AND i.secret_hash=$2))")
        .bind(id).bind(digest).bind(user).fetch_optional(c).await?.ok_or(AppError::NotFound("invitation"))
}
async fn consume_invite(c: &mut PgConnection, id: Uuid) -> AppResult<()> {
    let changed=sqlx::query("UPDATE group_invitations SET uses=uses+1,revoked_at=CASE WHEN uses+1=max_uses THEN NOW() ELSE revoked_at END WHERE id=$1 AND revoked_at IS NULL AND expires_at>NOW() AND uses<max_uses").bind(id).execute(c).await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "The invitation is no longer valid.".into(),
        ));
    }
    Ok(())
}
impl Store {
    pub async fn issue_group_invitation(
        &self,
        user: Uuid,
        group: Uuid,
        body: IssueGroupInvitation,
    ) -> AppResult<IssuedGroupInvitation> {
        self.cleanup_groups().await?;
        let mut tx = self.pool.begin().await?;
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups g JOIN group_memberships m ON m.group_id=g.id WHERE g.id=$1 AND g.deleted_at IS NULL AND m.user_id=$2 AND m.state='active' AND (g.owner_id=$2 OR m.role='leader'))").bind(group).bind(user).fetch_one(&mut *tx).await?;
        if !allowed {
            return Err(AppError::Forbidden("A group leader is required."));
        }
        let target = if body.kind == "direct" {
            let value = body.handle.as_deref().unwrap_or("").trim();
            let value = value.strip_prefix('@').unwrap_or(value);
            if !(3..=30).contains(&value.len())
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(AppError::Validation("Enter an exact @handle.".into()));
            }
            sqlx::query_scalar::<_, Uuid>(
                "SELECT user_id FROM user_handles WHERE handle=$1 AND user_id IS NOT NULL",
            )
            .bind(value.to_ascii_lowercase())
            .fetch_optional(&mut *tx)
            .await?
        } else {
            None
        };
        let mut users = vec![user];
        if let Some(target) = target {
            users.push(target);
        }
        lock_users(&mut tx, users).await?;
        let a = access(&mut tx, user, group).await?;
        a.leader(user)?;
        a.version(body.expected_version)?;
        if a.deleted_at.is_some() {
            return Err(AppError::NotFound("group"));
        }
        if !matches!(body.kind.as_str(), "direct" | "link" | "code")
            || !(1..=100).contains(&body.max_uses)
        {
            return Err(AppError::Validation(
                "Choose a valid invitation kind and 1-100 uses.".into(),
            ));
        }
        let recipient = if body.kind == "direct" {
            if body.max_uses != 1 {
                return Err(AppError::Validation(
                    "Direct invitations have one recipient.".into(),
                ));
            }
            let target = target.ok_or(AppError::NotFound("handle"))?;
            if target == user {
                return Err(AppError::Validation("You are already a member.".into()));
            }
            let state = sqlx::query_scalar::<_, String>(
                "SELECT state FROM group_memberships WHERE group_id=$1 AND user_id=$2",
            )
            .bind(group)
            .bind(target)
            .fetch_optional(&mut *tx)
            .await?;
            if state.as_deref() == Some("active") {
                return Err(AppError::Conflict("That user is already a member.".into()));
            }
            if state.as_deref() == Some("removed") {
                return Err(AppError::Forbidden(
                    "The owner must allow future invitations for this removed member first.",
                ));
            }
            sqlx::query("UPDATE group_invitations SET revoked_at=NOW(),version=version+1 WHERE group_id=$1 AND recipient_id=$2 AND revoked_at IS NULL").bind(group).bind(target).execute(&mut *tx).await?;
            Some(target)
        } else {
            if body.handle.is_some() {
                return Err(AppError::Validation(
                    "Only direct invitations accept a handle.".into(),
                ));
            }
            None
        };
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM group_invitations WHERE group_id=$1")
                .bind(group)
                .fetch_one(&mut *tx)
                .await?;
        if count >= 50 {
            return Err(AppError::Forbidden(
                "Invitation capacity reached (50 per group); revoke unused invitations and wait for cleanup.",
            ));
        }
        let credential = match body.kind.as_str() {
            "direct" => None,
            "link" => Some(oauth2::CsrfToken::new_random_len(32).secret().to_owned()),
            _ => {
                let random = oauth2::CsrfToken::new_random_len(32);
                let bytes = Sha256::digest(random.secret().as_bytes());
                Some(
                    bytes[..10]
                        .iter()
                        .map(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ"[usize::from(b & 31)] as char)
                        .collect(),
                )
            }
        };
        let hash = credential
            .as_ref()
            .map(|s| Sha256::digest(s.as_bytes()).to_vec());
        let id = Uuid::now_v7();
        // A collision never overwrites another group's credential.
        let inserted=sqlx::query("INSERT INTO group_invitations(id,group_id,issuer_id,kind,recipient_id,secret_hash,expires_at,max_uses) VALUES($1,$2,$3,$4,$5,$6,NOW()+INTERVAL '7 days',$7) ON CONFLICT(secret_hash) DO NOTHING")
            .bind(id).bind(group).bind(user).bind(body.kind).bind(recipient).bind(hash).bind(body.max_uses).execute(&mut *tx).await?;
        if inserted.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Could not issue a unique invitation. Please try again.".into(),
            ));
        }
        let invitation = sqlx::query_as(&format!("{INVITATION} WHERE i.id=$1"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(IssuedGroupInvitation {
            invitation,
            credential,
        })
    }
    pub async fn revoke_group_invitation(
        &self,
        user: Uuid,
        group: Uuid,
        invite: Uuid,
        version: i32,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, vec![user]).await?;
        let a = access(&mut tx, user, group).await?;
        a.leader(user)?;
        if a.deleted_at.is_some() {
            return Err(AppError::NotFound("group"));
        }
        let changed=sqlx::query("UPDATE group_invitations SET revoked_at=NOW(),version=version+1 WHERE group_id=$1 AND id=$2 AND version=$3 AND revoked_at IS NULL").bind(group).bind(invite).bind(version).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Invitation changed. Refresh before continuing.".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn group_invitation_inbox(&self, user: Uuid) -> AppResult<GroupInbox> {
        let invitations=sqlx::query_as(&format!("{INVITATION} WHERE i.recipient_id=$1 AND i.revoked_at IS NULL AND i.expires_at>NOW() AND i.uses<i.max_uses AND g.deleted_at IS NULL ORDER BY i.expires_at LIMIT 100")).bind(user).fetch_all(&self.pool).await?;
        let requests=sqlx::query_as("SELECT r.id,r.group_id,g.name AS group_name,CASE WHEN i.revoked_at IS NOT NULL OR i.expires_at<=NOW() OR i.version<>r.invitation_version OR i.uses>=i.max_uses THEN 'closed' ELSE r.state END AS state,r.version FROM group_join_requests r JOIN groups g ON g.id=r.group_id JOIN group_invitations i ON i.id=r.invitation_id AND i.group_id=r.group_id WHERE r.user_id=$1 AND r.state IN ('pending','denied') AND g.deleted_at IS NULL ORDER BY r.updated_at LIMIT 100").bind(user).fetch_all(&self.pool).await?;
        Ok(GroupInbox {
            invitations,
            requests,
        })
    }
    pub async fn decline_group_invitation(
        &self,
        user: Uuid,
        id: Uuid,
        version: i32,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, vec![user]).await?;
        let group:Uuid=sqlx::query_scalar("SELECT group_id FROM group_invitations WHERE id=$1 AND recipient_id=$2 AND kind='direct'").bind(id).bind(user).fetch_optional(&mut *tx).await?.ok_or(AppError::NotFound("invitation"))?;
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM groups WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(group)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound("group"))?;
        let changed=sqlx::query("UPDATE group_invitations SET revoked_at=NOW(),version=version+1 WHERE id=$1 AND recipient_id=$2 AND version=$3 AND revoked_at IS NULL AND uses=0").bind(id).bind(user).bind(version).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Invitation changed. Refresh before continuing.".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn preview_group_invitation(
        &self,
        user: Uuid,
        reference: GroupInviteReference,
    ) -> AppResult<GroupInvitePreview> {
        let mut c = self.pool.acquire().await?;
        let i = invitation(&mut c, user, reference).await?;
        if !i.valid {
            return Err(AppError::NotFound("invitation"));
        }
        Ok(GroupInvitePreview {
            group_id: i.group_id,
            group_name: i.group_name,
            approval_required: i.approval_required && i.kind != "direct",
        })
    }
    pub async fn join_group(&self, user: Uuid, body: JoinGroup) -> AppResult<GroupJoinResult> {
        if !body.confirm_roster_visibility {
            return Err(AppError::Validation(
                "Review who can see your membership before joining.".into(),
            ));
        }
        let reference = GroupInviteReference {
            invitation_id: body.invitation_id,
            credential: body.credential,
        };
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, vec![user]).await?;
        require_handle(&mut tx, user).await?;
        let i = invitation(&mut tx, user, reference).await?;
        // Lock the group before rechecking credential state, capacity and access.
        let approval: bool = sqlx::query_scalar(
            "SELECT approval_required FROM groups WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(i.group_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound("group"))?;
        let state = sqlx::query_scalar::<_, String>(
            "SELECT state FROM group_memberships WHERE group_id=$1 AND user_id=$2",
        )
        .bind(i.group_id)
        .bind(user)
        .fetch_optional(&mut *tx)
        .await?;
        if state.as_deref() == Some("active") {
            tx.commit().await?;
            return Ok(GroupJoinResult {
                status: "joined".into(),
                group_id: i.group_id,
            });
        }
        if state.as_deref() == Some("removed") {
            return Err(AppError::Forbidden(
                "The owner must allow future invitations after removal.",
            ));
        }
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM group_invitations WHERE id=$1 AND version=$2 AND revoked_at IS NULL AND expires_at>NOW() AND uses<max_uses)").bind(i.id).bind(i.version).fetch_one(&mut *tx).await?;
        if !valid {
            return Err(AppError::NotFound("invitation"));
        }
        let status = if i.kind != "direct" && approval {
            let previous:Option<(String,bool,Uuid,i32)>=sqlx::query_as("SELECT state,updated_at>NOW()-INTERVAL '10 minutes',invitation_id,invitation_version FROM group_join_requests WHERE group_id=$1 AND user_id=$2").bind(i.group_id).bind(user).fetch_optional(&mut *tx).await?;
            if previous
                .as_ref()
                .is_some_and(|(state, recent, _, _)| state == "denied" && *recent)
            {
                return Err(AppError::Conflict(
                    "Your request was declined recently. Wait before requesting again.".into(),
                ));
            }
            if !previous
                .as_ref()
                .is_some_and(|(state, _, invite, version)| {
                    state == "pending" && *invite == i.id && *version == i.version
                })
            {
                if previous.is_none() {
                    let count: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM group_join_requests WHERE group_id=$1",
                    )
                    .bind(i.group_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if count >= 100 {
                        return Err(AppError::Forbidden(
                            "Join request capacity reached. Contact a leader for a direct invitation.",
                        ));
                    }
                }
                sqlx::query("INSERT INTO group_join_requests(id,group_id,user_id,invitation_id,invitation_version) VALUES($1,$2,$3,$4,$5) ON CONFLICT(group_id,user_id) DO UPDATE SET id=EXCLUDED.id,invitation_id=EXCLUDED.invitation_id,invitation_version=EXCLUDED.invitation_version,state='pending',version=group_join_requests.version+1,updated_at=NOW()")
                    .bind(Uuid::now_v7()).bind(i.group_id).bind(user).bind(i.id).bind(i.version).execute(&mut *tx).await?;
            }
            "pending"
        } else {
            activate(&mut tx, user, i.group_id).await?;
            consume_invite(&mut tx, i.id).await?;
            sqlx::query("DELETE FROM group_join_requests WHERE group_id=$1 AND user_id=$2")
                .bind(i.group_id)
                .bind(user)
                .execute(&mut *tx)
                .await?;
            "joined"
        };
        tx.commit().await?;
        Ok(GroupJoinResult {
            status: status.into(),
            group_id: i.group_id,
        })
    }
}
