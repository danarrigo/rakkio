use std::time::Duration;

use tokio::sync::mpsc;

use crate::sync_service::SyncService;

const RETRY_POLL_INTERVAL: Duration = Duration::from_secs(5);
const MAX_JOBS_PER_DRAIN: usize = 32;

#[derive(Clone, Default)]
pub struct SyncDispatcher {
    sender: Option<mpsc::Sender<()>>,
}

impl SyncDispatcher {
    pub fn start(service: SyncService, storage: crate::file_storage::FileStorage) -> Self {
        let (sender, mut receiver) = mpsc::channel(1);
        tokio::spawn(async move {
            if let Err(error) = service.enqueue_periodic_work().await {
                tracing::error!(error = ?error, "startup synchronization enqueue failed");
            }
            loop {
                // Retry timestamps live in PostgreSQL. Recheck while this process is awake;
                // sleeping hosts still rely on the next startup or external worker trigger.
                tokio::select! {
                    message = receiver.recv() => {
                        if message.is_none() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(RETRY_POLL_INTERVAL) => {}
                }
                if let Err(error) = crate::file_lifecycle::cleanup_pending_uploads(
                    &service.store,
                    &storage,
                    None,
                    1,
                )
                .await
                {
                    tracing::error!(error=?error,"pending attachment cleanup failed");
                }
                for _ in 0..MAX_JOBS_PER_DRAIN {
                    // Finish an in-flight job, but do not start another after shutdown.
                    if receiver.is_closed() {
                        return;
                    }
                    match service.run_once().await {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(error) => {
                            tracing::error!(error = ?error, "immediate synchronization dispatch failed");
                            break;
                        }
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

    pub fn start_maintenance(
        store: crate::store::Store,
        storage: crate::file_storage::FileStorage,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel(1);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    message = receiver.recv() => if message.is_none() { break; },
                    _ = tokio::time::sleep(RETRY_POLL_INTERVAL) => {}
                }
                if let Err(error) = store.cleanup_task_delete_undos().await {
                    tracing::error!(error = ?error, "task deletion Undo cleanup failed");
                }
                if let Err(error) = store.cleanup_groups().await {
                    tracing::error!(error = ?error, "group metadata cleanup failed");
                }
                if let Err(error) = store.cleanup_discord().await {
                    tracing::error!(error = ?error, "Discord metadata cleanup failed");
                }
                if let Err(error) =
                    crate::file_lifecycle::cleanup_pending_uploads(&store, &storage, None, 1).await
                {
                    tracing::error!(error=?error,"pending attachment cleanup failed");
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
