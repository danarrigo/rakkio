use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, HeaderValue, Method, header},
    middleware,
    routing::{get, post, put},
};
use tower_http::{
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use utoipa::{
    Modify, OpenApi,
    openapi::security::{ApiKey, ApiKeyValue, SecurityScheme},
};
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    auth::{CSRF_HEADER, DEVELOPMENT_USER_HEADER, SESSION_COOKIE},
    calendar_move_routes, calendar_routes,
    config::{Config, Environment},
    error::{ErrorBody, ErrorResponse},
    export_routes, file_routes,
    file_storage::FileStorage,
    google_auth::GoogleOAuth,
    google_tasks_routes, import_routes,
    models::{
        ActivityEntry, ActivityList, Calendar, CalendarEvent, CalendarEventList, CalendarList,
        CalendarSource, CompleteDailyReviewRequest, CreateCalendarEventRequest,
        CreateCalendarRequest, CreateNoteRequest, CreateProjectRequest,
        CreateSynchronizationRequest, CreateTaskRequest, DailyPlan, DailyReview,
        DailyReviewResponse, DailyReviewStatus, DeleteAccountRequest, EventRecurrence, FileList,
        FileRecord, FileUsage, GoogleIntegrationStatus, HealthResponse, LabelList, Note, NoteList,
        Project, ProjectPage, ProjectStatus, ReorderTasksRequest, ResolveSyncConflictRequest,
        ReviewDecisionAction, ReviewTaskDecision, SearchResult, SearchResultKind, SearchResultList,
        SessionResponse, StartDailyReviewRequest, SyncConflict, SyncConflictList,
        SyncConflictPolicy, Synchronization, Task, TaskPage, TaskPriority, TaskRecurrence,
        TaskStatus, ThemePreference, TodoistImportRequest, TodoistImportResult, TodoistImportTask,
        UpdateCalendarEventRequest, UpdateCalendarRequest, UpdateDailyFocusRequest,
        UpdateNoteRequest, UpdateProjectRequest, UpdateTaskRequest, UpdateUserSettingsRequest,
        UserProfile, UserSettings,
    },
    note_routes, observability, operations,
    rate_limit::LoginRateLimiter,
    routes, saved_views,
    store::Store,
    sync_dispatcher::SyncDispatcher,
    sync_routes,
    sync_service::SyncService,
    task_delete_routes,
};

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub allow_insecure_dev_auth: bool,
    pub invite_only: bool,
    pub groups_enabled: bool,
    pub trust_proxy_headers: bool,
    pub login_rate_limiter: LoginRateLimiter,
    pub action_rate_limiter: LoginRateLimiter,
    pub request_limits: crate::request_limits::RequestLimits,
    pub secure_cookies: bool,
    pub app_url: String,
    pub google_oauth: Option<GoogleOAuth>,
    pub discord: Option<crate::discord::Discord>,
    pub file_storage: FileStorage,
    pub max_file_size_bytes: usize,
    pub max_user_file_storage_bytes: i64,
    pub max_total_file_storage_bytes: i64,
    pub max_user_accounts: Option<i64>,
    pub admin_user_ids: std::collections::HashSet<uuid::Uuid>,
    pub worker_trigger_token: Option<String>,
    pub sync_service: Option<SyncService>,
    pub sync_dispatcher: SyncDispatcher,
    pub metrics: metrics_exporter_prometheus::PrometheusHandle,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "rakkio API",
        version = "0.1.0",
        description = "Personal productivity API for rakkio"
    ),
    paths(
        crate::group_assignments::list,
        crate::group_assignments::create,
        crate::group_assignments::get,
        crate::group_assignments::update,
        crate::group_assignments::complete,
        crate::group_assignments::report,
        crate::group_assignments::preferences,
        crate::group_assignments::share,
        crate::group_assignments::delete,
        crate::group_assignments::restore,
        crate::group_assignments::undos,
        crate::group_assignments::usage,
        crate::groups::capability,
        crate::groups::list,
        crate::groups::create,
        crate::groups::detail,
        crate::groups::mutate,
        crate::groups::invite,
        crate::groups::revoke,
        crate::groups::inbox,
        crate::groups::decline,
        crate::groups::preview,
        crate::groups::join,
        crate::profile::get_handle,
        crate::profile::claim_handle,
        crate::discord_routes::status,
        crate::discord_routes::connect,
        crate::discord_routes::disconnect,
        crate::discord_routes::update_consent,
        saved_views::list,
        saved_views::create,
        saved_views::delete,
        task_delete_routes::delete_task_with_undo,
        task_delete_routes::list_task_delete_undos,
        task_delete_routes::undo_task_delete,
        calendar_move_routes::list_calendar_move_undos,
        calendar_move_routes::move_event,
        calendar_move_routes::move_task,
        calendar_move_routes::undo_calendar_move,
        operations::capability,
        operations::snapshot,
        routes::health,
        routes::ready,
        routes::metrics,
        routes::development_session,
        routes::current_session,
        routes::logout,
        routes::get_settings,
        routes::update_settings,
        routes::record_reminder_delivery,
        routes::google_auth_start,
        routes::google_calendar_connect_start,
        routes::google_tasks_connect_start,
        routes::google_auth_callback,
        routes::get_daily_plan,
        routes::update_daily_focus,
        routes::start_daily_review,
        routes::complete_daily_review,
        routes::list_labels,
        routes::create_project,
        routes::list_projects,
        routes::update_project,
        routes::delete_project,
        routes::create_task,
        routes::list_tasks,
        routes::reorder_tasks,
        routes::update_task,
        routes::delete_task,
        calendar_routes::list_calendars,
        calendar_routes::create_calendar,
        calendar_routes::update_calendar,
        calendar_routes::delete_calendar,
        calendar_routes::list_events,
        calendar_routes::create_event,
        calendar_routes::update_event,
        calendar_routes::delete_event,
        note_routes::list_notes,
        note_routes::create_note,
        note_routes::update_note,
        note_routes::delete_note,
        note_routes::global_search,
        export_routes::export_json,
        export_routes::export_tasks_csv,
        export_routes::export_notes_markdown,
        export_routes::export_calendars_ics,
        export_routes::delete_account,
        import_routes::import_todoist,
        file_routes::file_usage,
        file_routes::list_files,
        file_routes::upload_file,
        file_routes::download_file,
        file_routes::delete_file,
        google_tasks_routes::status,
        google_tasks_routes::lists,
        google_tasks_routes::configure,
        google_tasks_routes::create_list,
        google_tasks_routes::synchronize,
        google_tasks_routes::conflicts,
        google_tasks_routes::resolve_conflict,
        google_tasks_routes::recoveries,
        google_tasks_routes::leave_unlinked,
        sync_routes::google_status,
        sync_routes::discover_google_calendars,
        sync_routes::revoke_google,
        sync_routes::create_synchronization,
        sync_routes::get_synchronization,
        sync_routes::list_conflicts,
        sync_routes::resolve_conflict,
        sync_routes::activity
    ),
    components(schemas(
        crate::group_assignments::GroupAssignmentFilter,
        crate::group_assignments::AssignmentProgress,
        crate::group_assignments::GroupAssignment,
        crate::group_assignments::GroupAssignmentPage,
        crate::group_assignments::CreateGroupAssignment,
        crate::group_assignments::UpdateGroupAssignment,
        crate::group_assignments::GroupAssignmentVersion,
        crate::group_assignments::UpdateAssignmentProgress,
        crate::group_assignments::GroupProgressPreferences,
        crate::group_assignments::UpdateGroupProgressPreferences,
        crate::group_assignments::SharedAssignmentProgress,
        crate::group_assignments::AssignmentProgressReport,
        crate::group_assignments::GroupAssignmentUndo,
        crate::group_assignments::GroupAssignmentUndoPage,
        crate::group_assignments::GroupContentUsage,
        crate::groups::GroupCapability,
        crate::groups::GroupSummary,
        crate::groups::GroupMember,
        crate::groups::GroupInvitation,
        crate::groups::GroupJoinRequest,
        crate::groups::MyGroupJoinRequest,
        crate::groups::GroupInbox,
        crate::groups::OwnershipOffer,
        crate::groups::GroupDetail,
        crate::groups::CreateGroup,
        crate::groups::GroupMutation,
        crate::groups::GroupAction,
        crate::groups::GroupMutationResult,
        crate::groups::IssueGroupInvitation,
        crate::groups::IssuedGroupInvitation,
        crate::groups::RevokeGroupInvitation,
        crate::groups::GroupInviteReference,
        crate::groups::JoinGroup,
        crate::groups::GroupInvitePreview,
        crate::groups::GroupJoinResult,
        crate::profile::HandleProfile,
        crate::profile::ClaimHandleRequest,
        crate::discord_routes::DiscordStatus,
        crate::discord_routes::DiscordAuthorization,
        crate::discord_routes::DiscordConnectRequest,
        crate::discord_routes::DiscordDisconnectRequest,
        crate::discord_routes::DiscordConsentRequest,
        crate::discord_store::DiscordConnection,
        crate::google_tasks_store::GoogleTasksStatus,
        crate::google_tasks_store::GoogleTaskRecovery,
        crate::google_tasks::GoogleTaskConflict,
        crate::google_tasks::TaskConflictChoice,
        crate::google_tasks::TaskFields,
        crate::google_tasks_routes::GoogleTaskResolutionRequest,
        crate::google_tasks_client::GoogleTaskList,
        crate::google_tasks_routes::GoogleTasksSettingsRequest,
        crate::google_tasks_routes::GoogleTasksCreateListRequest,
        crate::models::DeleteTaskWithUndoRequest,
        crate::models::TaskDeleteUndo,
        crate::models::TaskDeleteUndoList,
        crate::models::MoveCalendarItemRequest,
        crate::models::CalendarMoveUndo,
        crate::models::CalendarMoveUndoList,
        operations::OperationsSnapshot,
        operations::OperationsMetrics,
        operations::OperationsLimits,
        operations::ProbeStatus,
        ErrorBody,
        ErrorResponse,
        HealthResponse,
        UserProfile,
        SessionResponse,
        ThemePreference,
        SyncConflictPolicy,
        UserSettings,
        UpdateUserSettingsRequest,
        DailyPlan,
        UpdateDailyFocusRequest,
        DailyReviewStatus,
        DailyReview,
        DailyReviewResponse,
        ReviewDecisionAction,
        ReviewTaskDecision,
        StartDailyReviewRequest,
        CompleteDailyReviewRequest,
        LabelList,
        CalendarSource,
        EventRecurrence,
        Calendar,
        CalendarList,
        CreateCalendarRequest,
        UpdateCalendarRequest,
        CalendarEvent,
        CalendarEventList,
        CreateCalendarEventRequest,
        UpdateCalendarEventRequest,
        Note,
        NoteList,
        CreateNoteRequest,
        UpdateNoteRequest,
        SearchResultKind,
        SearchResult,
        SearchResultList,
        DeleteAccountRequest,
        TodoistImportTask,
        TodoistImportRequest,
        TodoistImportResult,
        FileRecord,
        FileList,
        FileUsage,
        GoogleIntegrationStatus,
        Synchronization,
        CreateSynchronizationRequest,
        SyncConflict,
        SyncConflictList,
        ResolveSyncConflictRequest,
        ActivityEntry,
        ActivityList,
        ProjectStatus,
        Project,
        ProjectPage,
        CreateProjectRequest,
        UpdateProjectRequest,
        TaskStatus,
        TaskPriority,
        TaskRecurrence,
        Task,
        TaskPage,
        CreateTaskRequest,
        UpdateTaskRequest,
        ReorderTasksRequest
    )),
    modifiers(&DevelopmentSecurity),
    tags(
        (name = "system", description = "Health and readiness"),
        (name = "development", description = "Development-only session bootstrap"),
        (name = "settings", description = "User preferences"),
        (name = "discord", description = "Optional private Discord task commands"),
        (name = "projects", description = "Outcome-oriented projects"),
        (name = "tasks", description = "Project tasks"),
        (name = "calendars", description = "Native and connected calendars"),
        (name = "events", description = "Calendar events"),
        (name = "notes", description = "Markdown notes"),
        (name = "search", description = "Tenant-scoped global search"),
        (name = "exports", description = "Portable data exports"),
        (name = "imports", description = "Imports from other productivity tools"),
        (name = "account", description = "Account lifecycle"),
        (name = "groups", description = "Membership-authorized shared groups; staged server rollout"),
        (name = "files", description = "Authorized file attachments"),
        (name = "synchronization", description = "Google Calendar synchronization and activity")
    )
)]
pub struct ApiDoc;

struct DevelopmentSecurity;

impl Modify for DevelopmentSecurity {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "development_user",
                SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                    DEVELOPMENT_USER_HEADER,
                    "Development-only user identifier.",
                ))),
            );
            components.add_security_scheme(
                "session_cookie",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                    SESSION_COOKIE,
                    "Secure HTTP-only user session cookie.",
                ))),
            );
        }
    }
}

pub fn build(config: &Config, store: Store) -> anyhow::Result<Router> {
    let origins = config
        .cors_allowed_origin
        .split(',')
        .map(|origin| {
            let url = reqwest::Url::parse(origin.trim())?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some_and(|host| !host.contains('*'))
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "CORS_ALLOWED_ORIGIN must contain exact HTTP(S) origins, not paths or wildcards"
            );
            Ok::<HeaderValue, anyhow::Error>(url.origin().ascii_serialization().parse()?)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let request_id_header = HeaderName::from_static("x-request-id");
    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            header::CONTENT_TYPE,
            HeaderName::from_static(DEVELOPMENT_USER_HEADER),
            HeaderName::from_static(CSRF_HEADER),
        ])
        .allow_credentials(true);
    let google_oauth = config
        .google_oauth
        .clone()
        .map(GoogleOAuth::new)
        .transpose()?;
    let file_storage = FileStorage::new(&config.object_storage)?;
    let sync_service = google_oauth
        .clone()
        .map(|google| {
            SyncService::new(
                store.clone(),
                google,
                config.google_calendar_webhook_url.clone(),
            )
        })
        .transpose()?;
    let sync_dispatcher = sync_service
        .clone()
        .map(|service| SyncDispatcher::start(service, file_storage.clone()))
        .unwrap_or_else(|| SyncDispatcher::start_maintenance(store.clone(), file_storage.clone()));
    let state = AppState {
        store,
        allow_insecure_dev_auth: config.allow_insecure_dev_auth,
        invite_only: config.invite_only,
        groups_enabled: config.groups_enabled,
        trust_proxy_headers: config.trust_proxy_headers,
        login_rate_limiter: LoginRateLimiter::default()
            .with_proxy_header(config.trusted_proxy_ip_header),
        action_rate_limiter: LoginRateLimiter::with_limit(60)
            .with_proxy_header(config.trusted_proxy_ip_header),
        request_limits: Default::default(),
        secure_cookies: config.environment == Environment::Production,
        app_url: config.app_url.clone(),
        google_oauth,
        discord: config
            .discord
            .clone()
            .map(crate::discord::Discord::new)
            .transpose()?,
        file_storage,
        max_file_size_bytes: config.max_file_size_bytes,
        max_user_file_storage_bytes: config.max_user_file_storage_bytes,
        max_total_file_storage_bytes: config.max_total_file_storage_bytes,
        max_user_accounts: config.max_user_accounts,
        admin_user_ids: config.admin_user_ids.clone(),
        worker_trigger_token: config.worker_trigger_token.clone(),
        sync_service,
        sync_dispatcher,
        metrics: observability::initialize_metrics(),
    };

    let api = Router::new()
        .route(
            "/groups/{group_id}/assignments",
            get(crate::group_assignments::list).post(crate::group_assignments::create),
        )
        .route(
            "/groups/{group_id}/assignments/{assignment_id}",
            get(crate::group_assignments::get).put(crate::group_assignments::update),
        )
        .route(
            "/groups/{group_id}/assignments/{assignment_id}/progress",
            post(crate::group_assignments::complete),
        )
        .route(
            "/groups/{group_id}/assignments/{assignment_id}/progress-report",
            get(crate::group_assignments::report),
        )
        .route(
            "/groups/{group_id}/assignments/{assignment_id}/delete-with-undo",
            post(crate::group_assignments::delete),
        )
        .route(
            "/groups/{group_id}/assignments/{assignment_id}/restore",
            post(crate::group_assignments::restore),
        )
        .route(
            "/groups/{group_id}/assignment-undos",
            get(crate::group_assignments::undos),
        )
        .route(
            "/groups/{group_id}/preferences",
            get(crate::group_assignments::preferences).post(crate::group_assignments::share),
        )
        .route(
            "/groups/{group_id}/content-usage",
            get(crate::group_assignments::usage),
        )
        .route("/groups/capability", get(crate::groups::capability))
        .route(
            "/groups",
            get(crate::groups::list).post(crate::groups::create),
        )
        .route("/groups/{group_id}", get(crate::groups::detail))
        .route("/groups/{group_id}/actions", post(crate::groups::mutate))
        .route(
            "/groups/{group_id}/invitations",
            post(crate::groups::invite),
        )
        .route(
            "/groups/{group_id}/invitations/{invitation_id}/revoke",
            post(crate::groups::revoke),
        )
        .route("/groups/invitations/inbox", get(crate::groups::inbox))
        .route(
            "/groups/invitations/{invitation_id}/decline",
            post(crate::groups::decline),
        )
        .route("/groups/join/preview", post(crate::groups::preview))
        .route("/groups/join", post(crate::groups::join))
        .route("/integrations/discord", get(crate::discord_routes::status))
        .route(
            "/integrations/discord/connect",
            post(crate::discord_routes::connect),
        )
        .route(
            "/integrations/discord/disconnect",
            post(crate::discord_routes::disconnect),
        )
        .route(
            "/integrations/discord/consent",
            post(crate::discord_routes::update_consent),
        )
        .route(
            "/auth/discord/callback",
            get(crate::discord_routes::callback),
        )
        .route("/operations/capability", get(operations::capability))
        .route("/operations", get(operations::snapshot))
        .route("/development/session", post(routes::development_session))
        .route("/session", get(routes::current_session))
        .route(
            "/profile/handle",
            get(crate::profile::get_handle).post(crate::profile::claim_handle),
        )
        .route("/session/logout", post(routes::logout))
        .route(
            "/settings",
            get(routes::get_settings).put(routes::update_settings),
        )
        .route(
            "/telemetry/reminder-delivered",
            post(routes::record_reminder_delivery),
        )
        .route("/auth/google/start", get(routes::google_auth_start))
        .route(
            "/auth/google/calendar/start",
            get(routes::google_calendar_connect_start),
        )
        .route(
            "/auth/google/tasks/start",
            get(routes::google_tasks_connect_start),
        )
        .route("/auth/google/callback", get(routes::google_auth_callback))
        .route("/daily-plans/{date}", get(routes::get_daily_plan))
        .route("/daily-plans/{date}/focus", put(routes::update_daily_focus))
        .route(
            "/daily-reviews/{date}/start",
            post(routes::start_daily_review),
        )
        .route(
            "/daily-reviews/{date}/complete",
            post(routes::complete_daily_review),
        )
        .route("/labels", get(routes::list_labels))
        .route(
            "/calendars",
            get(calendar_routes::list_calendars).post(calendar_routes::create_calendar),
        )
        .route(
            "/calendars/{calendar_id}",
            put(calendar_routes::update_calendar).delete(calendar_routes::delete_calendar),
        )
        .route(
            "/events",
            get(calendar_routes::list_events).post(calendar_routes::create_event),
        )
        .route(
            "/events/{event_id}",
            put(calendar_routes::update_event).delete(calendar_routes::delete_event),
        )
        .route(
            "/notes",
            get(note_routes::list_notes).post(note_routes::create_note),
        )
        .route(
            "/notes/{note_id}",
            put(note_routes::update_note).delete(note_routes::delete_note),
        )
        .route("/search", get(note_routes::global_search))
        .route(
            "/saved-task-views",
            get(saved_views::list).post(saved_views::create),
        )
        .route(
            "/saved-task-views/{view_id}",
            axum::routing::delete(saved_views::delete),
        )
        .route("/exports/json", get(export_routes::export_json))
        .route("/exports/tasks.csv", get(export_routes::export_tasks_csv))
        .route(
            "/imports/todoist",
            post(import_routes::import_todoist).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route(
            "/exports/notes.md",
            get(export_routes::export_notes_markdown),
        )
        .route(
            "/exports/calendars.ics",
            get(export_routes::export_calendars_ics),
        )
        .route(
            "/integrations/google",
            get(sync_routes::google_status).delete(sync_routes::revoke_google),
        )
        .route(
            "/integrations/google/tasks",
            get(google_tasks_routes::status).put(google_tasks_routes::configure),
        )
        .route(
            "/integrations/google/tasks/lists",
            get(google_tasks_routes::lists).post(google_tasks_routes::create_list),
        )
        .route(
            "/integrations/google/tasks/recoveries",
            get(google_tasks_routes::recoveries),
        )
        .route(
            "/integrations/google/tasks/recoveries/{link_id}/detach",
            post(google_tasks_routes::leave_unlinked),
        )
        .route(
            "/integrations/google/tasks/conflicts",
            get(google_tasks_routes::conflicts),
        )
        .route(
            "/integrations/google/tasks/conflicts/{link_id}",
            post(google_tasks_routes::resolve_conflict),
        )
        .route(
            "/integrations/google/tasks/sync",
            post(google_tasks_routes::synchronize),
        )
        .route(
            "/integrations/google/calendars/discover",
            post(sync_routes::discover_google_calendars),
        )
        .route(
            "/synchronizations",
            post(sync_routes::create_synchronization),
        )
        .route(
            "/synchronizations/{synchronization_id}",
            get(sync_routes::get_synchronization),
        )
        .route("/sync-conflicts", get(sync_routes::list_conflicts))
        .route(
            "/sync-conflicts/{conflict_id}/resolve",
            post(sync_routes::resolve_conflict),
        )
        .route("/activity", get(sync_routes::activity))
        .route(
            "/account",
            axum::routing::delete(export_routes::delete_account),
        )
        .route(
            "/files",
            get(file_routes::list_files)
                .post(file_routes::upload_file)
                .layer(DefaultBodyLimit::max(
                    config.max_file_size_bytes.saturating_add(16 * 1024),
                )),
        )
        .route("/files/usage", get(file_routes::file_usage))
        .route("/files/{file_id}/download", get(file_routes::download_file))
        .route(
            "/files/{file_id}",
            axum::routing::delete(file_routes::delete_file),
        )
        .route(
            "/projects",
            get(routes::list_projects).post(routes::create_project),
        )
        .route(
            "/projects/{project_id}",
            put(routes::update_project).delete(routes::delete_project),
        )
        .route("/tasks", get(routes::list_tasks).post(routes::create_task))
        .route("/tasks/order", put(routes::reorder_tasks))
        .route(
            "/tasks/{task_id}/delete-with-undo",
            post(task_delete_routes::delete_task_with_undo),
        )
        .route(
            "/task-delete-undos",
            get(task_delete_routes::list_task_delete_undos),
        )
        .route(
            "/task-delete-undos/{receipt_id}/consume",
            post(task_delete_routes::undo_task_delete),
        )
        .route(
            "/calendar-move-undos",
            get(calendar_move_routes::list_calendar_move_undos),
        )
        .route(
            "/calendar-move-undos/{receipt_id}/consume",
            post(calendar_move_routes::undo_calendar_move),
        )
        .route(
            "/events/{event_id}/move",
            post(calendar_move_routes::move_event),
        )
        .route(
            "/tasks/{task_id}/move",
            post(calendar_move_routes::move_task),
        )
        .route(
            "/tasks/{task_id}",
            put(routes::update_task).delete(routes::delete_task),
        );

    Ok(Router::new()
        .route(
            "/webhooks/discord",
            post(crate::discord_routes::interactions),
        )
        .route("/health", get(routes::health))
        .route("/ready", get(routes::ready))
        .route("/metrics", get(routes::metrics))
        .route(
            "/internal/synchronization/run",
            post(routes::run_synchronization_worker),
        )
        .route(
            "/webhooks/google/calendar",
            post(sync_routes::google_calendar_webhook),
        )
        .nest(
            "/api/v1",
            api.layer(middleware::from_fn_with_state(
                state.clone(),
                crate::request_limits::limit_requests,
            )),
        )
        .merge(SwaggerUi::new("/docs").url("/api-doc/openapi.json", ApiDoc::openapi()))
        .fallback(routes::route_not_found)
        .method_not_allowed_fallback(routes::method_not_allowed)
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(observability::track_http_metrics))
        .layer(cors)
        .layer(middleware::from_fn_with_state(state, security_headers)))
}

async fn security_headers(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if state.secure_cookies {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'; object-src 'none'; base-uri 'none'"),
    );
    response
}
