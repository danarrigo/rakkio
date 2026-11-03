use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Redirect,
};
use chrono::{NaiveDate, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::{
        CurrentUser, DEVELOPMENT_USER_HEADER, SESSION_COOKIE, SessionCsrf, oauth_browser_secret,
        oauth_cookie_name, session_token,
    },
    error::{AppError, AppResult, ErrorResponse},
    extract::{ApiJson, ApiPath, ApiQuery},
    models::{
        CompleteDailyReviewRequest, CreateProjectRequest, CreateTaskRequest, DailyPlan,
        DailyReview, DailyReviewResponse, ExpectedVersionQuery, GoogleCallbackQuery,
        GoogleLoginQuery, HealthResponse, LabelList, PageQuery, Project, ProjectPage,
        ReorderTasksRequest, SessionResponse, StartDailyReviewRequest, Task, TaskListQuery,
        TaskPage, UpdateDailyFocusRequest, UpdateProjectRequest, UpdateUserSettingsRequest,
        UserSettings,
    },
    rate_limit::ClientAddress,
};

const CURRENT_TERMS_VERSION: &str = "2026-10-04";
const CURRENT_PRIVACY_VERSION: &str = "2026-11-03";

#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Process is healthy", body = HealthResponse)),
    tag = "system"
)]
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[utoipa::path(
    get,
    path = "/metrics",
    responses((status = 200, content_type = "text/plain", body = String)),
    tag = "system"
)]
pub async fn metrics(State(state): State<AppState>) -> ([(&'static str, &'static str); 1], String) {
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        state.metrics.render(),
    )
}

#[utoipa::path(
    get,
    path = "/ready",
    responses(
        (status = 200, description = "Dependencies are ready", body = HealthResponse),
        (status = 500, description = "A dependency is unavailable", body = ErrorResponse)
    ),
    tag = "system"
)]
pub async fn ready(State(state): State<AppState>) -> AppResult<Json<HealthResponse>> {
    state.store.ready().await?;
    Ok(Json(HealthResponse { status: "ready" }))
}

#[derive(Serialize)]
pub struct WorkerRunResponse {
    enqueued: u64,
    processed: bool,
}

pub async fn run_synchronization_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<WorkerRunResponse>> {
    let expected = state
        .worker_trigger_token
        .as_deref()
        .ok_or(AppError::NotConfigured("synchronization worker trigger"))?;
    if !worker_token_matches(&headers, expected) {
        return Err(AppError::Unauthorized);
    }
    let service = state
        .sync_service
        .as_ref()
        .ok_or(AppError::NotConfigured("Google synchronization"))?;
    let enqueued = service
        .enqueue_periodic_work()
        .await
        .map_err(AppError::Integration)?;
    let processed = service.run_once().await.map_err(AppError::Integration)?;
    state.sync_dispatcher.wake();
    Ok(Json(WorkerRunResponse {
        enqueued,
        processed,
    }))
}

fn worker_token_matches(headers: &HeaderMap, expected: &str) -> bool {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    Sha256::digest(token.as_bytes()) == Sha256::digest(expected.as_bytes())
}

#[utoipa::path(
    post,
    path = "/api/v1/development/session",
    responses(
        (status = 200, description = "Development user session", body = SessionResponse),
        (status = 401, description = "Development authentication is disabled", body = ErrorResponse)
    ),
    tag = "development"
)]
pub async fn development_session(
    State(state): State<AppState>,
    request_headers: HeaderMap,
) -> AppResult<(HeaderMap, Json<SessionResponse>)> {
    if !state.allow_insecure_dev_auth {
        return Err(AppError::Unauthorized);
    }

    let requested_user = request_headers
        .get(DEVELOPMENT_USER_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(Uuid::now_v7);
    let user = state.store.ensure_development_user(requested_user).await?;
    let session = state.store.create_session(user).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&session.token, state.secure_cookies)).map_err(
            |_| AppError::InvalidRequest {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "could not create session cookie".to_owned(),
            },
        )?,
    );
    Ok((headers, Json(session.response)))
}

#[utoipa::path(
    get,
    path = "/api/v1/session",
    responses(
        (status = 200, description = "Current user session", body = SessionResponse),
        (status = 401, body = ErrorResponse)
    ),
    tag = "authentication"
)]
pub async fn current_session(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    csrf: Option<Extension<SessionCsrf>>,
) -> AppResult<Json<SessionResponse>> {
    let csrf_token = csrf.map(|Extension(value)| value.0).unwrap_or_default();
    Ok(Json(SessionResponse {
        user: state.store.user_profile(user_id).await?,
        csrf_token,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/session/logout",
    responses(
        (status = 204, description = "Session ended"),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse)
    ),
    tag = "authentication"
)]
pub async fn logout(
    State(state): State<AppState>,
    CurrentUser(_user_id): CurrentUser,
    headers: HeaderMap,
) -> AppResult<(HeaderMap, StatusCode)> {
    if let Some(token) = session_token(&headers) {
        state.store.delete_session(token).await?;
    }
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&expired_session_cookie(state.secure_cookies)).map_err(|_| {
            AppError::InvalidRequest {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "could not expire session cookie".to_owned(),
            }
        })?,
    );
    Ok((response_headers, StatusCode::NO_CONTENT))
}

#[utoipa::path(
    post,
    path = "/api/v1/telemetry/reminder-delivered",
    responses((status = 204, description = "Reminder delivery recorded"), (status = 401, body = ErrorResponse), (status = 403, body = ErrorResponse)),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "system"
)]
pub async fn record_reminder_delivery(CurrentUser(_user_id): CurrentUser) -> StatusCode {
    metrics::counter!("prosepect_notification_deliveries_total", "channel" => "in_app")
        .increment(1);
    StatusCode::NO_CONTENT
}

#[utoipa::path(
    get,
    path = "/api/v1/settings",
    responses((status = 200, body = UserSettings), (status = 401, body = ErrorResponse)),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "settings"
)]
pub async fn get_settings(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
) -> AppResult<Json<UserSettings>> {
    Ok(Json(state.store.user_settings(user_id).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings",
    request_body = UpdateUserSettingsRequest,
    responses(
        (status = 200, body = UserSettings),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "settings"
)]
pub async fn update_settings(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiJson(request): ApiJson<UpdateUserSettingsRequest>,
) -> AppResult<Json<UserSettings>> {
    Ok(Json(
        state.store.update_user_settings(user_id, request).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/google/start",
    params(GoogleLoginQuery),
    responses(
        (status = 307, description = "Redirect to Google authorization"),
        (status = 422, body = ErrorResponse),
        (status = 503, body = ErrorResponse)
    ),
    tag = "authentication"
)]
pub async fn google_auth_start(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<GoogleLoginQuery>,
    headers: HeaderMap,
    ClientAddress(peer): ClientAddress,
) -> AppResult<(HeaderMap, Redirect)> {
    state
        .login_rate_limiter
        .check(&headers, peer, state.trust_proxy_headers)?;
    if query.terms_version != CURRENT_TERMS_VERSION
        || query.privacy_version != CURRENT_PRIVACY_VERSION
        || !query.age_confirmed
    {
        return Err(AppError::Validation(
            "the current Terms, Privacy Policy, and age requirement must be accepted".to_owned(),
        ));
    }
    let google = state
        .google_oauth
        .as_ref()
        .ok_or(AppError::NotConfigured("Google OAuth"))?;
    let login = google.begin_login().await.map_err(AppError::Integration)?;
    google_authorization_redirect(
        &state,
        &headers,
        login,
        None,
        "login",
        Some((CURRENT_TERMS_VERSION, CURRENT_PRIVACY_VERSION)),
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/google/calendar/start",
    responses(
        (status = 307, description = "Redirect to Google for incremental Calendar consent"),
        (status = 401, body = ErrorResponse),
        (status = 503, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "authentication"
)]
pub async fn google_calendar_connect_start(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    headers: HeaderMap,
    ClientAddress(peer): ClientAddress,
) -> AppResult<(HeaderMap, Redirect)> {
    state
        .login_rate_limiter
        .check(&headers, peer, state.trust_proxy_headers)?;
    let google = state
        .google_oauth
        .as_ref()
        .ok_or(AppError::NotConfigured("Google OAuth"))?;
    let login = google
        .begin_calendar_connection()
        .await
        .map_err(AppError::Integration)?;
    google_authorization_redirect(
        &state,
        &headers,
        login,
        Some(user_id),
        "calendar_connect",
        None,
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/google/tasks/start",
    responses(
        (status = 307, description = "Redirect to Google for incremental Tasks consent"),
        (status = 401, body = ErrorResponse),
        (status = 503, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "authentication"
)]
pub async fn google_tasks_connect_start(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    headers: HeaderMap,
    ClientAddress(peer): ClientAddress,
) -> AppResult<(HeaderMap, Redirect)> {
    state
        .login_rate_limiter
        .check(&headers, peer, state.trust_proxy_headers)?;
    let google = state
        .google_oauth
        .as_ref()
        .ok_or(AppError::NotConfigured("Google OAuth"))?;
    let login = google
        .begin_tasks_connection()
        .await
        .map_err(AppError::Integration)?;
    google_authorization_redirect(
        &state,
        &headers,
        login,
        Some(user_id),
        "tasks_connect",
        None,
    )
    .await
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/google/callback",
    params(GoogleCallbackQuery),
    responses(
        (status = 303, description = "Google account authenticated"),
        (status = 403, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
        (status = 503, body = ErrorResponse)
    ),
    tag = "authentication"
)]
pub async fn google_auth_callback(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<GoogleCallbackQuery>,
    headers: HeaderMap,
) -> AppResult<(HeaderMap, Redirect)> {
    if query.error.is_some() {
        return Err(AppError::Forbidden("Google sign-in was not completed"));
    }
    let code = query.code.ok_or(AppError::Forbidden(
        "Google did not return an authorization code",
    ))?;
    let state_value = query.state.ok_or(AppError::Forbidden(
        "Google did not return the sign-in state",
    ))?;
    let browser_secret = oauth_browser_secret(&headers, state.secure_cookies).ok_or(
        AppError::Forbidden("the Google sign-in request expired or is invalid"),
    )?;
    let attempt = state
        .store
        .consume_google_login_attempt(&state_value, browser_secret)
        .await?;
    if let Some(user_id) = attempt.user_id {
        let token = session_token(&headers).ok_or(AppError::Unauthorized)?;
        if state.store.authenticate_session(token).await?.user_id != user_id {
            return Err(AppError::Forbidden(
                "the Google connection belongs to a different session",
            ));
        }
    }
    let google = state
        .google_oauth
        .as_ref()
        .ok_or(AppError::NotConfigured("Google OAuth"))?;
    let login = google
        .complete_login(code, attempt.nonce, attempt.pkce_verifier)
        .await
        .map_err(AppError::Integration)?;
    if attempt.purpose == "tasks_connect" {
        let user_id = attempt.user_id.ok_or(AppError::Forbidden(
            "Google Tasks connection did not identify a user",
        ))?;
        if !login
            .scopes
            .iter()
            .any(|scope| scope == crate::google_tasks::TASKS_SCOPE)
        {
            return Err(AppError::Forbidden(
                "Google Tasks permission was not granted",
            ));
        }
        // Consent alone must not enable copying tasks or create a provider list.
        // The user explicitly enables the independent connection in Settings.
        state
            .store
            .update_google_credentials(user_id, login)
            .await?;
        return Ok((
            HeaderMap::new(),
            Redirect::to(&format!("{}/settings", state.app_url.trim_end_matches('/'))),
        ));
    }
    if attempt.purpose == "calendar_connect" {
        let user_id = attempt.user_id.ok_or(AppError::Forbidden(
            "Google Calendar connection did not identify a user",
        ))?;
        state
            .store
            .update_google_credentials(user_id, login)
            .await?;
        state
            .store
            .enqueue_sync(
                user_id,
                None,
                "calendar_discovery",
                &format!("calendar-connect:{user_id}:{}", Utc::now().timestamp()),
            )
            .await?;
        state.sync_dispatcher.wake();
        return Ok((
            HeaderMap::new(),
            Redirect::to(&format!("{}/settings", state.app_url.trim_end_matches('/'))),
        ));
    }
    let (Some(terms_version), Some(privacy_version)) = (
        attempt.terms_version.as_deref(),
        attempt.privacy_version.as_deref(),
    ) else {
        return Ok((
            HeaderMap::new(),
            authentication_error_redirect(&state.app_url, "legal_acceptance_required"),
        ));
    };
    if !attempt.age_confirmed {
        return Ok((
            HeaderMap::new(),
            authentication_error_redirect(&state.app_url, "legal_acceptance_required"),
        ));
    }
    if state.invite_only && !state.store.email_can_sign_in(&login.email).await? {
        return Ok((
            HeaderMap::new(),
            authentication_error_redirect(&state.app_url, "invite_required"),
        ));
    }
    let login_email = login.email.clone();
    let user = match state
        .store
        .upsert_google_user(login, state.max_user_accounts)
        .await
    {
        Ok(user) => user,
        Err(AppError::Forbidden(_)) => {
            return Ok((
                HeaderMap::new(),
                authentication_error_redirect(&state.app_url, "capacity_reached"),
            ));
        }
        Err(error) => return Err(error),
    };
    if state.invite_only {
        state
            .store
            .consume_account_invite(&login_email, user.id)
            .await?;
    }
    state
        .store
        .record_legal_acceptance(
            user.id,
            terms_version,
            privacy_version,
            attempt.age_confirmed,
        )
        .await?;
    let session = state.store.create_session(user).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&session.token, state.secure_cookies)).map_err(
            |_| AppError::InvalidRequest {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "could not create session cookie".to_owned(),
            },
        )?,
    );
    Ok((headers, Redirect::to(&state.app_url)))
}

async fn google_authorization_redirect(
    state: &AppState,
    headers: &HeaderMap,
    login: crate::google_auth::GoogleLoginStart,
    user_id: Option<Uuid>,
    purpose: &str,
    legal_versions: Option<(&str, &str)>,
) -> AppResult<(HeaderMap, Redirect)> {
    // Reuse the protected browser secret so independent tabs can complete their
    // own single-use flows. It is never placed in the authorization URL.
    let secret = oauth_browser_secret(headers, state.secure_cookies)
        .map(str::to_owned)
        .unwrap_or_else(|| oauth2::CsrfToken::new_random_len(32).secret().clone());
    state
        .store
        .save_google_login_attempt(&login, user_id, purpose, legal_versions, &secret)
        .await?;
    let cookie = format!(
        "{}={secret}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{}",
        oauth_cookie_name(state.secure_cookies),
        if state.secure_cookies { "; Secure" } else { "" }
    );
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| AppError::InvalidRequest {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "could not create OAuth cookie".to_owned(),
        })?,
    );
    Ok((
        response_headers,
        Redirect::temporary(&login.authorization_url),
    ))
}

fn authentication_error_redirect(app_url: &str, error: &str) -> Redirect {
    Redirect::to(&format!(
        "{}/?auth_error={error}",
        app_url.trim_end_matches('/')
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/daily-plans/{date}",
    params(("date" = NaiveDate, Path, description = "Local planning date")),
    responses(
        (status = 200, description = "Daily plan", body = DailyPlan),
        (status = 401, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "daily planning"
)]
pub async fn get_daily_plan(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiPath(date): ApiPath<NaiveDate>,
) -> AppResult<Json<DailyPlan>> {
    Ok(Json(state.store.daily_plan(user_id, date).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/daily-plans/{date}/focus",
    params(("date" = NaiveDate, Path, description = "Local planning date")),
    request_body = UpdateDailyFocusRequest,
    responses(
        (status = 200, description = "Updated daily plan", body = DailyPlan),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "daily planning"
)]
pub async fn update_daily_focus(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiPath(date): ApiPath<NaiveDate>,
    ApiJson(request): ApiJson<UpdateDailyFocusRequest>,
) -> AppResult<Json<DailyPlan>> {
    Ok(Json(
        state
            .store
            .update_daily_focus(user_id, date, request)
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/daily-reviews/{date}/start",
    params(("date" = NaiveDate, Path, description = "Local review date")),
    request_body = StartDailyReviewRequest,
    responses(
        (status = 200, body = DailyReviewResponse),
        (status = 401, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "daily planning"
)]
pub async fn start_daily_review(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiPath(date): ApiPath<NaiveDate>,
    ApiJson(request): ApiJson<StartDailyReviewRequest>,
) -> AppResult<Json<DailyReviewResponse>> {
    Ok(Json(
        state
            .store
            .start_daily_review(user_id, date, request.manual)
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/daily-reviews/{date}/complete",
    params(("date" = NaiveDate, Path, description = "Local review date")),
    request_body = CompleteDailyReviewRequest,
    responses(
        (status = 200, body = DailyReview),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "daily planning"
)]
pub async fn complete_daily_review(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiPath(date): ApiPath<NaiveDate>,
    ApiJson(request): ApiJson<CompleteDailyReviewRequest>,
) -> AppResult<Json<DailyReview>> {
    Ok(Json(
        state
            .store
            .complete_daily_review(user_id, date, request)
            .await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/labels",
    responses(
        (status = 200, description = "Reusable task labels", body = LabelList),
        (status = 401, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "tasks"
)]
pub async fn list_labels(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
) -> AppResult<Json<LabelList>> {
    Ok(Json(state.store.list_labels(user_id).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects",
    request_body = CreateProjectRequest,
    responses(
        (status = 201, description = "Project created", body = Project),
        (status = 401, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "projects"
)]
pub async fn create_project(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiJson(request): ApiJson<CreateProjectRequest>,
) -> AppResult<(StatusCode, Json<Project>)> {
    let project = state.store.create_project(user_id, request).await?;
    Ok((StatusCode::CREATED, Json(project)))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects",
    params(PageQuery),
    responses(
        (status = 200, description = "Projects", body = ProjectPage),
        (status = 401, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "projects"
)]
pub async fn list_projects(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiQuery(query): ApiQuery<PageQuery>,
) -> AppResult<Json<ProjectPage>> {
    let page = state
        .store
        .list_projects(user_id, query.cursor, query.normalized_limit())
        .await?;
    Ok(Json(page))
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project_id}",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = UpdateProjectRequest,
    responses(
        (status = 200, description = "Project updated", body = Project),
        (status = 401, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "projects"
)]
pub async fn update_project(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateProjectRequest>,
) -> AppResult<Json<Project>> {
    let project = state
        .store
        .update_project(user_id, project_id, request)
        .await?;
    Ok(Json(project))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project_id}",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ExpectedVersionQuery
    ),
    responses(
        (status = 204, description = "Project deleted"),
        (status = 401, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "projects"
)]
pub async fn delete_project(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ExpectedVersionQuery>,
) -> AppResult<StatusCode> {
    state
        .store
        .delete_project(user_id, project_id, query.expected_version)
        .await?;
    state.sync_dispatcher.wake();
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/tasks",
    request_body = CreateTaskRequest,
    responses(
        (status = 201, description = "Task created", body = Task),
        (status = 401, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "tasks"
)]
pub async fn create_task(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiJson(request): ApiJson<CreateTaskRequest>,
) -> AppResult<(StatusCode, Json<Task>)> {
    state
        .action_rate_limiter
        .check_key(&format!("capture:{user_id}"))?;
    let task = state.store.create_task(user_id, request).await?;
    state.sync_dispatcher.wake();
    Ok((StatusCode::CREATED, Json(task)))
}

#[utoipa::path(
    get,
    path = "/api/v1/tasks",
    params(TaskListQuery),
    responses(
        (status = 200, description = "Tasks", body = TaskPage),
        (status = 401, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "tasks"
)]
pub async fn list_tasks(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiQuery(query): ApiQuery<TaskListQuery>,
) -> AppResult<Json<TaskPage>> {
    let page = state
        .store
        .list_tasks(
            user_id,
            query.project_id,
            query.cursor,
            query.normalized_limit(),
        )
        .await?;
    Ok(Json(page))
}

#[utoipa::path(
    put,
    path = "/api/v1/tasks/order",
    request_body = ReorderTasksRequest,
    responses(
        (status = 204, description = "Task order updated"),
        (status = 401, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "tasks"
)]
pub async fn reorder_tasks(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiJson(request): ApiJson<ReorderTasksRequest>,
) -> AppResult<StatusCode> {
    state.store.reorder_tasks(user_id, request).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/v1/tasks/{task_id}",
    params(("task_id" = Uuid, Path, description = "Task identifier")),
    request_body = crate::task_recurrence::ScopedTaskUpdateRequest,
    responses(
        (status = 200, description = "Task updated", body = Task),
        (status = 401, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "tasks"
)]
pub async fn update_task(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiPath(task_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<crate::task_recurrence::ScopedTaskUpdateRequest>,
) -> AppResult<Json<Task>> {
    let task = state
        .store
        .update_task_scoped(user_id, task_id, request.task, request.recurrence_scope)
        .await?;
    state.sync_dispatcher.wake();
    Ok(Json(task))
}

#[utoipa::path(
    delete,
    path = "/api/v1/tasks/{task_id}",
    params(
        ("task_id" = Uuid, Path, description = "Task identifier"),
        ExpectedVersionQuery
    ),
    responses(
        (status = 204, description = "Task deleted"),
        (status = 401, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 422, body = ErrorResponse)
    ),
    security(("session_cookie" = []), ("development_user" = [])),
    tag = "tasks"
)]
pub async fn delete_task(
    State(state): State<AppState>,
    CurrentUser(user_id): CurrentUser,
    ApiPath(task_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ExpectedVersionQuery>,
) -> AppResult<StatusCode> {
    state
        .store
        .delete_task(user_id, task_id, query.expected_version)
        .await?;
    state.sync_dispatcher.wake();
    Ok(StatusCode::NO_CONTENT)
}

pub async fn route_not_found() -> AppError {
    AppError::RouteNotFound
}

pub async fn method_not_allowed() -> AppError {
    AppError::MethodNotAllowed
}

fn session_cookie(token: &str, secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000{}",
        if secure { "; Secure" } else { "" }
    )
}

fn expired_session_cookie(secure: bool) -> String {
    format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    )
}
