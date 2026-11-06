use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderValue, Response, StatusCode, header},
    response::Json,
};
use bytes::{Bytes, BytesMut};
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::CurrentUser,
    error::{AppError, AppResult, ErrorResponse},
    models::{FileList, FileListQuery, FileRecord, FileUsage},
};

#[utoipa::path(
    get,
    path = "/api/v1/files/usage",
    responses((status = 200, body = FileUsage), (status = 401, body = ErrorResponse)),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "files"
)]
pub async fn file_usage(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
) -> AppResult<Json<FileUsage>> {
    Ok(Json(FileUsage {
        used_bytes: state.store.file_usage_bytes(user_id).await?,
        max_user_storage_bytes: state.max_user_file_storage_bytes,
        max_file_size_bytes: state.max_file_size_bytes,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/files",
    params(FileListQuery),
    responses((status = 200, body = FileList), (status = 401, body = ErrorResponse)),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "files"
)]
pub async fn list_files(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    Query(query): Query<FileListQuery>,
) -> AppResult<Json<FileList>> {
    Ok(Json(state.store.list_files(user_id, query).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/files",
    request_body(content = String, content_type = "multipart/form-data"),
    responses(
        (status = 201, body = FileRecord),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 413, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "files"
)]
pub async fn upload_file(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<FileRecord>)> {
    state
        .login_rate_limiter
        .check_key(&format!("upload:{user_id}"))?;
    let _permit = state.request_limits.large_request()?;
    let mut upload: Option<Upload> = None;
    let mut project_id = None;
    let mut task_id = None;
    let mut note_id = None;
    let mut event_id = None;

    let mut seen = std::collections::HashSet::new();
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while let Some(field) = multipart.next_field().await.map_err(invalid_multipart)? {
            let name = field.name().unwrap_or_default().to_owned();
            if !matches!(
                name.as_str(),
                "file" | "project_id" | "task_id" | "note_id" | "event_id"
            ) || !seen.insert(name.clone())
            {
                return Err(AppError::Validation(
                    "unexpected or duplicate upload field".to_owned(),
                ));
            }
            if name == "file" {
                let filename = field
                    .file_name()
                    .map(clean_filename)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| AppError::Validation("file must have a filename".to_owned()))?;
                let content_type = field
                    .content_type()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "application/octet-stream".to_owned());
                content_type
                    .parse::<mime::Mime>()
                    .map_err(|_| AppError::Validation("file content type is invalid".to_owned()))?;
                let bytes = bounded_field(field, state.max_file_size_bytes).await?;
                upload = Some(Upload {
                    filename,
                    content_type,
                    bytes,
                });
            } else {
                let bytes = bounded_field(field, 64).await?;
                let value = std::str::from_utf8(&bytes).map_err(|_| {
                    AppError::Validation("file link identifier must be UTF-8".to_owned())
                })?;
                let id = parse_optional_id(value)?;
                match name.as_str() {
                    "project_id" => project_id = id,
                    "task_id" => task_id = id,
                    "note_id" => note_id = id,
                    "event_id" => event_id = id,
                    _ => {}
                }
            }
        }

        Ok::<_, AppError>(())
    })
    .await
    .map_err(|_| AppError::InvalidRequest {
        status: StatusCode::REQUEST_TIMEOUT,
        message: "file upload timed out".to_owned(),
    })??;
    let upload = upload.ok_or_else(|| AppError::Validation("file is required".to_owned()))?;
    crate::file_lifecycle::cleanup_pending_uploads(&state.store, &state.file_storage, None, 1)
        .await?;
    let file_id = Uuid::now_v7();
    let object_key = format!("{user_id}-{file_id}");
    let metadata = state
        .store
        .create_file_metadata(
            user_id,
            project_id,
            task_id,
            note_id,
            event_id,
            &object_key,
            &upload.filename,
            &upload.content_type,
            upload.bytes.len() as i64,
            state.max_user_file_storage_bytes,
            state.max_total_file_storage_bytes,
        )
        .await?;
    // A failed/timed-out PUT is uncertain: keep its durable reservation until
    // grace-period cleanup confirms deletion, rather than releasing quota now.
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        state.file_storage.put(&object_key, upload.bytes),
    )
    .await
    .map_err(storage_error)?
    .map_err(storage_error)?;
    if !state.store.finish_file_upload(user_id, metadata.id).await? {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            state.file_storage.delete(&object_key),
        )
        .await
        .map_err(storage_error)?
        .map_err(storage_error)?;
        match state.store.delete_file_metadata(user_id, metadata.id).await {
            Ok(_) | Err(AppError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
        return Err(AppError::Conflict(
            "Upload was not published because account erasure or cleanup started.".into(),
        ));
    }
    Ok((StatusCode::CREATED, Json(metadata)))
}

#[utoipa::path(
    get,
    path = "/api/v1/files/{file_id}/download",
    params(("file_id" = Uuid, Path, description = "File identifier")),
    responses(
        (status = 200, content_type = "application/octet-stream", body = String),
        (status = 401, body = ErrorResponse),
        (status = 404, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "files"
)]
pub async fn download_file(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    Path(file_id): Path<Uuid>,
) -> AppResult<Response<Body>> {
    let (file, object_key) = state.store.file_download(user_id, file_id).await?;
    if let Some(url) = state
        .file_storage
        .signed_download_url(&object_key)
        .await
        .map_err(storage_error)?
    {
        return Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, url)
            .body(Body::empty())
            .map_err(response_error);
    }
    let bytes = state
        .file_storage
        .get(&object_key)
        .await
        .map_err(storage_error)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, file.content_type)
        .header(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!(
                "attachment; filename=\"{}\"",
                file.filename.replace(['"', '\r', '\n'], "_")
            ))
            .map_err(response_error)?,
        )
        .body(Body::from(bytes))
        .map_err(response_error)
}

#[utoipa::path(
    delete,
    path = "/api/v1/files/{file_id}",
    params(("file_id" = Uuid, Path, description = "File identifier")),
    responses(
        (status = 204, description = "File deleted"),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "files"
)]
pub async fn delete_file(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    Path(file_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let (_, object_key) = state.store.file_download(user_id, file_id).await?;
    state
        .file_storage
        .delete(&object_key)
        .await
        .map_err(storage_error)?;
    state.store.delete_file_metadata(user_id, file_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

struct Upload {
    filename: String,
    content_type: String,
    bytes: Bytes,
}

async fn bounded_field(
    mut field: axum::extract::multipart::Field<'_>,
    maximum: usize,
) -> AppResult<Bytes> {
    let mut bytes = BytesMut::new();
    while let Some(chunk) = field.chunk().await.map_err(invalid_multipart)? {
        if chunk.len() > maximum.saturating_sub(bytes.len()) {
            return Err(AppError::InvalidRequest {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                message: format!("upload field must not exceed {maximum} bytes"),
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.freeze())
}

fn parse_optional_id(value: &str) -> AppResult<Option<Uuid>> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    value
        .parse()
        .map(Some)
        .map_err(|_| AppError::Validation("file link identifier must be a UUID".to_owned()))
}

fn clean_filename(value: &str) -> String {
    value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .replace(['\0', '\r', '\n'], "_")
        .chars()
        .take(255)
        .collect()
}

fn invalid_multipart(error: axum::extract::multipart::MultipartError) -> AppError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        AppError::InvalidRequest {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "multipart upload exceeds the request limit".to_owned(),
        }
    } else {
        AppError::Validation("invalid multipart file upload".to_owned())
    }
}

fn storage_error(_error: impl std::fmt::Debug) -> AppError {
    // Provider bodies/URLs can contain private identifiers or signed credentials.
    tracing::error!("file storage operation failed");
    AppError::InvalidRequest {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "file storage operation failed".to_owned(),
    }
}

fn response_error(error: impl std::fmt::Debug) -> AppError {
    tracing::error!(error = ?error, "failed to create file response");
    AppError::InvalidRequest {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "could not create file response".to_owned(),
    }
}
