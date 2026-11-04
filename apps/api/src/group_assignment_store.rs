use crate::{
    error::{AppError, AppResult},
    group_assignments::*,
    group_store::{lock_users, read_access, write_access},
    models::PageQuery,
    store::Store,
};
use chrono::{DateTime, NaiveDate, NaiveTime, Timelike, Utc};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

macro_rules! assignment_sql {($suffix:literal)=>{concat!("SELECT a.id,a.group_id,a.title,a.description,a.due_on,a.due_time::text AS due_time,a.timezone,(a.due_on+a.due_time) AT TIME ZONE a.timezone AS due_at,a.state,a.version,p.completed_at,COALESCE(p.version,0) AS progress_version,p.seen_assignment_version FROM group_assignments a LEFT JOIN group_assignment_progress p ON p.assignment_id=a.id AND p.group_id=a.group_id AND p.membership_id=$1",$suffix)};}
#[derive(FromRow)]
struct AssignmentRow {
    id: Uuid,
    group_id: Uuid,
    title: String,
    description: String,
    due_on: Option<NaiveDate>,
    due_time: Option<String>,
    timezone: String,
    due_at: Option<DateTime<Utc>>,
    state: String,
    version: i32,
    completed_at: Option<DateTime<Utc>>,
    progress_version: i32,
    seen_assignment_version: Option<i32>,
}
impl From<AssignmentRow> for GroupAssignment {
    fn from(a: AssignmentRow) -> Self {
        let changed =
            a.completed_at.is_some() && a.seen_assignment_version.is_some_and(|v| v < a.version);
        Self {
            id: a.id,
            group_id: a.group_id,
            title: a.title,
            description: a.description,
            due_on: a.due_on,
            due_time: a.due_time,
            timezone: a.timezone,
            due_at: a.due_at,
            state: a.state,
            version: a.version,
            my_progress: AssignmentProgress {
                completed_at: a.completed_at,
                version: a.progress_version,
                seen_assignment_version: a.seen_assignment_version,
            },
            changed_since_completion: changed,
        }
    }
}
async fn assignment(
    c: &mut PgConnection,
    membership: Uuid,
    group: Uuid,
    id: Uuid,
) -> AppResult<GroupAssignment> {
    let row: AssignmentRow = sqlx::query_as(assignment_sql!(
        " WHERE a.group_id=$2 AND a.id=$3 AND a.deleted_at IS NULL"
    ))
    .bind(membership)
    .bind(group)
    .bind(id)
    .fetch_optional(c)
    .await?
    .ok_or(AppError::NotFound("assignment"))?;
    Ok(row.into())
}
async fn fields(
    c: &mut PgConnection,
    title: &str,
    description: &str,
    date: Option<NaiveDate>,
    time: Option<&str>,
    timezone: &str,
) -> AppResult<(String, Option<NaiveTime>)> {
    let title = title.trim();
    if title.is_empty()
        || title.chars().count() > 240
        || title.chars().any(char::is_control)
        || description.chars().count() > 10000
        || description.contains('\0')
    {
        return Err(AppError::Validation(
            "Use a 1-240 character title on one line and a description up to 10,000 characters."
                .into(),
        ));
    }
    if timezone.len() > 64
        || !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name=$1)",
        )
        .bind(timezone)
        .fetch_one(&mut *c)
        .await?
    {
        return Err(AppError::Validation("Choose a valid IANA timezone.".into()));
    }
    let deadline = match (date, time) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(AppError::Validation("A deadline time needs a date.".into()));
        }
        (Some(date), value) => {
            if date < NaiveDate::from_ymd_opt(1900, 1, 1).unwrap()
                || date > NaiveDate::from_ymd_opt(9999, 12, 31).unwrap()
            {
                return Err(AppError::Validation(
                    "Choose a deadline between 1900 and 9999.".into(),
                ));
            }
            let value = value.unwrap_or("23:59");
            let time = NaiveTime::parse_from_str(value, "%H:%M")
                .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M:%S"))
                .map_err(|_| AppError::Validation("Choose a valid deadline time.".into()))?;
            if time.nanosecond() != 0 {
                return Err(AppError::Validation(
                    "Choose a deadline without leap seconds.".into(),
                ));
            }
            let valid:bool=sqlx::query_scalar("SELECT ((($1::date+$2::time) AT TIME ZONE $3) AT TIME ZONE $3)=($1::date+$2::time)").bind(date).bind(time).bind(timezone).fetch_one(c).await?;
            if !valid {
                return Err(AppError::Validation("That local time does not exist because the timezone clock changes. Choose another time.".into()));
            }
            Some(time)
        }
    };
    Ok((title.into(), deadline))
}
fn version(actual: i32, expected: i32) -> AppResult<()> {
    if actual != expected {
        return Err(AppError::Conflict(
            "The assignment or your progress changed. Refresh before continuing.".into(),
        ));
    }
    Ok(())
}
async fn preferences(
    c: &mut PgConnection,
    membership: Uuid,
) -> AppResult<GroupProgressPreferences> {
    Ok(sqlx::query_as("SELECT id AS membership_id,preferences_version AS version,share_progress,progress_notice_version AS notice_version,progress_shared_at AS shared_at FROM group_memberships WHERE id=$1 AND state='active'")
        .bind(membership).fetch_one(c).await?)
}

#[derive(FromRow)]
pub(crate) struct AssignmentExportRow {
    pub(crate) id: Uuid,
    pub(crate) value: serde_json::Value,
}
impl Store {
    pub(crate) async fn group_assignment_export_page(
        &self,
        user: Uuid,
        cursor: Option<Uuid>,
    ) -> AppResult<Vec<AssignmentExportRow>> {
        // Recheck current membership on every page, including after a long/slow download.
        Ok(sqlx::query_as("SELECT a.id,jsonb_build_object('group_name',g.name,'assignment',to_jsonb(a)-ARRAY['deleted_at','undo_until','deleted_membership_epoch'],'my_progress',jsonb_build_object('completed_at',p.completed_at,'version',COALESCE(p.version,0),'seen_assignment_version',p.seen_assignment_version)) AS value FROM group_assignments a JOIN groups g ON g.id=a.group_id JOIN group_memberships m ON m.group_id=g.id AND m.user_id=$1 AND m.state='active' LEFT JOIN group_assignment_progress p ON p.assignment_id=a.id AND p.membership_id=m.id WHERE g.deleted_at IS NULL AND a.deleted_at IS NULL AND ($2::uuid IS NULL OR a.id>$2) ORDER BY a.id LIMIT 100")
            .bind(user).bind(cursor).fetch_all(&self.pool).await?)
    }
    pub async fn group_assignments(
        &self,
        user: Uuid,
        group: Uuid,
        page: GroupAssignmentQuery,
    ) -> AppResult<GroupAssignmentPage> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let a = read_access(&mut tx, user, group).await?;
        let limit = PageQuery {
            cursor: page.cursor,
            limit: page.limit,
        }
        .normalized_limit();
        let mut rows:Vec<AssignmentRow>=sqlx::query_as(assignment_sql!(" WHERE a.group_id=$2 AND a.deleted_at IS NULL AND a.state=$5 AND ($3::uuid IS NULL OR a.id<$3) ORDER BY a.id DESC LIMIT $4"))
            .bind(a.my_membership_id).bind(group).bind(page.cursor).bind(limit+1).bind(page.state.as_str()).fetch_all(&mut *tx).await?;
        let more = rows.len() as i64 > limit;
        if more {
            rows.pop();
        }
        let next_cursor = if more {
            rows.last().map(|r| r.id)
        } else {
            None
        };
        tx.commit().await?;
        Ok(GroupAssignmentPage {
            items: rows.into_iter().map(Into::into).collect(),
            next_cursor,
        })
    }
    pub async fn create_group_assignment(
        &self,
        user: Uuid,
        group: Uuid,
        body: CreateGroupAssignment,
    ) -> AppResult<GroupAssignment> {
        self.cleanup_groups().await?;
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        let a = write_access(&mut tx, user, group).await?;
        a.leader(user)?;
        let (title, time) = fields(
            &mut tx,
            &body.title,
            &body.description,
            body.due_on,
            body.due_time.as_deref(),
            &a.timezone,
        )
        .await?;
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO group_assignments(id,group_id,title,description,due_on,due_time,timezone) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(id).bind(group).bind(title).bind(body.description).bind(body.due_on).bind(time).bind(a.timezone).execute(&mut *tx).await?;
        let result = assignment(&mut tx, a.my_membership_id, group, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn update_group_assignment(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
        body: UpdateGroupAssignment,
    ) -> AppResult<GroupAssignment> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        let a = write_access(&mut tx, user, group).await?;
        a.leader(user)?;
        let old = assignment(&mut tx, a.my_membership_id, group, id).await?;
        version(old.version, body.expected_version)?;
        if !matches!(body.state.as_str(), "open" | "closed") {
            return Err(AppError::Validation("Choose open or closed.".into()));
        }
        let timezone = body.deadline_timezone.as_deref().unwrap_or(&old.timezone);
        let (title, time) = fields(
            &mut tx,
            &body.title,
            &body.description,
            body.due_on,
            body.due_time.as_deref(),
            timezone,
        )
        .await?;
        sqlx::query("UPDATE group_assignments SET title=$3,description=$4,due_on=$5,due_time=$6,timezone=$7,state=$8,version=version+1 WHERE group_id=$1 AND id=$2")
            .bind(group).bind(id).bind(title).bind(body.description).bind(body.due_on).bind(time).bind(timezone).bind(body.state).execute(&mut *tx).await?;
        let result = assignment(&mut tx, a.my_membership_id, group, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn update_group_assignment_progress(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
        body: UpdateAssignmentProgress,
    ) -> AppResult<GroupAssignment> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        let a = write_access(&mut tx, user, group).await?;
        if a.my_membership_id != body.expected_membership_id {
            return Err(AppError::Conflict(
                "Your membership changed. Refresh before continuing.".into(),
            ));
        }
        let old = assignment(&mut tx, a.my_membership_id, group, id).await?;
        version(old.version, body.expected_assignment_version)?;
        version(old.my_progress.version, body.expected_progress_version)?;
        if body.completed || old.my_progress.version > 0 {
            sqlx::query("INSERT INTO group_assignment_progress(group_id,assignment_id,membership_id,user_id,completed_at,seen_assignment_version) VALUES($1,$2,$3,$4,CASE WHEN $5 THEN NOW() ELSE NULL END,CASE WHEN $5 THEN $6 ELSE NULL END) ON CONFLICT(assignment_id,membership_id) DO UPDATE SET completed_at=CASE WHEN $5 THEN COALESCE(group_assignment_progress.completed_at,NOW()) ELSE NULL END,seen_assignment_version=CASE WHEN $5 THEN $6 ELSE NULL END,version=group_assignment_progress.version+1")
                .bind(group).bind(id).bind(a.my_membership_id).bind(user).bind(body.completed).bind(old.version).execute(&mut *tx).await?;
        }
        let result = assignment(&mut tx, a.my_membership_id, group, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn group_progress_preferences(
        &self,
        user: Uuid,
        group: Uuid,
    ) -> AppResult<GroupProgressPreferences> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let a = read_access(&mut tx, user, group).await?;
        let result = preferences(&mut tx, a.my_membership_id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn update_group_progress_preferences(
        &self,
        user: Uuid,
        group: Uuid,
        body: UpdateGroupProgressPreferences,
    ) -> AppResult<GroupProgressPreferences> {
        if body.share_progress && body.notice_version.as_deref() != Some(PROGRESS_NOTICE_VERSION) {
            return Err(AppError::Validation(
                "Review the current progress-sharing notice before enabling it.".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        let a = write_access(&mut tx, user, group).await?;
        if a.my_membership_id != body.expected_membership_id {
            return Err(AppError::Conflict(
                "Your membership changed. Refresh before continuing.".into(),
            ));
        }
        let old = preferences(&mut tx, a.my_membership_id).await?;
        version(old.version, body.expected_version)?;
        sqlx::query("UPDATE group_memberships SET share_progress=$2,progress_notice_version=CASE WHEN $2 THEN $3 ELSE NULL END,progress_shared_at=CASE WHEN $2 THEN NOW() ELSE NULL END,preferences_version=preferences_version+1 WHERE id=$1")
            .bind(a.my_membership_id).bind(body.share_progress).bind(PROGRESS_NOTICE_VERSION).execute(&mut *tx).await?;
        let result = preferences(&mut tx, a.my_membership_id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn group_assignment_report(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
    ) -> AppResult<AssignmentProgressReport> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let a = read_access(&mut tx, user, group).await?;
        a.leader(user)?;
        assignment(&mut tx, a.my_membership_id, group, id).await?;
        let items=sqlx::query_as("SELECT m.user_id,h.handle,LEFT(u.display_name,120) AS display_name,p.completed_at,p.seen_assignment_version FROM group_memberships m JOIN users u ON u.id=m.user_id JOIN user_handles h ON h.user_id=m.user_id LEFT JOIN group_assignment_progress p ON p.membership_id=m.id AND p.group_id=m.group_id AND p.assignment_id=$2 WHERE m.group_id=$1 AND m.state='active' AND m.share_progress AND m.progress_notice_version=$3 ORDER BY h.handle LIMIT 100")
            .bind(group).bind(id).bind(PROGRESS_NOTICE_VERSION).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok(AssignmentProgressReport { items })
    }
    pub async fn delete_group_assignment(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
        expected: i32,
    ) -> AppResult<GroupAssignmentUndo> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        let a = write_access(&mut tx, user, group).await?;
        a.leader(user)?;
        let old = assignment(&mut tx, a.my_membership_id, group, id).await?;
        version(old.version, expected)?;
        let result=sqlx::query_as("UPDATE group_assignments SET deleted_at=NOW(),undo_until=NOW()+INTERVAL '60 seconds',deleted_membership_epoch=$3,version=version+1 WHERE group_id=$1 AND id=$2 RETURNING id,title,version,undo_until")
            .bind(group).bind(id).bind(a.membership_epoch).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn restore_group_assignment(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
        expected: i32,
    ) -> AppResult<GroupAssignment> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        let a = write_access(&mut tx, user, group).await?;
        a.leader(user)?;
        let restored=sqlx::query("UPDATE group_assignments SET deleted_at=NULL,undo_until=NULL,deleted_membership_epoch=NULL,version=version+1 WHERE group_id=$1 AND id=$2 AND version=$3 AND undo_until>NOW() AND deleted_membership_epoch=$4")
            .bind(group).bind(id).bind(expected).bind(a.membership_epoch).execute(&mut *tx).await?;
        if restored.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Undo expired or the assignment/membership changed. Refresh before continuing."
                    .into(),
            ));
        }
        let result = assignment(&mut tx, a.my_membership_id, group, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn group_assignment_undos(
        &self,
        user: Uuid,
        group: Uuid,
        page: PageQuery,
    ) -> AppResult<GroupAssignmentUndoPage> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let a = read_access(&mut tx, user, group).await?;
        a.leader(user)?;
        let limit = page.normalized_limit();
        let mut items:Vec<GroupAssignmentUndo>=sqlx::query_as("SELECT id,title,version,undo_until FROM group_assignments WHERE group_id=$1 AND undo_until>NOW() AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT $3")
            .bind(group).bind(page.cursor).bind(limit+1).fetch_all(&mut *tx).await?;
        let more = items.len() as i64 > limit;
        if more {
            items.pop();
        }
        let next_cursor = if more {
            items.last().map(|i| i.id)
        } else {
            None
        };
        tx.commit().await?;
        Ok(GroupAssignmentUndoPage { items, next_cursor })
    }
    pub async fn group_content_usage(
        &self,
        user: Uuid,
        group: Uuid,
    ) -> AppResult<GroupContentUsage> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        read_access(&mut tx, user, group).await?;
        let result=sqlx::query_as("SELECT COALESCE(u.bytes,0) AS used_bytes,COALESCE(u.records,0) AS used_records,l.group_bytes AS max_bytes,l.group_records AS max_records FROM workspace_content_limits l LEFT JOIN group_content_usage u ON u.group_id=$1 WHERE l.singleton")
            .bind(group).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(result)
    }
}
