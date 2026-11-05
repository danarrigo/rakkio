use crate::{
    error::{AppError, AppResult},
    group_events::*,
    group_store::{lock_users, read_access, write_access},
    models::{EventRecurrence, PageQuery},
    store::Store,
};
use chrono::{Datelike, Timelike};
use sqlx::PgConnection;
use uuid::Uuid;

macro_rules! event_sql {($suffix:literal)=>{concat!("SELECT id,group_id,title,description,location,starts_local,ends_local,all_day,timezone,recurrence,recurrence_until,version FROM group_events",$suffix)};}
async fn event(c: &mut PgConnection, group: Uuid, id: Uuid) -> AppResult<GroupEvent> {
    sqlx::query_as(event_sql!(
        " WHERE group_id=$1 AND id=$2 AND deleted_at IS NULL"
    ))
    .bind(group)
    .bind(id)
    .fetch_optional(c)
    .await?
    .ok_or(AppError::NotFound("group event"))
}
async fn timezone(c: &mut PgConnection, zone: &str) -> AppResult<()> {
    if zone.len() > 64
        || !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name=$1)",
        )
        .bind(zone)
        .fetch_one(c)
        .await?
    {
        return Err(AppError::Validation("Choose a valid IANA timezone.".into()));
    }
    Ok(())
}
async fn fields(c: &mut PgConnection, e: &GroupEventFields) -> AppResult<()> {
    if e.title.trim().is_empty()
        || e.title.trim().chars().count() > 240
        || e.title.chars().any(char::is_control)
        || e.description.chars().count() > 10000
        || e.description.contains('\0')
        || e.location.chars().count() > 500
        || e.location.chars().any(char::is_control)
    {
        return Err(AppError::Validation("Use a 1-240 character single-line title, description up to 10,000 characters and single-line location up to 500 characters.".into()));
    }
    if e.starts_local.year() < 1900
        || e.ends_local.year() > 9998
        || e.ends_local <= e.starts_local
        || e.ends_local - e.starts_local > chrono::Duration::days(31)
        || e.starts_local.nanosecond() != 0
        || e.ends_local.nanosecond() != 0
    {
        return Err(AppError::Validation("Choose whole-second event times between 1900 and 9998, lasting up to 31 days, with end after start.".into()));
    }
    timezone(c, &e.timezone).await?;
    if e.all_day {
        if e.starts_local.time() != chrono::NaiveTime::MIN
            || e.ends_local.time() != chrono::NaiveTime::MIN
        {
            return Err(AppError::Validation(
                "All-day dates must start at midnight and end at exclusive midnight.".into(),
            ));
        }
    } else {
        let valid:bool=sqlx::query_scalar("SELECT (($1::timestamp AT TIME ZONE $3) AT TIME ZONE $3)=$1 AND (($2::timestamp AT TIME ZONE $3) AT TIME ZONE $3)=$2 AND ($2::timestamp AT TIME ZONE $3)>($1::timestamp AT TIME ZONE $3)")
    .bind(e.starts_local).bind(e.ends_local).bind(&e.timezone).fetch_one(c).await?;
        if !valid {
            return Err(AppError::Validation("That local time does not exist because the timezone clock changes. Choose another time.".into()));
        }
    }
    if e.recurrence_until.is_some_and(|until| {
        e.recurrence == EventRecurrence::None
            || until < e.starts_local.date()
            || until.year() > 9998
    }) {
        return Err(AppError::Validation(
            "The inclusive final date needs a recurrence and cannot precede its first occurrence."
                .into(),
        ));
    }
    Ok(())
}
fn version(actual: i32, expected: i32) -> AppResult<()> {
    if actual != expected {
        return Err(AppError::Conflict(
            "The event series changed. Refresh before continuing.".into(),
        ));
    }
    Ok(())
}

impl Store {
    pub(crate) async fn group_event_export_page(
        &self,
        user: Uuid,
        cursor: Option<Uuid>,
    ) -> AppResult<Vec<crate::export_store::GroupExportRow>> {
        Ok(sqlx::query_as("SELECT e.id,jsonb_build_object('group_name',g.name,'event',to_jsonb(e)-ARRAY['deleted_at','undo_until','deleted_membership_epoch']) AS value FROM group_events e JOIN groups g ON g.id=e.group_id JOIN group_memberships m ON m.group_id=g.id AND m.user_id=$1 AND m.state='active' WHERE g.deleted_at IS NULL AND e.deleted_at IS NULL AND ($2::uuid IS NULL OR e.id>$2) ORDER BY e.id LIMIT 100")
            .bind(user).bind(cursor).fetch_all(&self.pool).await?)
    }
    pub async fn group_events(
        &self,
        user: Uuid,
        group: Uuid,
        page: PageQuery,
    ) -> AppResult<GroupEventPage> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        read_access(&mut tx, user, group).await?;
        let limit = page.normalized_limit();
        let mut items:Vec<GroupEvent>=sqlx::query_as(event_sql!(" WHERE group_id=$1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR id<$2) ORDER BY id DESC LIMIT $3")).bind(group).bind(page.cursor).bind(limit+1).fetch_all(&mut *tx).await?;
        let more = items.len() as i64 > limit;
        if more {
            items.pop();
        }
        let next_cursor = if more {
            items.last().map(|e| e.id)
        } else {
            None
        };
        tx.commit().await?;
        Ok(GroupEventPage { items, next_cursor })
    }
    pub async fn get_group_event(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
    ) -> AppResult<GroupEvent> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        read_access(&mut tx, user, group).await?;
        let result = event(&mut tx, group, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn create_group_event(
        &self,
        user: Uuid,
        group: Uuid,
        e: GroupEventFields,
    ) -> AppResult<GroupEvent> {
        self.cleanup_groups().await?;
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        write_access(&mut tx, user, group).await?.leader(user)?;
        fields(&mut tx, &e).await?;
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO group_events(id,group_id,title,description,location,starts_local,ends_local,all_day,timezone,recurrence,recurrence_until) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
   .bind(id).bind(group).bind(e.title.trim()).bind(e.description).bind(e.location).bind(e.starts_local).bind(e.ends_local).bind(e.all_day).bind(e.timezone).bind(e.recurrence).bind(e.recurrence_until).execute(&mut *tx).await?;
        let result = event(&mut tx, group, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn update_group_event(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
        expected: i32,
        e: GroupEventFields,
    ) -> AppResult<GroupEvent> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        write_access(&mut tx, user, group).await?.leader(user)?;
        let old = event(&mut tx, group, id).await?;
        version(old.version, expected)?;
        fields(&mut tx, &e).await?;
        sqlx::query("UPDATE group_events SET title=$3,description=$4,location=$5,starts_local=$6,ends_local=$7,all_day=$8,timezone=$9,recurrence=$10,recurrence_until=$11,version=version+1 WHERE group_id=$1 AND id=$2")
   .bind(group).bind(id).bind(e.title.trim()).bind(e.description).bind(e.location).bind(e.starts_local).bind(e.ends_local).bind(e.all_day).bind(e.timezone).bind(e.recurrence).bind(e.recurrence_until).execute(&mut *tx).await?;
        let result = event(&mut tx, group, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn group_event_occurrences(
        &self,
        user: Uuid,
        group: Uuid,
        query: GroupEventRangeQuery,
    ) -> AppResult<GroupEventOccurrencePage> {
        if query.from.year() < 1900
            || query.until.year() > 9998
            || query.until <= query.from
            || query.until - query.from > chrono::Duration::days(42)
        {
            return Err(AppError::Validation(
                "Choose an exclusive date window of 1-42 days between 1900 and 9998.".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        read_access(&mut tx, user, group).await?;
        timezone(&mut tx, &query.timezone).await?;
        let limit = query.limit.unwrap_or(10).clamp(1, 25);
        // Paginate definitions, not occurrence rows. Even a long-running daily series
        // emits only a bounded window, with no discarded instances at a page boundary.
        let mut ids:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM group_events WHERE group_id=$1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR id<$2) AND CASE WHEN all_day THEN starts_local<$4::date::timestamp ELSE starts_local AT TIME ZONE timezone<$4::date::timestamp AT TIME ZONE $5 END AND (recurrence<>'none' OR CASE WHEN all_day THEN ends_local>$3::date::timestamp ELSE ends_local AT TIME ZONE timezone>$3::date::timestamp AT TIME ZONE $5 END) AND (recurrence_until IS NULL OR recurrence_until>=CASE WHEN all_day THEN $3::date ELSE ($3::date::timestamp AT TIME ZONE $5 AT TIME ZONE timezone)::date END-31) ORDER BY id DESC LIMIT $6")
    .bind(group).bind(query.cursor).bind(query.from).bind(query.until).bind(&query.timezone).bind(limit+1).fetch_all(&mut *tx).await?;
        let more = ids.len() as i64 > limit;
        if more {
            ids.pop();
        }
        let next_cursor = if more { ids.last().copied() } else { None };
        let items = sqlx::query_as(include_str!("group_event_occurrences.sql"))
            .bind(user)
            .bind(&ids)
            .bind(&query.timezone)
            .bind(query.from)
            .bind(query.until)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(GroupEventOccurrencePage { items, next_cursor })
    }
    pub async fn delete_group_event(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
        expected: i32,
    ) -> AppResult<GroupEventUndo> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        let a = write_access(&mut tx, user, group).await?;
        a.leader(user)?;
        let old = event(&mut tx, group, id).await?;
        version(old.version, expected)?;
        let result=sqlx::query_as("UPDATE group_events SET deleted_at=clock_timestamp(),undo_until=clock_timestamp()+INTERVAL '60 seconds',deleted_membership_epoch=$3,version=version+1 WHERE group_id=$1 AND id=$2 RETURNING id,title,version,undo_until")
   .bind(group).bind(id).bind(a.membership_epoch).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn restore_group_event(
        &self,
        user: Uuid,
        group: Uuid,
        id: Uuid,
        expected: i32,
    ) -> AppResult<GroupEvent> {
        let mut tx = self.pool.begin().await?;
        lock_users(&mut tx, user, vec![]).await?;
        let a = write_access(&mut tx, user, group).await?;
        a.leader(user)?;
        let restored=sqlx::query("UPDATE group_events SET deleted_at=NULL,undo_until=NULL,deleted_membership_epoch=NULL,version=version+1 WHERE group_id=$1 AND id=$2 AND version=$3 AND undo_until>clock_timestamp() AND deleted_membership_epoch=$4")
    .bind(group).bind(id).bind(expected).bind(a.membership_epoch).execute(&mut *tx).await?;
        if restored.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Undo expired or the series/membership changed. Refresh before continuing.".into(),
            ));
        }
        let result = event(&mut tx, group, id).await?;
        tx.commit().await?;
        Ok(result)
    }
    pub async fn group_event_undos(
        &self,
        user: Uuid,
        group: Uuid,
        page: PageQuery,
    ) -> AppResult<GroupEventUndoPage> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        read_access(&mut tx, user, group).await?.leader(user)?;
        let limit = page.normalized_limit();
        let mut items:Vec<GroupEventUndo>=sqlx::query_as("SELECT id,title,version,undo_until FROM group_events WHERE group_id=$1 AND undo_until>clock_timestamp() AND ($2::uuid IS NULL OR id<$2) ORDER BY id DESC LIMIT $3").bind(group).bind(page.cursor).bind(limit+1).fetch_all(&mut *tx).await?;
        let more = items.len() as i64 > limit;
        if more {
            items.pop();
        }
        let next_cursor = if more {
            items.last().map(|e| e.id)
        } else {
            None
        };
        tx.commit().await?;
        Ok(GroupEventUndoPage { items, next_cursor })
    }
}
