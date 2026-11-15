use std::time::Duration;

use tokio::{sync::mpsc, time::Instant};

use crate::{file_storage::FileStorage, store::Store, sync_service::SyncService};

const ERROR_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_ERROR_RETRY_DELAY: Duration = Duration::from_secs(300);
const MAX_JOBS_PER_DRAIN: usize = 32;

#[derive(Clone, Default)]
pub struct SyncDispatcher {
    sender: Option<mpsc::Sender<()>>,
}

impl SyncDispatcher {
    pub fn start(service: SyncService, storage: FileStorage) -> Self {
        Self::spawn(service.store.clone(), Some(service), storage)
    }

    pub fn start_maintenance(store: Store, storage: FileStorage) -> Self {
        Self::spawn(store, None, storage)
    }

    fn spawn(store: Store, service: Option<SyncService>, storage: FileStorage) -> Self {
        let (sender, mut receiver) = mpsc::channel(1);
        tokio::spawn(async move {
            let mut wake_at: Option<Instant> = None;
            let mut startup_pending = service.is_some();
            let mut error_delay = ERROR_RETRY_DELAY;
            loop {
                // No queue/deadline means no database traffic. A real mutation,
                // provider notification, startup or external worker can wake us.
                tokio::select! {
                    message = receiver.recv() => if message.is_none() { return; },
                    _ = async {
                        match wake_at {
                            Some(at) => tokio::time::sleep_until(at).await,
                            None => std::future::pending::<()>().await,
                        }
                    } => {}
                }
                if receiver.is_closed() {
                    return;
                }
                let pass: anyhow::Result<Option<Duration>> = async {
                    if startup_pending {
                        if let Some(service) = &service {
                            service.enqueue_periodic_work().await?;
                        }
                        startup_pending = false;
                    }
                    store.cleanup_task_delete_undos().await?;
                    store.cleanup_groups().await?;
                    store.cleanup_discord().await?;
                    crate::file_lifecycle::cleanup_pending_uploads(&store, &storage, None, 1)
                        .await?;
                    if let Some(service) = &service {
                        for _ in 0..MAX_JOBS_PER_DRAIN {
                            // Finish current work on shutdown, never claim another job.
                            if receiver.is_closed() {
                                return Ok(None);
                            }
                            if !service.run_once().await? {
                                break;
                            }
                        }
                    }
                    next_work_delay(&store, service.is_some()).await
                }
                .await;
                if receiver.is_closed() {
                    return;
                }
                match pass {
                    Ok(delay) => {
                        error_delay = ERROR_RETRY_DELAY;
                        // A due row may be locked by another request/worker. Retry
                        // known work without spinning; future deadlines stay exact.
                        wake_at = delay.map(|delay| {
                            Instant::now()
                                + if delay.is_zero() {
                                    ERROR_RETRY_DELAY
                                } else {
                                    delay
                                }
                        });
                    }
                    Err(error) => {
                        tracing::error!(error = ?error, "background work deferred after failure");
                        wake_at = Some(Instant::now() + error_delay);
                        error_delay = (error_delay * 2).min(MAX_ERROR_RETRY_DELAY);
                    }
                }
            }
        });
        let dispatcher = Self {
            sender: Some(sender),
        };
        dispatcher.wake();
        dispatcher
    }

    pub fn wake(&self) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(());
        }
    }
}

// One bounded scheduling decision after each drain, not an idle poll. Keep this
// aligned with the cleanup methods above when adding maintenance expiry tables.
// PostgreSQL supplies the relative delay so host clock skew cannot advance expiry.
async fn next_work_delay(store: &Store, sync_enabled: bool) -> anyhow::Result<Option<Duration>> {
    let seconds: Option<f64> = sqlx::query_scalar(
        r#"
        /* rakkio: background deadline */
        SELECT EXTRACT(EPOCH FROM (MIN(due_at) - clock_timestamp()))::DOUBLE PRECISION
        FROM (
            SELECT MIN(GREATEST(available_at, leased_until)) AS due_at
            FROM sync_jobs
            WHERE $1 AND status IN ('pending', 'failed') AND attempt_count < 8
            UNION ALL
            SELECT MIN(expires_at) FROM task_delete_undos
            UNION ALL
            SELECT MIN(undo_until) FROM groups
            UNION ALL
            SELECT MIN(undo_until) FROM group_assignments
            UNION ALL
            SELECT MIN(LEAST(expires_at, revoked_at + INTERVAL '15 minutes'))
            FROM group_invitations
            UNION ALL
            SELECT MIN(expires_at) FROM group_ownership_offers
            UNION ALL
            SELECT MIN(created_at + INTERVAL '15 minutes') FROM discord_interactions
            UNION ALL
            SELECT MIN(expires_at) FROM discord_oauth_attempts
            UNION ALL
            SELECT MIN(GREATEST(expires_at, cleanup_after)) FROM pending_file_uploads
        ) deadlines
        "#,
    )
    .bind(sync_enabled)
    .fetch_one(&store.pool)
    .await?;
    seconds
        .map(|seconds| Duration::try_from_secs_f64(seconds.max(0.0)))
        .transpose()
        .map_err(Into::into)
}
