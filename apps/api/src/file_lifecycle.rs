//! Crash recovery for uploads that never reached publication. Reservations stay
//! charged and hidden until object deletion is acknowledged. The five-minute
//! lease outlives the bounded PUT plus a grace period for uncertain provider I/O.
use crate::{error::AppResult, file_storage::FileStorage, store::Store};
use std::time::Duration;
use uuid::Uuid;

pub async fn cleanup_pending_uploads(
    store: &Store,
    storage: &FileStorage,
    user: Option<Uuid>,
    limit: usize,
) -> AppResult<()> {
    for _ in 0..limit {
        let mut tx = store.pool.begin().await?;
        let pending:Option<(Uuid,String)>=sqlx::query_as("SELECT f.id,f.object_key FROM pending_file_uploads p JOIN files f ON f.id=p.file_id WHERE p.expires_at<=clock_timestamp() AND p.cleanup_after<=clock_timestamp() AND ($1::uuid IS NULL OR f.user_id=$1) ORDER BY p.cleanup_after LIMIT 1 FOR UPDATE OF f SKIP LOCKED")
            .bind(user).fetch_optional(&mut *tx).await?;
        let Some((id, key)) = pending else {
            return Ok(());
        };
        // Explicit file-then-marker order matches metadata deletion/cascades.
        // Recheck after claiming the file: a concurrent publication may have
        // removed the marker, in which case this is now a ready file, not trash.
        let claimed:Option<Uuid>=sqlx::query_scalar("SELECT file_id FROM pending_file_uploads WHERE file_id=$1 AND expires_at<=clock_timestamp() AND cleanup_after<=clock_timestamp() FOR UPDATE SKIP LOCKED")
            .bind(id).fetch_optional(&mut *tx).await?;
        if claimed.is_none() {
            tx.commit().await?;
            continue;
        }
        if matches!(
            tokio::time::timeout(Duration::from_secs(10), storage.delete(&key)).await,
            Ok(Ok(()))
        ) {
            sqlx::query("DELETE FROM files WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("UPDATE pending_file_uploads SET cleanup_after=clock_timestamp()+INTERVAL '1 minute' WHERE file_id=$1").bind(id).execute(&mut *tx).await?;
            tracing::warn!("attachment cleanup deferred after storage failure");
        }
        tx.commit().await?;
    }
    Ok(())
}
