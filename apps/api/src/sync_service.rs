use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    google_auth::{GoogleCredentials, GoogleOAuth},
    models::EventRecurrence,
    store::Store,
    sync_store::ClaimedSyncJob,
};

const GOOGLE_CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

#[derive(Clone)]
pub struct SyncService {
    pub(crate) store: Store,
    google: GoogleOAuth,
    pub(crate) tasks_client: crate::google_tasks_client::GoogleTasksClient,
    http: reqwest::Client,
    google_api_base: String,
    webhook_url: Option<String>,
}

impl SyncService {
    pub fn new(store: Store, google: GoogleOAuth, webhook_url: Option<String>) -> Result<Self> {
        Ok(Self {
            store,
            google,
            tasks_client: crate::google_tasks_client::GoogleTasksClient::new()?,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(45))
                .build()
                .context("failed to build Google Calendar HTTP client")?,
            google_api_base: GOOGLE_CALENDAR_API.to_owned(),
            webhook_url,
        })
    }

    pub fn with_api_base(mut self, base: String) -> Self {
        self.google_api_base = base;
        self
    }

    pub fn with_tasks_api_base(mut self, base: Url) -> Self {
        self.tasks_client = self.tasks_client.with_api_base(base);
        self
    }

    pub async fn enqueue_periodic_work(&self) -> Result<u64> {
        let mut enqueued = self.store.enqueue_periodic_synchronizations().await?;
        if self.webhook_url.is_some() {
            enqueued += self.store.enqueue_expiring_calendar_watches(None).await?;
        }
        Ok(enqueued)
    }

    pub async fn run_once(&self) -> Result<bool> {
        self.store.cleanup_task_delete_undos().await?;
        self.store.cleanup_discord().await?;
        let Some(job) = self.store.claim_sync_job().await? else {
            return Ok(false);
        };
        let mut user_lock = self.store.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("prosepect-sync:{}", job.user_id))
            .execute(&mut *user_lock)
            .await?;
        let status = sqlx::query_scalar::<_, String>("SELECT status FROM sync_jobs WHERE id = $1")
            .bind(job.id)
            .fetch_optional(&self.store.pool)
            .await?;
        if status.as_deref() != Some("running") {
            user_lock.rollback().await?;
            return Ok(true);
        }
        let started = std::time::Instant::now();
        let result = self.execute(&job).await;
        metrics::histogram!(
            "prosepect_synchronization_duration_seconds",
            "kind" => job.kind.clone()
        )
        .record(started.elapsed().as_secs_f64());
        match result {
            Ok(()) => {
                metrics::counter!(
                    "prosepect_sync_jobs_total",
                    "kind" => job.kind.clone(),
                    "status" => "succeeded"
                )
                .increment(1);
                self.store.complete_sync_job(job.id).await?;
                self.record_activity(
                    job.user_id,
                    "synchronization_succeeded",
                    "Calendar synchronization completed",
                )
                .await?;
            }
            Err(error) => {
                metrics::counter!(
                    "prosepect_sync_jobs_total",
                    "kind" => job.kind.clone(),
                    "status" => "failed"
                )
                .increment(1);
                metrics::counter!(
                    "prosepect_sync_job_failures_total",
                    "kind" => job.kind.clone()
                )
                .increment(1);
                self.store
                    .fail_sync_job(job.id, job.attempt_count + 1, &format!("{error:#}"))
                    .await?;
                self.record_activity(
                    job.user_id,
                    "synchronization_failed",
                    if job.attempt_count + 1 < 8 {
                        "Calendar synchronization failed and will be retried"
                    } else {
                        "Calendar synchronization failed after the final attempt"
                    },
                )
                .await?;
            }
        }
        // Drop only queues a rollback. Await release before reporting completion so
        // an immediately following nonblocking user mutation cannot see our old lock.
        user_lock.rollback().await?;
        Ok(true)
    }

    async fn execute(&self, job: &ClaimedSyncJob) -> Result<()> {
        match job.kind.as_str() {
            "calendar_discovery" => self.discover_calendars(job.user_id).await,
            "tasks_sync" => self.sync_google_tasks(job.user_id).await,
            "calendar_sync" => self.sync_calendars(job.user_id, job.calendar_id).await,
            "calendar_watch" => {
                let calendar_id = job
                    .calendar_id
                    .ok_or_else(|| anyhow!("calendar watch job requires a calendar"))?;
                self.watch_calendar(job.user_id, calendar_id).await
            }
            "credential_revoke" => self.revoke(job.user_id).await,
            kind => bail!("unsupported synchronization job kind {kind}"),
        }
    }

    pub(crate) async fn validate_task_delete_undo(
        &self,
        user: Uuid,
        mapping: &serde_json::Value,
    ) -> crate::error::AppResult<()> {
        use crate::error::AppError;
        let unavailable = || {
            AppError::InvalidRequest {
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            message: "Google could not be freshly validated. Nothing was changed; retry deletion or Undo while it remains eligible.".into(),
        }
        };
        let validate = async {
            let calendar = mapping["external_calendar_id"]
                .as_str()
                .ok_or_else(crate::task_delete_store::changed)?;
            let identity = mapping["external_event_id"]
                .as_str()
                .ok_or_else(crate::task_delete_store::changed)?;
            let token = self.access_token(user).await.map_err(|_| unavailable())?;
            let url = self
                .url(&["calendars", calendar, "events", identity])
                .map_err(|_| unavailable())?;
            let mut response = self
                .send(Method::GET, url, &token, Option::<&()>::None, None)
                .await
                .map_err(|_| unavailable())?;
            if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::GONE) {
                return Err(crate::task_delete_store::changed());
            }
            if !response.status().is_success() {
                return Err(unavailable());
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                if body.len() + chunk.len() > 128 * 1024 {
                    return Err(unavailable());
                }
                body.extend_from_slice(&chunk);
            }
            let event: GoogleEvent = serde_json::from_slice(&body).map_err(|_| unavailable())?;
            if event.id.as_deref() != Some(identity)
                || event.status.as_deref() == Some("cancelled")
                || event.etag.as_deref() != mapping["external_etag"].as_str()
            {
                return Err(crate::task_delete_store::changed());
            }
            let normalized = normalize_event(&event).map_err(|_| unavailable())?;
            if Some(event_fingerprint(&normalized).as_str()) != mapping["base_fingerprint"].as_str()
            {
                return Err(crate::task_delete_store::changed());
            }
            Ok(())
        };
        tokio::time::timeout(Duration::from_secs(5), validate)
            .await
            .map_err(|_| unavailable())?
    }

    pub(crate) async fn access_token(&self, user_id: Uuid) -> Result<String> {
        let credentials = self.store.google_credentials(user_id).await?;
        let access = self.google.access_token(&credentials).await?;
        if let Some(encrypted) = access.encrypted_token {
            self.store
                .update_google_access_token(user_id, encrypted, access.expires_at)
                .await?;
        }
        Ok(access.token)
    }

    async fn discover_calendars(&self, user_id: Uuid) -> Result<()> {
        let token = self.access_token(user_id).await?;
        let mut page_token: Option<String> = None;
        loop {
            let mut url = self.url(&["users", "me", "calendarList"])?;
            {
                let mut query = url.query_pairs_mut();
                query.append_pair("maxResults", "250");
                if let Some(page_token) = &page_token {
                    query.append_pair("pageToken", page_token);
                }
            }
            let page: GoogleCalendarList = self
                .send_json(Method::GET, url, &token, Option::<&()>::None, None)
                .await?;
            for calendar in page.items {
                sqlx::query(
                    r#"
                    INSERT INTO calendars (
                        id, user_id, name, color, source, external_id, selected, is_default,
                        provider_primary, access_role
                    ) VALUES ($1, $2, $3, $4, 'google', $5, $6, FALSE, $7, $8)
                    ON CONFLICT (user_id, source, external_id)
                    DO UPDATE SET name = EXCLUDED.name, color = EXCLUDED.color,
                        provider_primary = EXCLUDED.provider_primary,
                        access_role = EXCLUDED.access_role,
                        selected = calendars.selected, updated_at = NOW()
                    "#,
                )
                .bind(Uuid::now_v7())
                .bind(user_id)
                .bind(calendar.summary)
                .bind(
                    calendar
                        .background_color
                        .unwrap_or_else(|| "#4285f4".to_owned()),
                )
                .bind(calendar.id)
                .bind(calendar.selected.unwrap_or(false) || calendar.primary.unwrap_or(false))
                .bind(calendar.primary.unwrap_or(false))
                .bind(calendar.access_role.unwrap_or_else(|| "reader".to_owned()))
                .execute(&self.store.pool)
                .await?;
            }
            page_token = page.next_page_token;
            if page_token.is_none() {
                break;
            }
        }
        self.store
            .migrate_scheduled_tasks_to_preferred_calendar(user_id)
            .await?;
        if self.webhook_url.is_some() {
            self.store
                .enqueue_expiring_calendar_watches(Some(user_id))
                .await?;
        }
        self.record_activity(user_id, "calendar_discovery", "Google calendars refreshed")
            .await
    }

    async fn watch_calendar(&self, user_id: Uuid, calendar_id: Uuid) -> Result<()> {
        let webhook_url = self
            .webhook_url
            .as_deref()
            .ok_or_else(|| anyhow!("Google Calendar webhook URL is not configured"))?;
        let calendar = sqlx::query_as::<_, WatchCalendar>(
            r#"
            SELECT id, external_id
            FROM calendars
            WHERE id = $1 AND user_id = $2 AND source = 'google' AND selected
              AND access_role IN ('writer', 'owner')
            "#,
        )
        .bind(calendar_id)
        .bind(user_id)
        .fetch_optional(&self.store.pool)
        .await?;
        let Some(calendar) = calendar else {
            return Ok(());
        };
        let access_token = self.access_token(user_id).await?;
        let channel_id = Uuid::new_v4();
        let channel_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let request = GoogleWatchRequest {
            id: channel_id.to_string(),
            kind: "web_hook",
            address: webhook_url,
            token: &channel_token,
            params: GoogleWatchParams { ttl: "604800" },
        };
        let url = self.url(&["calendars", &calendar.external_id, "events", "watch"])?;
        let response: GoogleWatchResponse = self
            .send_json(Method::POST, url, &access_token, Some(&request), None)
            .await?;
        let expires_at = response
            .expiration
            .as_deref()
            .and_then(|value| value.parse::<i64>().ok())
            .and_then(|value| Utc.timestamp_millis_opt(value).single())
            .unwrap_or_else(|| Utc::now() + chrono::Duration::days(6));
        let token_hash = Sha256::digest(channel_token.as_bytes()).to_vec();
        let old_channels = match self
            .store
            .replace_google_watch_channel(
                user_id,
                calendar.id,
                channel_id,
                &response.resource_id,
                &token_hash,
                expires_at,
            )
            .await
        {
            Ok(channels) => channels,
            Err(error) => {
                let stop_url = self.url(&["channels", "stop"])?;
                let stop = GoogleStopChannelRequest {
                    id: channel_id.to_string(),
                    resource_id: &response.resource_id,
                };
                if let Err(stop_error) = self
                    .send(Method::POST, stop_url, &access_token, Some(&stop), None)
                    .await
                    .and_then(|response| {
                        response
                            .error_for_status()
                            .map(|_| ())
                            .context("Google Calendar channel cleanup failed")
                    })
                {
                    tracing::warn!(error = ?stop_error, channel_id = %channel_id, "unpersisted Google watch channel could not be stopped");
                }
                return Err(error.into());
            }
        };
        for old in old_channels {
            let stop_url = self.url(&["channels", "stop"])?;
            let stop = GoogleStopChannelRequest {
                id: old.channel_id.to_string(),
                resource_id: &old.resource_id,
            };
            if let Err(error) = self
                .send(Method::POST, stop_url, &access_token, Some(&stop), None)
                .await
                .and_then(|response| {
                    response
                        .error_for_status()
                        .map(|_| ())
                        .context("Google Calendar channel stop failed")
                })
            {
                tracing::warn!(error = ?error, channel_id = %old.channel_id, "old Google watch channel could not be stopped");
            }
        }
        self.store
            .enqueue_sync(
                user_id,
                Some(calendar.id),
                "calendar_sync",
                &format!("calendar-watch-start:{channel_id}"),
            )
            .await?;
        self.record_activity(
            user_id,
            "calendar_watch_enabled",
            "Real-time Google Calendar notifications enabled",
        )
        .await
    }

    async fn sync_calendars(&self, user_id: Uuid, only_calendar: Option<Uuid>) -> Result<()> {
        let calendars = sqlx::query_as::<_, SyncCalendar>(
            r#"
            SELECT id, external_id, sync_token
            FROM calendars
            WHERE user_id = $1 AND source = 'google' AND selected
              AND ($2::UUID IS NULL OR id = $2)
            "#,
        )
        .bind(user_id)
        .bind(only_calendar)
        .fetch_all(&self.store.pool)
        .await?;
        let token = self.access_token(user_id).await?;
        for calendar in calendars {
            let result = self.pull_calendar(user_id, &calendar, &token).await;
            if let Err(error) = result {
                sqlx::query(
                    "UPDATE calendars SET last_sync_error = LEFT($3, 2000) WHERE id = $1 AND user_id = $2",
                )
                .bind(calendar.id)
                .bind(user_id)
                .bind(format!("{error:#}"))
                .execute(&self.store.pool)
                .await?;
                return Err(error);
            }
            self.reconcile_deleted_task_resolution(user_id, &calendar, &token)
                .await?;
            self.push_calendar(user_id, &calendar, &token).await?;
            sqlx::query(
                "UPDATE calendars SET last_synced_at = NOW(), last_sync_error = NULL WHERE id = $1 AND user_id = $2",
            )
            .bind(calendar.id)
            .bind(user_id)
            .execute(&self.store.pool)
            .await?;
        }
        Ok(())
    }

    async fn reconcile_deleted_task_resolution(
        &self,
        user: Uuid,
        calendar: &SyncCalendar,
        token: &str,
    ) -> Result<()> {
        // Explicit Keep Google has disarmed all pushes. A targeted read also settles
        // missing identities when an incremental/full list contains no event.
        let pending: Vec<(Uuid,String)> = sqlx::query_as("SELECT id,external_event_id FROM external_event_mappings WHERE user_id=$1 AND calendar_id=$2 AND delete_resolution_pending AND NOT local_dirty AND NOT local_deleted AND canonical_event_id IS NULL ORDER BY id LIMIT 100")
            .bind(user).bind(calendar.id).fetch_all(&self.store.pool).await?;
        for (mapping, identity) in pending {
            let url = self.url(&["calendars", &calendar.external_id, "events", &identity])?;
            let response = self
                .send(Method::GET, url, token, Option::<&()>::None, None)
                .await?;
            if matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::GONE) {
                sqlx::query(
                    "DELETE FROM external_event_mappings WHERE id=$1 AND delete_resolution_pending",
                )
                .bind(mapping)
                .execute(&self.store.pool)
                .await?;
            } else {
                let event: GoogleEvent = response.error_for_status()?.json().await?;
                if event.id.as_deref() != Some(&identity) {
                    bail!("Google resolution returned a different identity");
                }
                self.apply_remote_event(user, calendar, event).await?;
            }
        }
        Ok(())
    }

    async fn pull_calendar(
        &self,
        user_id: Uuid,
        calendar: &SyncCalendar,
        token: &str,
    ) -> Result<()> {
        let mut page_token: Option<String> = None;
        let mut sync_token = calendar.sync_token.clone();
        let mut retried_full = false;
        loop {
            let mut url = self.url(&["calendars", &calendar.external_id, "events"])?;
            {
                let mut query = url.query_pairs_mut();
                query
                    .append_pair("showDeleted", "true")
                    .append_pair("maxResults", "2500");
                if let Some(value) = &sync_token {
                    query.append_pair("syncToken", value);
                }
                if let Some(value) = &page_token {
                    query.append_pair("pageToken", value);
                }
            }
            let response = self
                .send(Method::GET, url, token, Option::<&()>::None, None)
                .await?;
            if response.status() == StatusCode::GONE && !retried_full {
                sync_token = None;
                page_token = None;
                retried_full = true;
                continue;
            }
            let response = response
                .error_for_status()
                .context("Google event pull failed")?;
            let page: GoogleEventList =
                response.json().await.context("invalid Google event list")?;
            for event in page.items {
                self.apply_remote_event(user_id, calendar, event).await?;
            }
            page_token = page.next_page_token;
            if page_token.is_none() {
                let has_unresolved_conflicts: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS(
                        SELECT 1 FROM external_event_mappings
                        WHERE calendar_id = $1 AND conflict_state = 'unresolved'
                    )
                    "#,
                )
                .bind(calendar.id)
                .fetch_one(&self.store.pool)
                .await?;
                if !has_unresolved_conflicts && let Some(next_sync_token) = page.next_sync_token {
                    sqlx::query("UPDATE calendars SET sync_token = $2 WHERE id = $1")
                        .bind(calendar.id)
                        .bind(next_sync_token)
                        .execute(&self.store.pool)
                        .await?;
                }
                break;
            }
        }
        Ok(())
    }

    async fn apply_remote_event(
        &self,
        user_id: Uuid,
        calendar: &SyncCalendar,
        event: GoogleEvent,
    ) -> Result<()> {
        let Some(external_id) = event.id.clone() else {
            return Ok(());
        };
        let mapping = sqlx::query_as::<_, MappingState>(
            r#"
            SELECT m.id, m.canonical_event_id, m.external_etag, m.local_dirty, m.local_deleted,
                   m.pending_resolution, m.reversible_tombstone, e.updated_at AS local_updated_at
            FROM external_event_mappings m
            LEFT JOIN calendar_events e ON e.id = m.canonical_event_id
            WHERE m.user_id = $1 AND m.external_calendar_id = $2 AND m.external_event_id = $3
            "#,
        )
        .bind(user_id)
        .bind(&calendar.external_id)
        .bind(&external_id)
        .fetch_optional(&self.store.pool)
        .await?;
        // Reversible tombstones keep their own provenance after receipt cleanup.
        // This precedes every conflict policy, full pull, and 410 recovery path.
        if let Some(mapping) = &mapping
            && mapping.reversible_tombstone
        {
            if event.status.as_deref() == Some("cancelled") {
                let mut tx = self.store.pool.begin().await?;
                sqlx::query("DELETE FROM task_delete_undos WHERE user_id=$1 AND snapshot->'mapping'->>'id'=$2")
                    .bind(user_id).bind(mapping.id.to_string()).execute(&mut *tx).await?;
                sqlx::query("DELETE FROM external_event_mappings WHERE id=$1")
                    .bind(mapping.id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            } else if event.etag.is_none() || mapping.external_etag != event.etag {
                // Never replace the deleted canonical event or advance this baseline.
                self.create_conflict(
                    user_id,
                    mapping,
                    event.summary.as_deref().unwrap_or("Deleted task event"),
                )
                .await?;
                sqlx::query("DELETE FROM task_delete_undos WHERE user_id=$1 AND snapshot->'mapping'->>'id'=$2")
                    .bind(user_id).bind(mapping.id.to_string()).execute(&self.store.pool).await?;
            }
            return Ok(());
        }
        if event.status.as_deref() == Some("cancelled") {
            if let Some(mapping) = mapping {
                if mapping.local_dirty {
                    let policy = if let Some(resolution) = &mapping.pending_resolution {
                        resolution.clone()
                    } else {
                        sqlx::query_scalar(
                            "SELECT sync_conflict_policy FROM user_settings WHERE user_id = $1",
                        )
                        .bind(user_id)
                        .fetch_optional(&self.store.pool)
                        .await?
                        .unwrap_or_else(|| "ask".to_owned())
                    };
                    let keep_local = policy == "prosepect"
                        || (policy == "latest"
                            && event.updated.zip(mapping.local_updated_at).is_some_and(
                                |(remote_updated, local_updated)| local_updated >= remote_updated,
                            ));
                    if policy == "ask" {
                        self.create_conflict(
                            user_id,
                            &mapping,
                            event.summary.as_deref().unwrap_or("Deleted event"),
                        )
                        .await?;
                        return Ok(());
                    }
                    if keep_local {
                        sqlx::query("DELETE FROM external_event_mappings WHERE id = $1")
                            .bind(mapping.id)
                            .execute(&self.store.pool)
                            .await?;
                        return Ok(());
                    }
                }
                if let Some(event_id) = mapping.canonical_event_id {
                    delete_canonical_event(&self.store, user_id, event_id).await?;
                }
                sqlx::query("DELETE FROM external_event_mappings WHERE id = $1")
                    .bind(mapping.id)
                    .execute(&self.store.pool)
                    .await?;
                self.record_activity(
                    user_id,
                    "remote_event_deleted",
                    "Applied a Google event deletion",
                )
                .await?;
            }
            return Ok(());
        }
        let normalized = normalize_event(&event)?;
        if let Some(mapping) = mapping {
            if mapping.external_etag == event.etag {
                return Ok(());
            }
            if mapping.local_dirty || mapping.local_deleted {
                let policy = if let Some(resolution) = &mapping.pending_resolution {
                    resolution.clone()
                } else {
                    sqlx::query_scalar(
                        "SELECT sync_conflict_policy FROM user_settings WHERE user_id = $1",
                    )
                    .bind(user_id)
                    .fetch_optional(&self.store.pool)
                    .await?
                    .unwrap_or_else(|| "ask".to_owned())
                };
                let keep_local = policy == "prosepect"
                    || (policy == "latest"
                        && event.updated.zip(mapping.local_updated_at).is_some_and(
                            |(remote_updated, local_updated)| local_updated >= remote_updated,
                        ));
                if policy == "ask" || keep_local {
                    if policy == "ask" {
                        self.create_conflict(user_id, &mapping, &normalized.title)
                            .await?;
                    } else {
                        sqlx::query(
                            "UPDATE external_event_mappings SET pending_resolution = NULL WHERE id = $1",
                        )
                        .bind(mapping.id)
                        .execute(&self.store.pool)
                        .await?;
                    }
                    return Ok(());
                }
            }
            if let Some(event_id) = mapping.canonical_event_id {
                update_canonical_event(&self.store, user_id, event_id, calendar.id, &normalized)
                    .await?;
            } else {
                let event_id = Uuid::now_v7();
                insert_canonical_event(&self.store, user_id, event_id, calendar.id, &normalized)
                    .await?;
                sqlx::query(
                    "UPDATE external_event_mappings SET canonical_event_id = $2 WHERE id = $1",
                )
                .bind(mapping.id)
                .bind(event_id)
                .execute(&self.store.pool)
                .await?;
            }
            update_mapping_baseline(&self.store, mapping.id, &event, &normalized).await?;
            self.record_activity(
                user_id,
                "remote_event_applied",
                "Applied a Google event change",
            )
            .await?;
        } else {
            let event_id = Uuid::now_v7();
            insert_canonical_event(&self.store, user_id, event_id, calendar.id, &normalized)
                .await?;
            sqlx::query(
                r#"
                INSERT INTO external_event_mappings (
                    id, user_id, calendar_id, canonical_event_id, external_calendar_id,
                    external_event_id, external_etag, base_fingerprint, last_synced_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(user_id)
            .bind(calendar.id)
            .bind(event_id)
            .bind(&calendar.external_id)
            .bind(external_id)
            .bind(&event.etag)
            .bind(event_fingerprint(&normalized))
            .execute(&self.store.pool)
            .await?;
            self.record_activity(
                user_id,
                "remote_event_imported",
                "Imported an event from Google",
            )
            .await?;
        }
        Ok(())
    }

    async fn push_calendar(
        &self,
        user_id: Uuid,
        calendar: &SyncCalendar,
        token: &str,
    ) -> Result<()> {
        let unmapped = sqlx::query_as::<_, LocalEvent>(
            r#"
            SELECT e.id, e.title, e.description, e.starts_at, e.ends_at, e.all_day,
                   e.timezone, e.location, e.attendees, e.recurrence, e.recurrence_until
            FROM calendar_events e
            LEFT JOIN external_event_mappings m ON m.canonical_event_id = e.id
            WHERE e.user_id = $1 AND e.calendar_id = $2 AND m.id IS NULL
            "#,
        )
        .bind(user_id)
        .bind(calendar.id)
        .fetch_all(&self.store.pool)
        .await?;
        for event in unmapped {
            let external_id = google_event_id(event.id);
            let mut body = GoogleEventWrite::from_local(&event);
            body.id = Some(external_id.clone());
            let url = self.url(&["calendars", &calendar.external_id, "events"])?;
            let response = self
                .send(Method::POST, url, token, Some(&body), None)
                .await?;
            let remote: GoogleEvent = if response.status() == StatusCode::CONFLICT {
                let url =
                    self.url(&["calendars", &calendar.external_id, "events", &external_id])?;
                self.send_json(Method::GET, url, token, Option::<&()>::None, None)
                    .await?
            } else {
                response
                    .error_for_status()
                    .context("Google event creation failed")?
                    .json()
                    .await
                    .context("Google event creation returned invalid JSON")?
            };
            let external_id = remote.id.clone().unwrap_or(external_id);
            sqlx::query(
                r#"
                INSERT INTO external_event_mappings (
                    id, user_id, calendar_id, canonical_event_id, external_calendar_id,
                    external_event_id, external_etag, base_fingerprint, last_synced_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(user_id)
            .bind(calendar.id)
            .bind(event.id)
            .bind(&calendar.external_id)
            .bind(external_id)
            .bind(remote.etag)
            .bind(local_fingerprint(&event))
            .execute(&self.store.pool)
            .await?;
            self.record_activity(
                user_id,
                "local_event_created",
                "Created a Google event from Prosepect",
            )
            .await?;
        }
        let dirty = sqlx::query_as::<_, DirtyMapping>(
            r#"
            SELECT m.id AS mapping_id, m.external_event_id, m.external_etag,
                   m.local_deleted, e.id, e.title, e.description, e.starts_at, e.ends_at,
                   e.all_day, e.timezone, e.location, e.attendees, e.recurrence,
                   e.recurrence_until
            FROM external_event_mappings m
            LEFT JOIN calendar_events e ON e.id = m.canonical_event_id
            WHERE m.user_id = $1 AND m.calendar_id = $2
              AND (m.local_dirty OR m.local_deleted) AND m.conflict_state = 'none'
              AND NOT m.delete_resolution_pending
              AND (m.deletion_hold_until IS NULL OR m.deletion_hold_until <= clock_timestamp())
            "#,
        )
        .bind(user_id)
        .bind(calendar.id)
        .fetch_all(&self.store.pool)
        .await?;
        for mapping in dirty {
            let url = self.url(&[
                "calendars",
                &calendar.external_id,
                "events",
                &mapping.external_event_id,
            ])?;
            if mapping.local_deleted || mapping.id.is_none() {
                let response = self
                    .send(
                        Method::DELETE,
                        url,
                        token,
                        Option::<&()>::None,
                        mapping.external_etag.as_deref(),
                    )
                    .await?;
                if response.status() != StatusCode::NOT_FOUND {
                    response
                        .error_for_status()
                        .context("Google event deletion failed")?;
                }
                sqlx::query("DELETE FROM external_event_mappings WHERE id = $1")
                    .bind(mapping.mapping_id)
                    .execute(&self.store.pool)
                    .await?;
                self.record_activity(
                    user_id,
                    "local_event_deleted",
                    "Deleted a Google event from Prosepect",
                )
                .await?;
            } else {
                let local = mapping.local_event()?;
                let body = GoogleEventWrite::from_local(&local);
                let remote: GoogleEvent = self
                    .send_json(
                        Method::PUT,
                        url,
                        token,
                        Some(&body),
                        mapping.external_etag.as_deref(),
                    )
                    .await?;
                sqlx::query(
                    "UPDATE external_event_mappings SET external_etag = $2, base_fingerprint = $3, local_dirty = FALSE, last_synced_at = NOW() WHERE id = $1",
                ).bind(mapping.mapping_id).bind(remote.etag).bind(local_fingerprint(&local))
                  .execute(&self.store.pool).await?;
                self.record_activity(
                    user_id,
                    "local_event_updated",
                    "Updated a Google event from Prosepect",
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn revoke(&self, user_id: Uuid) -> Result<()> {
        let credentials: GoogleCredentials = self.store.google_credentials(user_id).await?;
        let access_token = self.access_token(user_id).await.ok();
        if let Some(access_token) = access_token {
            for channel in self.store.google_watch_channels_for_user(user_id).await? {
                let stop_url = self.url(&["channels", "stop"])?;
                let request = GoogleStopChannelRequest {
                    id: channel.channel_id.to_string(),
                    resource_id: &channel.resource_id,
                };
                if let Err(error) = self
                    .send(Method::POST, stop_url, &access_token, Some(&request), None)
                    .await
                    .and_then(|response| {
                        response
                            .error_for_status()
                            .map(|_| ())
                            .context("Google Calendar channel stop failed")
                    })
                {
                    tracing::warn!(error = ?error, channel_id = %channel.channel_id, "Google watch channel cleanup failed");
                }
            }
        }
        self.google.revoke(&credentials).await?;
        self.store.revoke_google_integration(user_id).await?;
        self.record_activity(
            user_id,
            "integration_revoked",
            "Google Calendar disconnected",
        )
        .await
    }

    async fn create_conflict(
        &self,
        user_id: Uuid,
        mapping: &MappingState,
        title: &str,
    ) -> Result<()> {
        let conflict_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO sync_conflicts (id, user_id, mapping_id, canonical_event_id, title)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (mapping_id) WHERE status = 'unresolved' DO NOTHING
            "#,
        )
        .bind(conflict_id)
        .bind(user_id)
        .bind(mapping.id)
        .bind(mapping.canonical_event_id)
        .bind(title)
        .execute(&self.store.pool)
        .await?;
        sqlx::query(
            "UPDATE external_event_mappings SET conflict_state = 'unresolved' WHERE id = $1",
        )
        .bind(mapping.id)
        .execute(&self.store.pool)
        .await?;
        self.record_activity(
            user_id,
            "synchronization_conflict",
            "A calendar event needs a conflict decision",
        )
        .await?;
        Ok(())
    }

    async fn send_json<T: for<'de> Deserialize<'de>, B: Serialize + ?Sized>(
        &self,
        method: Method,
        url: Url,
        token: &str,
        body: Option<&B>,
        etag: Option<&str>,
    ) -> Result<T> {
        self.send(method, url, token, body, etag)
            .await?
            .error_for_status()
            .context("Google Calendar request failed")?
            .json()
            .await
            .context("Google Calendar returned invalid JSON")
    }

    async fn send<B: Serialize + ?Sized>(
        &self,
        method: Method,
        url: Url,
        token: &str,
        body: Option<&B>,
        etag: Option<&str>,
    ) -> Result<reqwest::Response> {
        for attempt in 0..4 {
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .bearer_auth(token);
            if let Some(body) = body {
                request = request.json(body);
            }
            if let Some(etag) = etag {
                request = request.header(reqwest::header::IF_MATCH, etag);
            }
            let response = request
                .send()
                .await
                .context("Google Calendar request failed")?;
            if response.status() != StatusCode::TOO_MANY_REQUESTS
                && !response.status().is_server_error()
            {
                return Ok(response);
            }
            if attempt == 3 {
                return Ok(response);
            }
            let delay = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
                .unwrap_or(2_u64.pow(attempt));
            tokio::time::sleep(Duration::from_secs(delay.min(30))).await;
        }
        unreachable!()
    }

    fn url(&self, segments: &[&str]) -> Result<Url> {
        let mut url = Url::parse(&self.google_api_base)?;
        url.path_segments_mut()
            .map_err(|_| anyhow!("invalid Google API base URL"))?
            .extend(segments.iter().copied());
        Ok(url)
    }

    async fn record_activity(&self, user_id: Uuid, kind: &str, message: &str) -> Result<()> {
        self.store.record_activity(user_id, kind, message).await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct WatchCalendar {
    id: Uuid,
    external_id: String,
}

#[derive(Serialize)]
struct GoogleWatchRequest<'a> {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
    address: &'a str,
    token: &'a str,
    params: GoogleWatchParams,
}

#[derive(Serialize)]
struct GoogleWatchParams {
    ttl: &'static str,
}

#[derive(Deserialize)]
struct GoogleWatchResponse {
    #[serde(rename = "resourceId")]
    resource_id: String,
    expiration: Option<String>,
}

#[derive(Serialize)]
struct GoogleStopChannelRequest<'a> {
    id: String,
    #[serde(rename = "resourceId")]
    resource_id: &'a str,
}

#[derive(Debug, Deserialize, Serialize)]
struct GoogleCalendarList {
    #[serde(default)]
    items: Vec<GoogleCalendar>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct GoogleCalendar {
    id: String,
    summary: String,
    #[serde(rename = "backgroundColor")]
    background_color: Option<String>,
    selected: Option<bool>,
    primary: Option<bool>,
    #[serde(rename = "accessRole")]
    access_role: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct GoogleEventList {
    #[serde(default)]
    items: Vec<GoogleEvent>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(rename = "nextSyncToken")]
    next_sync_token: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct GoogleEvent {
    id: Option<String>,
    etag: Option<String>,
    status: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    description: String,
    start: Option<GoogleEventTime>,
    end: Option<GoogleEventTime>,
    #[serde(default)]
    location: String,
    #[serde(default)]
    attendees: Vec<GoogleAttendee>,
    #[serde(default)]
    recurrence: Vec<String>,
    updated: Option<DateTime<Utc>>,
}
#[derive(Debug, Deserialize, Serialize)]
struct GoogleEventTime {
    #[serde(rename = "dateTime")]
    date_time: Option<DateTime<Utc>>,
    date: Option<NaiveDate>,
    #[serde(rename = "timeZone")]
    time_zone: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
struct GoogleAttendee {
    email: String,
}
#[derive(Debug, Serialize)]
struct GoogleEventWrite {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    summary: String,
    description: String,
    start: GoogleEventTimeWrite,
    end: GoogleEventTimeWrite,
    location: String,
    attendees: Vec<GoogleAttendee>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    recurrence: Vec<String>,
}
#[derive(Debug, Serialize)]
struct GoogleEventTimeWrite {
    #[serde(rename = "dateTime", skip_serializing_if = "Option::is_none")]
    date_time: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<NaiveDate>,
    #[serde(rename = "timeZone", skip_serializing_if = "Option::is_none")]
    time_zone: Option<String>,
}

impl GoogleEventWrite {
    fn from_local(event: &LocalEvent) -> Self {
        Self {
            id: None,
            summary: event.title.clone(),
            description: event.description.clone(),
            start: GoogleEventTimeWrite {
                date_time: (!event.all_day).then_some(event.starts_at),
                date: event.all_day.then_some(event.starts_at.date_naive()),
                time_zone: (!event.all_day).then(|| event.timezone.clone()),
            },
            end: GoogleEventTimeWrite {
                date_time: (!event.all_day).then_some(event.ends_at),
                date: event.all_day.then_some(event.ends_at.date_naive()),
                time_zone: (!event.all_day).then(|| event.timezone.clone()),
            },
            location: event.location.clone(),
            attendees: event
                .attendees
                .iter()
                .map(|email| GoogleAttendee {
                    email: email.clone(),
                })
                .collect(),
            recurrence: recurrence_rule(event.recurrence, event.recurrence_until),
        }
    }
}

#[derive(sqlx::FromRow)]
struct SyncCalendar {
    id: Uuid,
    external_id: String,
    sync_token: Option<String>,
}
#[derive(sqlx::FromRow)]
struct MappingState {
    id: Uuid,
    canonical_event_id: Option<Uuid>,
    external_etag: Option<String>,
    local_dirty: bool,
    local_deleted: bool,
    pending_resolution: Option<String>,
    reversible_tombstone: bool,
    local_updated_at: Option<DateTime<Utc>>,
}
#[derive(Clone, Deserialize, sqlx::FromRow)]
struct LocalEvent {
    id: Uuid,
    title: String,
    description: String,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    all_day: bool,
    timezone: String,
    location: String,
    attendees: Vec<String>,
    recurrence: EventRecurrence,
    recurrence_until: Option<DateTime<Utc>>,
}
#[derive(sqlx::FromRow)]
struct DirtyMapping {
    mapping_id: Uuid,
    external_event_id: String,
    external_etag: Option<String>,
    local_deleted: bool,
    id: Option<Uuid>,
    title: Option<String>,
    description: Option<String>,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    all_day: Option<bool>,
    timezone: Option<String>,
    location: Option<String>,
    attendees: Option<Vec<String>>,
    recurrence: Option<EventRecurrence>,
    recurrence_until: Option<DateTime<Utc>>,
}
impl DirtyMapping {
    fn local_event(&self) -> Result<LocalEvent> {
        Ok(LocalEvent {
            id: self
                .id
                .ok_or_else(|| anyhow!("dirty mapping has no event"))?,
            title: self.title.clone().unwrap_or_default(),
            description: self.description.clone().unwrap_or_default(),
            starts_at: self
                .starts_at
                .ok_or_else(|| anyhow!("event start missing"))?,
            ends_at: self.ends_at.ok_or_else(|| anyhow!("event end missing"))?,
            all_day: self.all_day.unwrap_or(false),
            timezone: self.timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
            location: self.location.clone().unwrap_or_default(),
            attendees: self.attendees.clone().unwrap_or_default(),
            recurrence: self.recurrence.unwrap_or_default(),
            recurrence_until: self.recurrence_until,
        })
    }
}
struct NormalizedEvent {
    title: String,
    description: String,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    all_day: bool,
    timezone: String,
    location: String,
    attendees: Vec<String>,
    recurrence: EventRecurrence,
    recurrence_until: Option<DateTime<Utc>>,
}

fn normalize_event(event: &GoogleEvent) -> Result<NormalizedEvent> {
    let start = event
        .start
        .as_ref()
        .ok_or_else(|| anyhow!("Google event has no start"))?;
    let end = event
        .end
        .as_ref()
        .ok_or_else(|| anyhow!("Google event has no end"))?;
    let all_day = start.date_time.is_none();
    let starts_at = event_time(start)?;
    let ends_at = event_time(end)?;
    let (recurrence, until) = parse_recurrence(&event.recurrence);
    Ok(NormalizedEvent {
        title: event
            .summary
            .clone()
            .unwrap_or_else(|| "Untitled event".to_owned()),
        description: event.description.clone(),
        starts_at,
        ends_at,
        all_day,
        timezone: start.time_zone.clone().unwrap_or_else(|| "UTC".to_owned()),
        location: event.location.clone(),
        attendees: event.attendees.iter().map(|a| a.email.clone()).collect(),
        recurrence,
        recurrence_until: until,
    })
}
fn event_time(value: &GoogleEventTime) -> Result<DateTime<Utc>> {
    if let Some(value) = value.date_time {
        Ok(value)
    } else if let Some(date) = value.date {
        Ok(Utc.from_utc_datetime(
            &date
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| anyhow!("invalid all-day event"))?,
        ))
    } else {
        bail!("Google event time is empty")
    }
}
fn parse_recurrence(values: &[String]) -> (EventRecurrence, Option<DateTime<Utc>>) {
    let Some(rule) = values.iter().find(|v| v.starts_with("RRULE:")) else {
        return (EventRecurrence::None, None);
    };
    let recurrence = if rule.contains("FREQ=DAILY") {
        EventRecurrence::Daily
    } else if rule.contains("FREQ=WEEKLY") {
        EventRecurrence::Weekly
    } else if rule.contains("FREQ=MONTHLY") {
        EventRecurrence::Monthly
    } else if rule.contains("FREQ=YEARLY") {
        EventRecurrence::Yearly
    } else {
        EventRecurrence::None
    };
    let until = rule
        .split(';')
        .find_map(|part| part.strip_prefix("UNTIL="))
        .and_then(|value| chrono::NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ").ok())
        .map(|value| Utc.from_utc_datetime(&value));
    (recurrence, until)
}
fn recurrence_rule(value: EventRecurrence, until: Option<DateTime<Utc>>) -> Vec<String> {
    if value == EventRecurrence::None {
        return Vec::new();
    }
    let mut rule = format!("RRULE:FREQ={}", format!("{value:?}").to_uppercase());
    if let Some(until) = until {
        rule.push_str(&format!(";UNTIL={}", until.format("%Y%m%dT%H%M%SZ")));
    }
    vec![rule]
}
fn event_fingerprint(event: &NormalizedEvent) -> String {
    local_fingerprint(&LocalEvent {
        id: Uuid::nil(),
        title: event.title.clone(),
        description: event.description.clone(),
        starts_at: event.starts_at,
        ends_at: event.ends_at,
        all_day: event.all_day,
        timezone: event.timezone.clone(),
        location: event.location.clone(),
        attendees: event.attendees.clone(),
        recurrence: event.recurrence,
        recurrence_until: event.recurrence_until,
    })
}
fn local_fingerprint(event: &LocalEvent) -> String {
    // Hash exactly the provider-visible write shape, including all-day/timezone,
    // attendees and recurrence end. Legacy baselines require read-only revalidation.
    fingerprint(
        &serde_json::to_string(&GoogleEventWrite::from_local(event))
            .expect("Google event writes contain only JSON-serializable fields"),
    )
}
pub(crate) fn snapshot_event_fingerprint(
    value: &serde_json::Value,
) -> crate::error::AppResult<String> {
    let event: LocalEvent =
        serde_json::from_value(value.clone()).map_err(|_| crate::task_delete_store::changed())?;
    Ok(local_fingerprint(&event))
}
pub(crate) fn legacy_snapshot_event_fingerprint(
    value: &serde_json::Value,
) -> crate::error::AppResult<String> {
    let event: LocalEvent =
        serde_json::from_value(value.clone()).map_err(|_| crate::task_delete_store::changed())?;
    Ok(fingerprint(&format!(
        "{}|{}|{}|{}|{}|{:?}",
        event.title,
        event.description,
        event.starts_at,
        event.ends_at,
        event.location,
        event.recurrence
    )))
}
fn fingerprint(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn google_event_id(event_id: Uuid) -> String {
    event_id.simple().to_string()
}

async fn insert_canonical_event(
    store: &Store,
    user_id: Uuid,
    id: Uuid,
    calendar_id: Uuid,
    event: &NormalizedEvent,
) -> Result<()> {
    sqlx::query(r#"INSERT INTO calendar_events (id,user_id,calendar_id,title,description,starts_at,ends_at,all_day,timezone,location,attendees,recurrence,recurrence_until) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)"#).bind(id).bind(user_id).bind(calendar_id).bind(&event.title).bind(&event.description).bind(event.starts_at).bind(event.ends_at).bind(event.all_day).bind(&event.timezone).bind(&event.location).bind(&event.attendees).bind(event.recurrence).bind(event.recurrence_until).execute(&store.pool).await?;
    Ok(())
}
async fn update_canonical_event(
    store: &Store,
    user_id: Uuid,
    id: Uuid,
    calendar_id: Uuid,
    event: &NormalizedEvent,
) -> Result<()> {
    let mut transaction = store.pool.begin().await?;
    let linked_task_id = sqlx::query_scalar::<_, Option<Uuid>>(r#"UPDATE calendar_events SET calendar_id=$3,title=$4,description=$5,starts_at=$6,ends_at=$7,all_day=$8,timezone=$9,location=$10,attendees=$11,recurrence=$12,recurrence_until=$13,updated_at=NOW(),version=version+1 WHERE id=$1 AND user_id=$2 RETURNING linked_task_id"#).bind(id).bind(user_id).bind(calendar_id).bind(&event.title).bind(&event.description).bind(event.starts_at).bind(event.ends_at).bind(event.all_day).bind(&event.timezone).bind(&event.location).bind(&event.attendees).bind(event.recurrence).bind(event.recurrence_until).fetch_optional(&mut *transaction).await?.flatten();
    if let Some(task_id) = linked_task_id {
        sqlx::query(
            r#"
            UPDATE tasks SET title = $3, description = $4, scheduled_start = $5,
                scheduled_end = $6, updated_at = NOW(), version = version + 1
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(task_id)
        .bind(user_id)
        .bind(&event.title)
        .bind(&event.description)
        .bind(event.starts_at)
        .bind(event.ends_at)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn delete_canonical_event(store: &Store, user_id: Uuid, event_id: Uuid) -> Result<()> {
    let mut transaction = store.pool.begin().await?;
    let linked_task_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "DELETE FROM calendar_events WHERE id = $1 AND user_id = $2 RETURNING linked_task_id",
    )
    .bind(event_id)
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?
    .flatten();
    if let Some(task_id) = linked_task_id {
        sqlx::query(
            r#"
            UPDATE tasks SET scheduled_start = NULL, scheduled_end = NULL,
                updated_at = NOW(), version = version + 1
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(task_id)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}
async fn update_mapping_baseline(
    store: &Store,
    mapping_id: Uuid,
    remote: &GoogleEvent,
    event: &NormalizedEvent,
) -> Result<()> {
    sqlx::query("UPDATE external_event_mappings SET external_etag=$2,base_fingerprint=$3,local_dirty=FALSE,local_deleted=FALSE,pending_resolution=NULL,delete_resolution_pending=FALSE,last_synced_at=NOW() WHERE id=$1").bind(mapping_id).bind(&remote.etag).bind(event_fingerprint(event)).execute(&store.pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
    use base64::Engine;
    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::config::GoogleOAuthConfig;

    #[test]
    fn normalizes_all_day_google_events_and_recurrence_until() {
        let event = GoogleEvent {
            id: Some("event-1".to_owned()),
            etag: Some("etag-1".to_owned()),
            status: Some("confirmed".to_owned()),
            summary: Some("Retrospective".to_owned()),
            description: String::new(),
            start: Some(GoogleEventTime {
                date_time: None,
                date: NaiveDate::from_ymd_opt(2026, 9, 1),
                time_zone: None,
            }),
            end: Some(GoogleEventTime {
                date_time: None,
                date: NaiveDate::from_ymd_opt(2026, 9, 2),
                time_zone: None,
            }),
            location: String::new(),
            attendees: Vec::new(),
            recurrence: vec!["RRULE:FREQ=WEEKLY;UNTIL=20261001T090000Z".to_owned()],
            updated: None,
        };

        let normalized = normalize_event(&event).expect("valid event");
        assert!(normalized.all_day);
        assert_eq!(normalized.recurrence, EventRecurrence::Weekly);
        assert_eq!(
            normalized.recurrence_until,
            Some(Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap())
        );
    }

    #[test]
    fn emits_google_recurrence_rules() {
        assert_eq!(
            recurrence_rule(
                EventRecurrence::Monthly,
                Some(Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap())
            ),
            vec!["RRULE:FREQ=MONTHLY;UNTIL=20261201T000000Z"]
        );
    }

    #[test]
    fn derives_a_retry_safe_google_event_id_from_the_canonical_id() {
        let id = Uuid::parse_str("019cf000-0000-7000-8000-000000000001").unwrap();
        assert_eq!(google_event_id(id), "019cf000000070008000000000000001");
    }

    #[tokio::test]
    async fn retries_rate_limited_google_requests() {
        async fn response(
            State(attempts): State<Arc<AtomicUsize>>,
        ) -> (
            StatusCode,
            [(axum::http::HeaderName, &'static str); 1],
            Json<serde_json::Value>,
        ) {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(axum::http::header::RETRY_AFTER, "0")],
                    Json(serde_json::json!({ "error": "slow down" })),
                );
            }
            (
                StatusCode::OK,
                [(axum::http::header::RETRY_AFTER, "0")],
                Json(serde_json::json!({ "items": [] })),
            )
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route("/calendar-list", get(response))
                    .with_state(attempts.clone()),
            )
            .into_future(),
        );
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres@localhost/prosepect")
            .unwrap();
        let key = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
        let google = GoogleOAuth::new(GoogleOAuthConfig {
            client_id: "client".to_owned(),
            client_secret: "secret".to_owned(),
            redirect_uri: "http://localhost/callback".to_owned(),
            token_encryption_key: key,
        })
        .unwrap();
        let service = SyncService::new(Store::from_pool(pool), google, None)
            .unwrap()
            .with_api_base(format!("http://{address}"));
        let url = service.url(&["calendar-list"]).unwrap();
        let page: GoogleCalendarList = service
            .send_json(Method::GET, url, "token", Option::<&()>::None, None)
            .await
            .unwrap();

        assert!(page.items.is_empty());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
