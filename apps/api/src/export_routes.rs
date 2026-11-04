use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Response, StatusCode, header},
};

use crate::{
    app::AppState,
    auth::{CurrentUser, SESSION_COOKIE},
    error::{AppError, AppResult, ErrorResponse},
    extract::ApiJson,
    models::DeleteAccountRequest,
};

#[utoipa::path(
    get,
    path = "/api/v1/exports/json",
    responses((status = 200, content_type = "application/json", body = String), (status = 401, body = ErrorResponse)),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "exports"
)]
pub async fn export_json(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
) -> AppResult<Response<Body>> {
    download(
        state.store.export_json(user_id).await?,
        "application/json; charset=utf-8",
        "rakkio-export.json",
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/exports/tasks.csv",
    responses((status = 200, content_type = "text/csv", body = String), (status = 401, body = ErrorResponse)),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "exports"
)]
pub async fn export_tasks_csv(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
) -> AppResult<Response<Body>> {
    download(
        state.store.export_tasks_csv(user_id).await?,
        "text/csv; charset=utf-8",
        "rakkio-tasks.csv",
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/exports/notes.md",
    responses((status = 200, content_type = "text/markdown", body = String), (status = 401, body = ErrorResponse)),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "exports"
)]
pub async fn export_notes_markdown(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
) -> AppResult<Response<Body>> {
    download(
        state.store.export_notes_markdown(user_id).await?,
        "text/markdown; charset=utf-8",
        "rakkio-notes.md",
    )
}

#[utoipa::path(
    get,
    path = "/api/v1/exports/calendars.ics",
    responses((status = 200, content_type = "text/calendar", body = String), (status = 401, body = ErrorResponse)),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "exports"
)]
pub async fn export_calendars_ics(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
) -> AppResult<Response<Body>> {
    download(
        state.store.export_calendars_ics(user_id).await?,
        "text/calendar; charset=utf-8",
        "rakkio-calendars.ics",
    )
}

#[utoipa::path(
    delete,
    path = "/api/v1/account",
    request_body = DeleteAccountRequest,
    responses(
        (status = 204, description = "Account and user-owned data deleted"),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "account"
)]
pub async fn delete_account(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiJson(request): ApiJson<DeleteAccountRequest>,
) -> AppResult<Response<Body>> {
    if request.confirmation != "DELETE" {
        return Err(AppError::Validation(
            "account deletion confirmation must equal DELETE".to_owned(),
        ));
    }
    state.store.prepare_account_deletion(user_id).await?;
    match state.store.google_credentials(user_id).await {
        Ok(credentials) => {
            let google = state
                .google_oauth
                .as_ref()
                .ok_or(AppError::NotConfigured("Google OAuth"))?;
            google
                .revoke(&credentials)
                .await
                .map_err(AppError::Integration)?;
        }
        Err(AppError::NotConfigured(_)) => {}
        Err(error) => return Err(error),
    }
    for object_key in state.store.account_object_keys(user_id).await? {
        state
            .file_storage
            .delete(&object_key)
            .await
            .map_err(|error| {
                tracing::error!(error = ?error, object_key, "failed to delete account file");
                AppError::InvalidRequest {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "could not delete account files".to_owned(),
                }
            })?;
    }
    state.store.delete_account(user_id, "DELETE").await?;
    let cookie = format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if state.secure_cookies { "; Secure" } else { "" }
    );
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header(header::SET_COOKIE, cookie)
        .body(Body::empty())
        .map_err(response_error)
}

fn download(content: Vec<u8>, content_type: &str, filename: &str) -> AppResult<Response<Body>> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
                .map_err(response_error)?,
        )
        .body(Body::from(content))
        .map_err(response_error)
}

fn response_error(error: impl std::fmt::Debug) -> AppError {
    tracing::error!(error = ?error, "failed to create download response");
    AppError::InvalidRequest {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "could not create download response".to_owned(),
    }
}
