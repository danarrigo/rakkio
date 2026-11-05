use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{FileList, FileListQuery, FileRecord},
    store::Store,
};

impl Store {
    pub async fn file_usage_bytes(&self, user_id: Uuid) -> AppResult<i64> {
        sqlx::query_scalar(
            "SELECT COALESCE(SUM(byte_size), 0)::BIGINT FROM files WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_file_metadata(
        &self,
        user_id: Uuid,
        project_id: Option<Uuid>,
        task_id: Option<Uuid>,
        note_id: Option<Uuid>,
        event_id: Option<Uuid>,
        object_key: &str,
        filename: &str,
        content_type: &str,
        byte_size: i64,
        max_user_storage_bytes: i64,
        max_total_storage_bytes: i64,
    ) -> AppResult<FileRecord> {
        validate_one_parent(project_id, task_id, note_id, event_id)?;
        self.verify_file_parent(user_id, project_id, task_id, note_id, event_id)
            .await?;

        let mut transaction = self.pool.begin().await?;
        let deleting: bool =
            sqlx::query_scalar("SELECT group_admission_blocked FROM users WHERE id=$1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(AppError::Unauthorized)?;
        if deleting {
            return Err(AppError::Conflict(
                "Account erasure is pending. New uploads are blocked.".into(),
            ));
        }
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(741_733_074_i64)
            .execute(&mut *transaction)
            .await?;
        let user_used_bytes = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(byte_size), 0)::BIGINT FROM files WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_one(&mut *transaction)
        .await?;
        if byte_size > max_user_storage_bytes.saturating_sub(user_used_bytes) {
            return Err(AppError::InvalidRequest {
                status: axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                message: "personal attachment storage quota exceeded".to_owned(),
            });
        }
        let total_used_bytes =
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(SUM(byte_size), 0)::BIGINT FROM files")
                .fetch_one(&mut *transaction)
                .await?;
        if byte_size > max_total_storage_bytes.saturating_sub(total_used_bytes) {
            return Err(AppError::InvalidRequest {
                status: axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                message: "attachment storage quota exceeded".to_owned(),
            });
        }

        let file = sqlx::query_as::<_, FileRecord>(
            r#"
            INSERT INTO files (
                id, user_id, project_id, task_id, note_id, event_id,
                object_key, filename, content_type, byte_size
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING id, project_id, task_id, note_id, event_id, filename,
                      content_type, byte_size, created_at
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(project_id)
        .bind(task_id)
        .bind(note_id)
        .bind(event_id)
        .bind(object_key)
        .bind(filename)
        .bind(content_type)
        .bind(byte_size)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query("INSERT INTO pending_file_uploads(file_id) VALUES($1)")
            .bind(file.id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(file)
    }

    /// Publish only after the object PUT succeeded, while serializing with erasure.
    pub async fn finish_file_upload(&self, user_id: Uuid, file_id: Uuid) -> AppResult<bool> {
        let mut tx = self.pool.begin().await?;
        let deleting: Option<bool> =
            sqlx::query_scalar("SELECT group_admission_blocked FROM users WHERE id=$1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        if deleting != Some(false) {
            return Ok(false);
        }
        let published=sqlx::query("DELETE FROM pending_file_uploads p USING files f WHERE p.file_id=f.id AND f.user_id=$1 AND f.id=$2 AND p.expires_at>clock_timestamp()")
            .bind(user_id).bind(file_id).execute(&mut *tx).await?.rows_affected()==1;
        tx.commit().await?;
        Ok(published)
    }

    pub async fn list_files(&self, user_id: Uuid, query: FileListQuery) -> AppResult<FileList> {
        validate_one_parent(
            query.project_id,
            query.task_id,
            query.note_id,
            query.event_id,
        )?;
        let items = sqlx::query_as::<_, FileRecord>(
            r#"
            SELECT id, project_id, task_id, note_id, event_id, filename,
                   content_type, byte_size, created_at
            FROM files
            WHERE user_id = $1
              AND NOT EXISTS(SELECT 1 FROM pending_file_uploads p WHERE p.file_id=files.id)
              AND ($2::UUID IS NULL OR project_id = $2)
              AND ($3::UUID IS NULL OR task_id = $3)
              AND ($4::UUID IS NULL OR note_id = $4)
              AND ($5::UUID IS NULL OR event_id = $5)
            ORDER BY created_at DESC, id DESC
            "#,
        )
        .bind(user_id)
        .bind(query.project_id)
        .bind(query.task_id)
        .bind(query.note_id)
        .bind(query.event_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(FileList { items })
    }

    pub async fn file_download(
        &self,
        user_id: Uuid,
        file_id: Uuid,
    ) -> AppResult<(FileRecord, String)> {
        sqlx::query_as::<_, FileDownload>(
            r#"
            SELECT id, project_id, task_id, note_id, event_id, filename,
                   content_type, byte_size, created_at, object_key
            FROM files WHERE user_id = $1 AND id = $2
              AND NOT EXISTS(SELECT 1 FROM pending_file_uploads p WHERE p.file_id=files.id)
            "#,
        )
        .bind(user_id)
        .bind(file_id)
        .fetch_optional(&self.pool)
        .await?
        .map(|file| (file.record(), file.object_key))
        .ok_or(AppError::NotFound("file"))
    }

    pub async fn delete_file_metadata(&self, user_id: Uuid, file_id: Uuid) -> AppResult<String> {
        sqlx::query_scalar::<_, String>(
            "DELETE FROM files WHERE user_id = $1 AND id = $2 RETURNING object_key",
        )
        .bind(user_id)
        .bind(file_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AppError::NotFound("file"))
    }

    pub async fn account_object_keys(&self, user_id: Uuid) -> AppResult<Vec<String>> {
        sqlx::query_scalar("SELECT object_key FROM files WHERE user_id = $1")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::from)
    }

    async fn verify_file_parent(
        &self,
        user_id: Uuid,
        project_id: Option<Uuid>,
        task_id: Option<Uuid>,
        note_id: Option<Uuid>,
        event_id: Option<Uuid>,
    ) -> AppResult<()> {
        let exists = if let Some(id) = project_id {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE user_id = $1 AND id = $2)",
            )
            .bind(user_id)
            .bind(id)
            .fetch_one(&self.pool)
            .await?
        } else if let Some(id) = task_id {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE user_id = $1 AND id = $2)")
                .bind(user_id)
                .bind(id)
                .fetch_one(&self.pool)
                .await?
        } else if let Some(id) = note_id {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notes WHERE user_id = $1 AND id = $2)")
                .bind(user_id)
                .bind(id)
                .fetch_one(&self.pool)
                .await?
        } else if let Some(id) = event_id {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM calendar_events WHERE user_id = $1 AND id = $2)",
            )
            .bind(user_id)
            .bind(id)
            .fetch_one(&self.pool)
            .await?
        } else {
            true
        };
        if exists {
            Ok(())
        } else {
            Err(AppError::NotFound("file parent"))
        }
    }
}

#[derive(sqlx::FromRow)]
struct FileDownload {
    id: Uuid,
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    note_id: Option<Uuid>,
    event_id: Option<Uuid>,
    filename: String,
    content_type: String,
    byte_size: i64,
    created_at: chrono::DateTime<chrono::Utc>,
    object_key: String,
}

impl FileDownload {
    fn record(&self) -> FileRecord {
        FileRecord {
            id: self.id,
            project_id: self.project_id,
            task_id: self.task_id,
            note_id: self.note_id,
            event_id: self.event_id,
            filename: self.filename.clone(),
            content_type: self.content_type.clone(),
            byte_size: self.byte_size,
            created_at: self.created_at,
        }
    }
}

fn validate_one_parent(
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    note_id: Option<Uuid>,
    event_id: Option<Uuid>,
) -> AppResult<()> {
    if [project_id, task_id, note_id, event_id]
        .into_iter()
        .flatten()
        .count()
        > 1
    {
        return Err(AppError::Validation(
            "a file can be linked to at most one item".to_owned(),
        ));
    }
    Ok(())
}
