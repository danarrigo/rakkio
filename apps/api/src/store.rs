use std::collections::HashSet;

use chrono::{Months, TimeDelta, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    google_auth::{GoogleLoginResult, GoogleLoginStart},
    models::{
        CreateProjectRequest, CreateTaskRequest, DailyPlan, LabelList, Project, ProjectPage,
        ReorderTasksRequest, SessionResponse, Task, TaskPage, TaskRecurrence, TaskStatus,
        TodoistImportRequest, TodoistImportResult, UpdateDailyFocusRequest, UpdateProjectRequest,
        UpdateTaskRequest, UpdateUserSettingsRequest, UserProfile, UserSettings,
    },
};

pub const DEVELOPMENT_USER_ID: Uuid = Uuid::from_u128(0x00000000_0000_4000_8000_000000000001);

#[derive(Clone)]
pub struct Store {
    pub(crate) pool: PgPool,
}

impl Store {
    pub async fn connect(config: &Config) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(config.database_max_connections)
            .connect(&config.database_url)
            .await?;

        sqlx::migrate!("../../migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn ready(&self) -> AppResult<()> {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn ensure_development_user(&self, user_id: Uuid) -> AppResult<UserProfile> {
        let user = sqlx::query_as::<_, UserProfile>(
            r#"
            INSERT INTO users (id, email, display_name)
            VALUES ($1, $1::TEXT || '@development.invalid', '')
            ON CONFLICT (id) DO UPDATE
            SET updated_at = NOW()
            RETURNING id, email, display_name, avatar_url, timezone
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(user)
    }

    pub async fn create_session(&self, user: UserProfile) -> AppResult<CreatedSession> {
        let token = random_token();
        let csrf_token = random_token();
        let expires_at = Utc::now() + TimeDelta::days(30);

        sqlx::query(
            r#"
            INSERT INTO sessions (token_hash, user_id, csrf_token, expires_at)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(token_hash(&token))
        .bind(user.id)
        .bind(&csrf_token)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;

        Ok(CreatedSession {
            token,
            response: SessionResponse { user, csrf_token },
        })
    }

    pub async fn authenticate_session(&self, token: &str) -> AppResult<SessionAuthentication> {
        let session = sqlx::query_as::<_, SessionAuthentication>(
            r#"
            UPDATE sessions
            SET last_seen_at = NOW()
            WHERE token_hash = $1 AND expires_at > NOW()
            RETURNING user_id, csrf_token
            "#,
        )
        .bind(token_hash(token))
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AppError::Unauthorized)?;

        Ok(session)
    }

    pub async fn user_profile(&self, user_id: Uuid) -> AppResult<UserProfile> {
        sqlx::query_as::<_, UserProfile>(
            "SELECT id, email, display_name, avatar_url, timezone FROM users WHERE id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AppError::Unauthorized)
    }

    pub async fn user_settings(&self, user_id: Uuid) -> AppResult<UserSettings> {
        sqlx::query_as::<_, UserSettings>(
            r#"
            INSERT INTO user_settings (user_id)
            VALUES ($1)
            ON CONFLICT (user_id) DO UPDATE SET user_id = EXCLUDED.user_id
            RETURNING theme, automatic_daily_review, sync_conflict_policy, sidebar_visible, updated_at, version
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn update_user_settings(
        &self,
        user_id: Uuid,
        request: UpdateUserSettingsRequest,
    ) -> AppResult<UserSettings> {
        self.user_settings(user_id).await?;
        let settings = sqlx::query_as::<_, UserSettings>(
            r#"
            UPDATE user_settings
            SET
                theme = $2,
                automatic_daily_review = $3,
                sync_conflict_policy = $4,
                sidebar_visible = $5,
                updated_at = NOW(),
                version = version + 1
            WHERE user_id = $1 AND version = $6
            RETURNING theme, automatic_daily_review, sync_conflict_policy, sidebar_visible, updated_at, version
            "#,
        )
        .bind(user_id)
        .bind(request.theme)
        .bind(request.automatic_daily_review)
        .bind(request.sync_conflict_policy)
        .bind(request.sidebar_visible)
        .bind(request.expected_version)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(settings) = settings {
            return Ok(settings);
        }
        let version =
            sqlx::query_scalar::<_, i32>("SELECT version FROM user_settings WHERE user_id = $1")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        match version {
            Some(version) => Err(AppError::Conflict(format!(
                "settings changed since version {}; current version is {version}",
                request.expected_version
            ))),
            None => Err(AppError::NotFound("settings")),
        }
    }

    pub async fn email_can_sign_in(&self, email: &str) -> AppResult<bool> {
        sqlx::query_scalar(
            r#"
            SELECT
                EXISTS(SELECT 1 FROM users WHERE LOWER(email::TEXT) = LOWER($1))
                OR EXISTS(
                    SELECT 1 FROM account_invites
                    WHERE LOWER(email::TEXT) = LOWER($1) AND used_at IS NULL
                )
            "#,
        )
        .bind(email)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn consume_account_invite(&self, email: &str, user_id: Uuid) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE account_invites
            SET used_at = NOW(), used_by = $2
            WHERE LOWER(email::TEXT) = LOWER($1) AND used_at IS NULL
            "#,
        )
        .bind(email)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_session(&self, token: &str) -> AppResult<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash(token))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_google_login_attempt(
        &self,
        login: &GoogleLoginStart,
        user_id: Option<Uuid>,
        purpose: &str,
        legal_versions: Option<(&str, &str)>,
        browser_secret: &str,
    ) -> AppResult<()> {
        let (terms_version, privacy_version, age_confirmed) = match (purpose, legal_versions) {
            ("login", Some((terms_version, privacy_version))) => {
                (Some(terms_version), Some(privacy_version), true)
            }
            ("calendar_connect" | "tasks_connect", None) => (None, None, false),
            _ => return Err(AppError::Validation("invalid OAuth purpose".to_owned())),
        };
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('google-oauth-pending-capacity',0))",
        )
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM oauth_login_attempts WHERE expires_at <= clock_timestamp()")
            .execute(&mut *transaction)
            .await?;
        let (total,browser):(i64,i64)=sqlx::query_as("SELECT COUNT(*),COUNT(*) FILTER(WHERE browser_binding_hash=$1) FROM oauth_login_attempts").bind(token_hash(browser_secret)).fetch_one(&mut *transaction).await?;
        if total >= 5000 || browser >= 5 {
            return Err(AppError::InvalidRequest {
                status: axum::http::StatusCode::TOO_MANY_REQUESTS,
                message: "too many pending sign-in requests; try again shortly".into(),
            });
        }
        sqlx::query(
            r#"
            INSERT INTO oauth_login_attempts (
                state_hash, nonce, pkce_verifier, expires_at, user_id, purpose,
                terms_version, privacy_version, age_confirmed, browser_binding_hash
            )
            VALUES ($1, $2, $3, clock_timestamp() + INTERVAL '10 minutes', $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(token_hash(&login.state))
        .bind(&login.nonce)
        .bind(&login.pkce_verifier)
        .bind(user_id)
        .bind(purpose)
        .bind(terms_version)
        .bind(privacy_version)
        .bind(age_confirmed)
        .bind(token_hash(browser_secret))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn consume_google_login_attempt(
        &self,
        state: &str,
        browser_secret: &str,
    ) -> AppResult<OAuthLoginAttempt> {
        sqlx::query_as::<_, OAuthLoginAttempt>(
            r#"
            DELETE FROM oauth_login_attempts
            WHERE state_hash = $1 AND browser_binding_hash = $2 AND expires_at > clock_timestamp()
            RETURNING nonce, pkce_verifier, user_id, purpose,
                      terms_version, privacy_version, age_confirmed
            "#,
        )
        .bind(token_hash(state))
        .bind(token_hash(browser_secret))
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AppError::Forbidden(
            "the Google sign-in request expired or is invalid",
        ))
    }

    pub async fn record_legal_acceptance(
        &self,
        user_id: Uuid,
        terms_version: &str,
        privacy_version: &str,
        age_confirmed: bool,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO legal_acceptances (
                id, user_id, terms_version, privacy_version, age_confirmed
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (user_id, terms_version, privacy_version) DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(terms_version)
        .bind(privacy_version)
        .bind(age_confirmed)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn daily_plan(&self, user_id: Uuid, date: chrono::NaiveDate) -> AppResult<DailyPlan> {
        let focus_tasks = sqlx::query_as::<_, Task>(
            r#"
            SELECT
                tasks.id,
                tasks.project_id,
                tasks.parent_task_id,
                tasks.title,
                tasks.description,
                tasks.due_at,
                tasks.scheduled_start,
                tasks.scheduled_end,
                tasks.status,
                tasks.priority,
                tasks.recurrence,
                tasks.labels,
                tasks.remind_at,
                tasks.position,
                tasks.completed_at,
                tasks.created_at,
                tasks.updated_at,
                tasks.version
            FROM daily_focus_tasks focus
            JOIN tasks ON tasks.id = focus.task_id AND tasks.user_id = focus.user_id
            WHERE focus.user_id = $1 AND focus.focus_date = $2
            ORDER BY focus.position
            "#,
        )
        .bind(user_id)
        .bind(date)
        .fetch_all(&self.pool)
        .await?;

        Ok(DailyPlan { date, focus_tasks })
    }

    pub async fn update_daily_focus(
        &self,
        user_id: Uuid,
        date: chrono::NaiveDate,
        request: UpdateDailyFocusRequest,
    ) -> AppResult<DailyPlan> {
        if request.task_ids.len() > 3 {
            return Err(AppError::Validation(
                "a daily plan cannot contain more than three focus tasks".to_owned(),
            ));
        }
        let unique: HashSet<_> = request.task_ids.iter().copied().collect();
        if unique.len() != request.task_ids.len() {
            return Err(AppError::Validation(
                "focus task identifiers must be unique".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(AppError::Unauthorized)?;
        let owned = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM tasks
            WHERE user_id = $1 AND id = ANY($2) AND status <> 'completed'
            "#,
        )
        .bind(user_id)
        .bind(&request.task_ids)
        .fetch_one(&mut *transaction)
        .await?;
        if owned != request.task_ids.len() as i64 {
            return Err(AppError::Validation(
                "focus tasks must reference the user's incomplete tasks".to_owned(),
            ));
        }

        Self::bump_focus_revision(&mut transaction, user_id, date).await?;
        sqlx::query("DELETE FROM daily_focus_tasks WHERE user_id = $1 AND focus_date = $2")
            .bind(user_id)
            .bind(date)
            .execute(&mut *transaction)
            .await?;
        for (index, task_id) in request.task_ids.iter().enumerate() {
            sqlx::query(
                r#"
                INSERT INTO daily_focus_tasks (user_id, focus_date, task_id, position)
                VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(user_id)
            .bind(date)
            .bind(task_id)
            .bind(index as i16 + 1)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;

        self.daily_plan(user_id, date).await
    }

    pub async fn list_labels(&self, user_id: Uuid) -> AppResult<LabelList> {
        let items = sqlx::query_scalar::<_, String>(
            "SELECT name FROM labels WHERE user_id = $1 ORDER BY name",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(LabelList { items })
    }

    pub async fn upsert_google_user(
        &self,
        login: GoogleLoginResult,
        max_user_accounts: Option<i64>,
    ) -> AppResult<UserProfile> {
        let mut transaction = self.pool.begin().await?;
        if let Some(max_user_accounts) = max_user_accounts {
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(741_733_075_i64)
                .execute(&mut *transaction)
                .await?;
            let already_registered = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM users WHERE google_subject = $1)",
            )
            .bind(&login.subject)
            .fetch_one(&mut *transaction)
            .await?;
            if !already_registered {
                let user_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
                    .fetch_one(&mut *transaction)
                    .await?;
                if user_count >= max_user_accounts {
                    return Err(AppError::Forbidden(
                        "new registrations are temporarily paused because service capacity has been reached",
                    ));
                }
            }
        }
        let user = sqlx::query_as::<_, UserProfile>(
            r#"
            INSERT INTO users (
                id, google_subject, email, display_name, avatar_url
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (google_subject) DO UPDATE
            SET
                email = EXCLUDED.email,
                display_name = EXCLUDED.display_name,
                avatar_url = EXCLUDED.avatar_url,
                updated_at = NOW()
            RETURNING id, email, display_name, avatar_url, timezone
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(&login.subject)
        .bind(&login.email)
        .bind(&login.display_name)
        .bind(&login.avatar_url)
        .fetch_one(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO google_accounts (
                user_id,
                encrypted_access_token,
                encrypted_refresh_token,
                access_token_expires_at,
                scopes
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (user_id) DO UPDATE
            SET
                encrypted_access_token = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM unnest(EXCLUDED.scopes) scope
                        WHERE scope LIKE '%/auth/calendar%'
                    ) THEN EXCLUDED.encrypted_access_token
                    ELSE google_accounts.encrypted_access_token
                END,
                encrypted_refresh_token = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM unnest(EXCLUDED.scopes) scope
                        WHERE scope LIKE '%/auth/calendar%'
                    ) THEN COALESCE(
                        EXCLUDED.encrypted_refresh_token,
                        google_accounts.encrypted_refresh_token
                    )
                    ELSE google_accounts.encrypted_refresh_token
                END,
                access_token_expires_at = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM unnest(EXCLUDED.scopes) scope
                        WHERE scope LIKE '%/auth/calendar%'
                    ) THEN EXCLUDED.access_token_expires_at
                    ELSE google_accounts.access_token_expires_at
                END,
                scopes = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM unnest(EXCLUDED.scopes) scope
                        WHERE scope LIKE '%/auth/calendar%'
                    ) THEN EXCLUDED.scopes
                    ELSE google_accounts.scopes
                END,
                updated_at = NOW()
            "#,
        )
        .bind(user.id)
        .bind(login.encrypted_access_token)
        .bind(login.encrypted_refresh_token)
        .bind(login.access_token_expires_at)
        .bind(login.scopes)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(user)
    }

    pub async fn update_google_credentials(
        &self,
        user_id: Uuid,
        login: GoogleLoginResult,
    ) -> AppResult<()> {
        let matches_user = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM users
                WHERE id = $1 AND google_subject = $2
                  AND LOWER(email::TEXT) = LOWER($3)
            )
            "#,
        )
        .bind(user_id)
        .bind(&login.subject)
        .bind(&login.email)
        .fetch_one(&self.pool)
        .await?;
        if !matches_user {
            return Err(AppError::Forbidden(
                "Google Calendar must be connected with the signed-in account",
            ));
        }
        sqlx::query(
            r#"
            INSERT INTO google_accounts (
                user_id, encrypted_access_token, encrypted_refresh_token,
                access_token_expires_at, scopes
            ) VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (user_id) DO UPDATE SET
                encrypted_access_token = EXCLUDED.encrypted_access_token,
                encrypted_refresh_token = COALESCE(
                    EXCLUDED.encrypted_refresh_token,
                    google_accounts.encrypted_refresh_token
                ),
                access_token_expires_at = EXCLUDED.access_token_expires_at,
                scopes = EXCLUDED.scopes,
                updated_at = NOW()
            "#,
        )
        .bind(user_id)
        .bind(login.encrypted_access_token)
        .bind(login.encrypted_refresh_token)
        .bind(login.access_token_expires_at)
        .bind(login.scopes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn create_project(
        &self,
        user_id: Uuid,
        request: CreateProjectRequest,
    ) -> AppResult<Project> {
        let name = request.name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(AppError::Validation(
                "project name must contain between 1 and 120 characters".to_owned(),
            ));
        }
        if request.outcome.chars().count() > 2_000 {
            return Err(AppError::Validation(
                "project outcome cannot exceed 2000 characters".to_owned(),
            ));
        }

        let project = sqlx::query_as::<_, Project>(
            r#"
            INSERT INTO projects (id, user_id, name, outcome, target_date, status)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING
                id,
                name,
                outcome,
                target_date,
                status,
                0::BIGINT AS total_tasks,
                0::BIGINT AS completed_tasks,
                created_at,
                updated_at,
                version
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(name)
        .bind(request.outcome.trim())
        .bind(request.target_date)
        .bind(request.status)
        .fetch_one(&self.pool)
        .await?;

        Ok(project)
    }

    pub async fn update_project(
        &self,
        user_id: Uuid,
        project_id: Uuid,
        request: UpdateProjectRequest,
    ) -> AppResult<Project> {
        validate_project_fields(&request.name, &request.outcome)?;
        if request.expected_version < 1 {
            return Err(AppError::Validation(
                "expected_version must be greater than zero".to_owned(),
            ));
        }

        let project = sqlx::query_as::<_, Project>(
            r#"
            WITH updated AS (
                UPDATE projects
                SET
                    name = $3,
                    outcome = $4,
                    target_date = $5,
                    status = $6,
                    updated_at = NOW(),
                    version = version + 1
                WHERE id = $1 AND user_id = $2 AND version = $7
                RETURNING *
            )
            SELECT
                updated.id,
                updated.name,
                updated.outcome,
                updated.target_date,
                updated.status,
                COUNT(tasks.id)::BIGINT AS total_tasks,
                COUNT(tasks.id) FILTER (WHERE tasks.status = 'completed')::BIGINT AS completed_tasks,
                updated.created_at,
                updated.updated_at,
                updated.version
            FROM updated
            LEFT JOIN tasks ON tasks.project_id = updated.id AND tasks.user_id = $2
            GROUP BY
                updated.id,
                updated.name,
                updated.outcome,
                updated.target_date,
                updated.status,
                updated.created_at,
                updated.updated_at,
                updated.version
            "#,
        )
        .bind(project_id)
        .bind(user_id)
        .bind(request.name.trim())
        .bind(request.outcome.trim())
        .bind(request.target_date)
        .bind(request.status)
        .bind(request.expected_version)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(project) = project {
            return Ok(project);
        }

        let current_version = sqlx::query_scalar::<_, i32>(
            "SELECT version FROM projects WHERE id = $1 AND user_id = $2",
        )
        .bind(project_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        match current_version {
            Some(version) => Err(AppError::Conflict(format!(
                "project changed since version {}; current version is {version}",
                request.expected_version
            ))),
            None => Err(AppError::NotFound("project")),
        }
    }

    pub async fn list_projects(
        &self,
        user_id: Uuid,
        cursor: Option<Uuid>,
        limit: i64,
    ) -> AppResult<ProjectPage> {
        let mut projects = sqlx::query_as::<_, Project>(
            r#"
            SELECT
                p.id,
                p.name,
                p.outcome,
                p.target_date,
                p.status,
                COUNT(t.id)::BIGINT AS total_tasks,
                COUNT(t.id) FILTER (WHERE t.status = 'completed')::BIGINT AS completed_tasks,
                p.created_at,
                p.updated_at,
                p.version
            FROM projects p
            LEFT JOIN tasks t ON t.project_id = p.id AND t.user_id = p.user_id
            WHERE p.user_id = $1 AND ($2::UUID IS NULL OR p.id < $2)
            GROUP BY p.id
            ORDER BY p.id DESC
            LIMIT $3
            "#,
        )
        .bind(user_id)
        .bind(cursor)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await?;

        let next_cursor = if projects.len() > limit as usize {
            projects.pop();
            projects.last().map(|project| project.id)
        } else {
            None
        };

        Ok(ProjectPage {
            items: projects,
            next_cursor,
        })
    }

    pub async fn delete_project(
        &self,
        user_id: Uuid,
        project_id: Uuid,
        expected_version: i32,
    ) -> AppResult<()> {
        if expected_version < 1 {
            return Err(AppError::Validation(
                "expected_version must be greater than zero".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await?;
        Self::lock_task_graph(&mut transaction, user_id).await?;
        let task_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM tasks WHERE user_id = $1 AND project_id = $2",
        )
        .bind(user_id)
        .bind(project_id)
        .fetch_all(&mut *transaction)
        .await?;
        for task_id in task_ids {
            Self::remove_task_calendar_event(&mut transaction, user_id, task_id).await?;
        }
        let result =
            sqlx::query("DELETE FROM projects WHERE id = $1 AND user_id = $2 AND version = $3")
                .bind(project_id)
                .bind(user_id)
                .bind(expected_version)
                .execute(&mut *transaction)
                .await?;

        if result.rows_affected() == 1 {
            transaction.commit().await?;
            return Ok(());
        }

        let current_version = sqlx::query_scalar::<_, i32>(
            "SELECT version FROM projects WHERE id = $1 AND user_id = $2",
        )
        .bind(project_id)
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?;

        match current_version {
            Some(version) => Err(AppError::Conflict(format!(
                "project changed since version {expected_version}; current version is {version}"
            ))),
            None => Err(AppError::NotFound("project")),
        }
    }

    pub async fn import_todoist_project(
        &self,
        user_id: Uuid,
        request: TodoistImportRequest,
    ) -> AppResult<TodoistImportResult> {
        validate_project_fields(&request.project_name, &request.project_description)?;
        if request.tasks.len() > 5_000 {
            return Err(AppError::Validation(
                "a Todoist import cannot contain more than 5000 tasks".to_owned(),
            ));
        }

        let mut prepared_tasks = Vec::with_capacity(request.tasks.len());
        for (index, mut task) in request.tasks.into_iter().enumerate() {
            if task
                .parent_index
                .is_some_and(|parent_index| parent_index >= index)
            {
                return Err(AppError::Validation(
                    "an imported task parent must appear before its child".to_owned(),
                ));
            }
            validate_task_fields(
                &task.title,
                &task.description,
                task.due_at,
                task.scheduled_start,
                task.scheduled_end,
                task.recurrence,
                task.parent_index.map(|_| Uuid::nil()),
            )?;
            let labels = validate_labels(std::mem::take(&mut task.labels))?;
            prepared_tasks.push((task, labels));
        }

        let mut transaction = self.pool.begin().await?;
        Self::lock_task_graph(&mut transaction, user_id).await?;
        let project_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO projects (id, user_id, name, outcome, status)
            VALUES ($1, $2, $3, $4, 'active')
            "#,
        )
        .bind(project_id)
        .bind(user_id)
        .bind(request.project_name.trim())
        .bind(request.project_description.trim())
        .execute(&mut *transaction)
        .await?;

        let mut position = Self::next_task_position(&mut transaction, user_id).await?;
        let mut task_ids = Vec::with_capacity(prepared_tasks.len());
        for (task, labels) in prepared_tasks {
            let task_id = Uuid::now_v7();
            let parent_task_id = task.parent_index.map(|parent_index| task_ids[parent_index]);
            let imported_task = sqlx::query_as::<_, Task>(
                r#"
                INSERT INTO tasks (
                    id,
                    user_id,
                    project_id,
                    parent_task_id,
                    title,
                    description,
                    due_at,
                    scheduled_start,
                    scheduled_end,
                    status,
                    priority,
                    recurrence,
                    labels,
                    position
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'todo', $10, $11, $12, $13)
                RETURNING
                    id,
                    project_id,
                    parent_task_id,
                    title,
                    description,
                    due_at,
                    scheduled_start,
                    scheduled_end,
                    status,
                    priority,
                    recurrence,
                    labels,
                    remind_at,
                    position,
                    completed_at,
                    created_at,
                    updated_at,
                    version
                "#,
            )
            .bind(task_id)
            .bind(user_id)
            .bind(project_id)
            .bind(parent_task_id)
            .bind(task.title.trim())
            .bind(task.description.trim())
            .bind(task.due_at)
            .bind(task.scheduled_start)
            .bind(task.scheduled_end)
            .bind(task.priority)
            .bind(task.recurrence)
            .bind(labels)
            .bind(position)
            .fetch_one(&mut *transaction)
            .await?;
            Self::ensure_labels(&mut transaction, user_id, &imported_task.labels).await?;
            Self::sync_task_calendar_event(&mut transaction, user_id, &imported_task).await?;
            task_ids.push(task_id);
            position += 1_024;
        }

        let project = sqlx::query_as::<_, Project>(
            r#"
            SELECT
                p.id,
                p.name,
                p.outcome,
                p.target_date,
                p.status,
                COUNT(t.id)::BIGINT AS total_tasks,
                COUNT(t.id) FILTER (WHERE t.status = 'completed')::BIGINT AS completed_tasks,
                p.created_at,
                p.updated_at,
                p.version
            FROM projects p
            LEFT JOIN tasks t ON t.project_id = p.id AND t.user_id = p.user_id
            WHERE p.id = $1 AND p.user_id = $2
            GROUP BY p.id
            "#,
        )
        .bind(project_id)
        .bind(user_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(TodoistImportResult {
            project,
            imported_tasks: task_ids.len(),
        })
    }

    pub async fn create_task(&self, user_id: Uuid, request: CreateTaskRequest) -> AppResult<Task> {
        let mut transaction = self.pool.begin().await?;
        let task = self
            .create_task_in(&mut transaction, user_id, request)
            .await?;
        transaction.commit().await?;
        Ok(task)
    }

    // Integrations must commit their idempotency receipt with the normal task
    // operation, not recreate its validation, recurrence, quota or sync rules.
    pub(crate) async fn create_task_in(
        &self,
        transaction: &mut PgConnection,
        user_id: Uuid,
        request: CreateTaskRequest,
    ) -> AppResult<Task> {
        validate_task_fields(
            &request.title,
            &request.description,
            request.due_at,
            request.scheduled_start,
            request.scheduled_end,
            request.recurrence,
            request.parent_task_id,
        )?;
        let labels = validate_labels(request.labels)?;
        if request.recurrence != TaskRecurrence::None && request.status == TaskStatus::Completed {
            return Err(AppError::Validation(
                "a recurring task cannot be created as completed".to_owned(),
            ));
        }
        Self::lock_task_graph(transaction, user_id).await?;
        self.validate_task_relationships(
            transaction,
            user_id,
            request.project_id,
            request.parent_task_id,
        )
        .await?;

        let position = Self::next_task_position(transaction, user_id).await?;
        let completed_at = (request.status == TaskStatus::Completed).then(Utc::now);
        let remind_at = request.remind_at;
        let task = sqlx::query_as::<_, Task>(
            r#"
            INSERT INTO tasks (
                id,
                user_id,
                project_id,
                parent_task_id,
                title,
                description,
                due_at,
                scheduled_start,
                scheduled_end,
                status,
                priority,
                recurrence,
                labels,
                remind_at,
                position,
                completed_at
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8,
                $9, $10, $11, $12, $13, $14, $15, $16
            )
            RETURNING
                id,
                project_id,
                parent_task_id,
                title,
                description,
                due_at,
                scheduled_start,
                scheduled_end,
                status,
                priority,
                recurrence,
                labels,
                remind_at,
                position,
                completed_at,
                created_at,
                updated_at,
                version
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(request.project_id)
        .bind(request.parent_task_id)
        .bind(request.title.trim())
        .bind(request.description.trim())
        .bind(request.due_at)
        .bind(request.scheduled_start)
        .bind(request.scheduled_end)
        .bind(request.status)
        .bind(request.priority)
        .bind(request.recurrence)
        .bind(labels)
        .bind(remind_at)
        .bind(position)
        .bind(completed_at)
        .fetch_one(&mut *transaction)
        .await?;
        Self::ensure_labels(transaction, user_id, &task.labels).await?;
        Self::sync_task_calendar_event(transaction, user_id, &task).await?;

        Ok(task)
    }

    pub async fn list_tasks(
        &self,
        user_id: Uuid,
        project_id: Option<Uuid>,
        cursor: Option<Uuid>,
        limit: i64,
    ) -> AppResult<TaskPage> {
        let mut tasks = sqlx::query_as::<_, Task>(
            r#"
            SELECT
                id,
                project_id,
                parent_task_id,
                title,
                description,
                due_at,
                scheduled_start,
                scheduled_end,
                status,
                priority,
                recurrence,
                labels,
                remind_at,
                position,
                completed_at,
                created_at,
                updated_at,
                version
            FROM tasks
            WHERE
                user_id = $1
                AND ($2::UUID IS NULL OR project_id = $2)
                AND ($3::UUID IS NULL OR id < $3)
            ORDER BY id DESC
            LIMIT $4
            "#,
        )
        .bind(user_id)
        .bind(project_id)
        .bind(cursor)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await?;

        let next_cursor = if tasks.len() > limit as usize {
            tasks.pop();
            tasks.last().map(|task| task.id)
        } else {
            None
        };

        Ok(TaskPage {
            items: tasks,
            next_cursor,
        })
    }

    pub async fn update_task(
        &self,
        user_id: Uuid,
        task_id: Uuid,
        request: UpdateTaskRequest,
    ) -> AppResult<Task> {
        self.update_task_scoped(user_id, task_id, request, None)
            .await
    }

    pub async fn update_task_scoped(
        &self,
        user_id: Uuid,
        task_id: Uuid,
        request: UpdateTaskRequest,
        scope: Option<crate::task_recurrence::RecurrenceEditScope>,
    ) -> AppResult<Task> {
        let mut transaction = self.pool.begin().await?;
        let result = self
            .update_task_scoped_in(&mut transaction, user_id, task_id, request, scope)
            .await;
        match result {
            Ok(task) => {
                transaction.commit().await?;
                Ok(task)
            }
            Err(error) => {
                transaction.rollback().await?;
                Err(error)
            }
        }
    }

    pub(crate) async fn update_task_scoped_in(
        &self,
        transaction: &mut PgConnection,
        user_id: Uuid,
        task_id: Uuid,
        request: UpdateTaskRequest,
        scope: Option<crate::task_recurrence::RecurrenceEditScope>,
    ) -> AppResult<Task> {
        validate_task_fields(
            &request.title,
            &request.description,
            request.due_at,
            request.scheduled_start,
            request.scheduled_end,
            request.recurrence,
            request.parent_task_id,
        )?;
        let labels = validate_labels(request.labels.clone())?;
        if request.expected_version < 1 {
            return Err(AppError::Validation(
                "expected_version must be greater than zero".to_owned(),
            ));
        }
        if request.parent_task_id == Some(task_id) {
            return Err(AppError::Validation(
                "a task cannot be its own parent".to_owned(),
            ));
        }
        let result: AppResult<Task> = async {
            Self::lock_task_graph(transaction, user_id).await?;
            self.validate_task_relationships(
                transaction,
                user_id,
                request.project_id,
                request.parent_task_id,
            )
            .await?;
            self.validate_task_update_relationships(
                transaction,
                user_id,
                task_id,
                request.project_id,
                request.parent_task_id,
                request.recurrence,
            )
            .await?;

            let mut recurring =
                crate::task_recurrence::RecurringTaskState::load(transaction, user_id, task_id)
                    .await?;
            recurring.prepare(&request, scope)?;
            let previous_status = recurring.task.status;
            if previous_status == TaskStatus::Completed && request.status != TaskStatus::Completed {
                Self::remove_next_occurrence_for_reopen(transaction, user_id, task_id).await?;
            }
            let completed_at = (request.status == TaskStatus::Completed).then(Utc::now);
            let remind_at = request.remind_at;
            let task = sqlx::query_as::<_, Task>(
                r#"
            UPDATE tasks
            SET
                project_id = $3,
                parent_task_id = $4,
                title = $5,
                description = $6,
                due_at = $7,
                scheduled_start = $8,
                scheduled_end = $9,
                status = $10,
                priority = $11,
                recurrence = $12,
                recurrence_defaults = $17,
                recurrence_project_id = $18,
                labels = $13,
                remind_at = $14,
                completed_at = CASE
                    WHEN $10 = 'completed' AND completed_at IS NOT NULL THEN completed_at
                    ELSE $15
                END,
                updated_at = NOW(),
                version = version + 1
            WHERE id = $1 AND user_id = $2 AND version = $16
            RETURNING
                id,
                project_id,
                parent_task_id,
                title,
                description,
                due_at,
                scheduled_start,
                scheduled_end,
                status,
                priority,
                recurrence,
                labels,
                remind_at,
                position,
                completed_at,
                created_at,
                updated_at,
                version
            "#,
            )
            .bind(task_id)
            .bind(user_id)
            .bind(request.project_id)
            .bind(request.parent_task_id)
            .bind(request.title.trim())
            .bind(request.description.trim())
            .bind(request.due_at)
            .bind(request.scheduled_start)
            .bind(request.scheduled_end)
            .bind(request.status)
            .bind(request.priority)
            .bind(request.recurrence)
            .bind(labels)
            .bind(remind_at)
            .bind(completed_at)
            .bind(request.expected_version)
            .bind(recurring.recurrence_defaults)
            .bind(recurring.recurrence_project_id)
            .fetch_optional(&mut *transaction)
            .await?;

            if let Some(task) = task {
                Self::ensure_labels(transaction, user_id, &task.labels).await?;
                Self::sync_task_calendar_event(transaction, user_id, &task).await?;
                if previous_status != TaskStatus::Completed
                    && task.status == TaskStatus::Completed
                    && task.recurrence != TaskRecurrence::None
                {
                    Self::create_next_recurring_task(transaction, user_id, &task).await?;
                }
                return Ok(task);
            }

            let current_version = sqlx::query_scalar::<_, i32>(
                "SELECT version FROM tasks WHERE id = $1 AND user_id = $2",
            )
            .bind(task_id)
            .bind(user_id)
            .fetch_optional(&mut *transaction)
            .await?;

            match current_version {
                Some(version) => Err(AppError::Conflict(format!(
                    "task changed since version {}; current version is {version}",
                    request.expected_version
                ))),
                None => Err(AppError::NotFound("task")),
            }
        }
        .await;
        result
    }

    pub async fn reorder_tasks(
        &self,
        user_id: Uuid,
        request: ReorderTasksRequest,
    ) -> AppResult<()> {
        let unique_ids: HashSet<_> = request.task_ids.iter().copied().collect();
        if unique_ids.len() != request.task_ids.len() {
            return Err(AppError::Validation(
                "task_ids cannot contain duplicates".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await?;
        Self::lock_task_graph(&mut transaction, user_id).await?;
        let task_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks WHERE user_id = $1")
                .bind(user_id)
                .fetch_one(&mut *transaction)
                .await?;
        if task_count != request.task_ids.len() as i64 {
            return Err(AppError::Validation(
                "task_ids must contain every task in the workspace".to_owned(),
            ));
        }

        let owned_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks WHERE user_id = $1 AND id = ANY($2)",
        )
        .bind(user_id)
        .bind(&request.task_ids)
        .fetch_one(&mut *transaction)
        .await?;
        if owned_count != task_count {
            return Err(AppError::Validation(
                "task_ids must reference only the user's tasks".to_owned(),
            ));
        }

        for (index, task_id) in request.task_ids.iter().enumerate() {
            sqlx::query("UPDATE tasks SET position = $3 WHERE id = $1 AND user_id = $2")
                .bind(task_id)
                .bind(user_id)
                .bind((index as i64 + 1) * 1024)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn delete_task(
        &self,
        user_id: Uuid,
        task_id: Uuid,
        expected_version: i32,
    ) -> AppResult<()> {
        if expected_version < 1 {
            return Err(AppError::Validation(
                "expected_version must be greater than zero".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await?;
        Self::lock_task_graph(&mut transaction, user_id).await?;
        let has_subtasks = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE parent_task_id = $1 AND user_id = $2)",
        )
        .bind(task_id)
        .bind(user_id)
        .fetch_one(&mut *transaction)
        .await?;
        if has_subtasks {
            return Err(AppError::Conflict(
                "task cannot be deleted while it has subtasks".to_owned(),
            ));
        }
        Self::remove_task_calendar_event(&mut transaction, user_id, task_id).await?;

        let result =
            sqlx::query("DELETE FROM tasks WHERE id = $1 AND user_id = $2 AND version = $3")
                .bind(task_id)
                .bind(user_id)
                .bind(expected_version)
                .execute(&mut *transaction)
                .await?;

        if result.rows_affected() == 1 {
            transaction.commit().await?;
            return Ok(());
        }

        let current_version = sqlx::query_scalar::<_, i32>(
            "SELECT version FROM tasks WHERE id = $1 AND user_id = $2",
        )
        .bind(task_id)
        .bind(user_id)
        .fetch_optional(&mut *transaction)
        .await?;

        match current_version {
            Some(version) => Err(AppError::Conflict(format!(
                "task changed since version {expected_version}; current version is {version}"
            ))),
            None => Err(AppError::NotFound("task")),
        }
    }

    async fn next_task_position(connection: &mut PgConnection, user_id: Uuid) -> AppResult<i64> {
        let position = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(position), 0) + 1024 FROM tasks WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_one(connection)
        .await?;
        Ok(position)
    }

    async fn remove_next_occurrence_for_reopen(
        connection: &mut PgConnection,
        user_id: Uuid,
        task_id: Uuid,
    ) -> AppResult<()> {
        let next = sqlx::query_as::<_, (Uuid, TaskStatus, i32)>(
            "SELECT id, status, version FROM tasks WHERE recurrence_source_id = $1 AND user_id = $2",
        )
        .bind(task_id)
        .bind(user_id)
        .fetch_optional(&mut *connection)
        .await?;

        match next {
            Some((next_id, TaskStatus::Todo, 1)) => {
                sqlx::query("DELETE FROM tasks WHERE id = $1 AND user_id = $2")
                    .bind(next_id)
                    .bind(user_id)
                    .execute(connection)
                    .await?;
            }
            Some(_) => {
                return Err(AppError::Conflict(
                    "task cannot be reopened after its next occurrence was changed".to_owned(),
                ));
            }
            None => {}
        }
        Ok(())
    }

    async fn create_next_recurring_task(
        connection: &mut PgConnection,
        user_id: Uuid,
        task: &Task,
    ) -> AppResult<()> {
        let source = crate::task_recurrence::RecurringTaskState::load(connection, user_id, task.id)
            .await?
            .successor_source();
        let task = &source;
        Self::ensure_labels(connection, user_id, &task.labels).await?;
        let due_at = task.due_at.ok_or_else(|| {
            AppError::Validation("a recurring task must have a deadline".to_owned())
        })?;
        let next_due_at = next_recurrence_date(due_at, task.recurrence)?;
        let shift = next_due_at - due_at;
        let position = Self::next_task_position(connection, user_id).await?;

        let next = sqlx::query_as::<_, Task>(
            r#"
            INSERT INTO tasks (
                id,
                user_id,
                project_id,
                parent_task_id,
                recurrence_source_id,
                title,
                description,
                due_at,
                scheduled_start,
                scheduled_end,
                status,
                priority,
                recurrence,
                labels,
                remind_at,
                position,
                completed_at
            )
            VALUES (
                $1, $2, $3, NULL, $4, $5, $6, $7, $8,
                $9, 'todo', $10, $11, $12, $13, $14, NULL
            )
            RETURNING
                id, project_id, parent_task_id, title, description, due_at,
                scheduled_start, scheduled_end, status, priority, recurrence,
                labels, remind_at, position, completed_at, created_at, updated_at, version
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(task.project_id)
        .bind(task.id)
        .bind(&task.title)
        .bind(&task.description)
        .bind(next_due_at)
        .bind(task.scheduled_start.map(|value| value + shift))
        .bind(task.scheduled_end.map(|value| value + shift))
        .bind(task.priority)
        .bind(task.recurrence)
        .bind(&task.labels)
        .bind(task.remind_at.map(|value| value + shift))
        .bind(position)
        .fetch_one(&mut *connection)
        .await?;
        Self::sync_task_calendar_event(connection, user_id, &next).await?;
        Ok(())
    }

    async fn ensure_labels(
        connection: &mut PgConnection,
        user_id: Uuid,
        labels: &[String],
    ) -> AppResult<()> {
        for label in labels {
            sqlx::query(
                "INSERT INTO labels (user_id, name) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(user_id)
            .bind(label)
            .execute(&mut *connection)
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn lock_task_graph(
        connection: &mut PgConnection,
        user_id: Uuid,
    ) -> AppResult<()> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::TEXT, 0))")
            .bind(user_id.to_string())
            .execute(connection)
            .await?;
        Ok(())
    }

    async fn validate_task_update_relationships(
        &self,
        connection: &mut PgConnection,
        user_id: Uuid,
        task_id: Uuid,
        project_id: Option<Uuid>,
        parent_task_id: Option<Uuid>,
        recurrence: TaskRecurrence,
    ) -> AppResult<()> {
        let current_project = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT project_id FROM tasks WHERE id = $1 AND user_id = $2",
        )
        .bind(task_id)
        .bind(user_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(AppError::NotFound("task"))?;

        let has_subtasks = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE parent_task_id = $1 AND user_id = $2)",
        )
        .bind(task_id)
        .bind(user_id)
        .fetch_one(&mut *connection)
        .await?;

        if recurrence != TaskRecurrence::None && has_subtasks {
            return Err(AppError::Validation(
                "a task with subtasks cannot recur".to_owned(),
            ));
        }

        if current_project != project_id && has_subtasks {
            return Err(AppError::Validation(
                "a task with subtasks cannot change its project assignment".to_owned(),
            ));
        }

        if let Some(parent_task_id) = parent_task_id {
            let creates_cycle = sqlx::query_scalar::<_, bool>(
                r#"
                WITH RECURSIVE parent_chain AS (
                    SELECT id, parent_task_id
                    FROM tasks
                    WHERE id = $1 AND user_id = $2
                    UNION
                    SELECT task.id, task.parent_task_id
                    FROM tasks task
                    JOIN parent_chain parent ON task.id = parent.parent_task_id
                    WHERE task.user_id = $2
                )
                SELECT EXISTS(SELECT 1 FROM parent_chain WHERE id = $3)
                "#,
            )
            .bind(parent_task_id)
            .bind(user_id)
            .bind(task_id)
            .fetch_one(&mut *connection)
            .await?;

            if creates_cycle {
                return Err(AppError::Validation(
                    "parent_task_id would create a subtask cycle".to_owned(),
                ));
            }
        }

        Ok(())
    }

    async fn validate_task_relationships(
        &self,
        connection: &mut PgConnection,
        user_id: Uuid,
        project_id: Option<Uuid>,
        parent_task_id: Option<Uuid>,
    ) -> AppResult<()> {
        if let Some(project_id) = project_id {
            let project_exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1 AND user_id = $2)",
            )
            .bind(project_id)
            .bind(user_id)
            .fetch_one(&mut *connection)
            .await?;

            if !project_exists {
                return Err(AppError::Validation(
                    "project_id must reference one of the user's projects".to_owned(),
                ));
            }
        }

        if let Some(parent_task_id) = parent_task_id {
            let parent = sqlx::query_as::<_, (Option<Uuid>, TaskRecurrence)>(
                "SELECT project_id, recurrence FROM tasks WHERE id = $1 AND user_id = $2",
            )
            .bind(parent_task_id)
            .bind(user_id)
            .fetch_optional(&mut *connection)
            .await?;

            match parent {
                Some((parent_project, _)) if parent_project != project_id => {
                    return Err(AppError::Validation(
                        "parent task must belong to the same project".to_owned(),
                    ));
                }
                Some((_, recurrence)) if recurrence != TaskRecurrence::None => {
                    return Err(AppError::Validation(
                        "a recurring task cannot have subtasks".to_owned(),
                    ));
                }
                Some(_) => {}
                None => {
                    return Err(AppError::Validation(
                        "parent_task_id must reference one of the user's tasks".to_owned(),
                    ));
                }
            }
        }

        Ok(())
    }
}

pub struct CreatedSession {
    pub token: String,
    pub response: SessionResponse,
}

#[derive(sqlx::FromRow)]
pub struct OAuthLoginAttempt {
    pub nonce: String,
    pub pkce_verifier: String,
    pub user_id: Option<Uuid>,
    pub purpose: String,
    pub terms_version: Option<String>,
    pub privacy_version: Option<String>,
    pub age_confirmed: bool,
}

#[derive(sqlx::FromRow)]
pub struct SessionAuthentication {
    pub user_id: Uuid,
    pub csrf_token: String,
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn validate_project_fields(name: &str, outcome: &str) -> AppResult<()> {
    if name.trim().is_empty() || name.trim().chars().count() > 120 {
        return Err(AppError::Validation(
            "project name must contain between 1 and 120 characters".to_owned(),
        ));
    }
    if outcome.chars().count() > 2_000 {
        return Err(AppError::Validation(
            "project outcome cannot exceed 2000 characters".to_owned(),
        ));
    }
    Ok(())
}

fn validate_task_fields(
    title: &str,
    description: &str,
    due_at: Option<chrono::DateTime<Utc>>,
    scheduled_start: Option<chrono::DateTime<Utc>>,
    scheduled_end: Option<chrono::DateTime<Utc>>,
    recurrence: TaskRecurrence,
    parent_task_id: Option<Uuid>,
) -> AppResult<()> {
    if title.trim().is_empty() || title.trim().chars().count() > 240 {
        return Err(AppError::Validation(
            "task title must contain between 1 and 240 characters".to_owned(),
        ));
    }
    if description.chars().count() > 10_000 {
        return Err(AppError::Validation(
            "task description cannot exceed 10000 characters".to_owned(),
        ));
    }
    if recurrence != TaskRecurrence::None && due_at.is_none() {
        return Err(AppError::Validation(
            "a recurring task must have a deadline".to_owned(),
        ));
    }
    if recurrence != TaskRecurrence::None && parent_task_id.is_some() {
        return Err(AppError::Validation("a subtask cannot recur".to_owned()));
    }

    match (scheduled_start, scheduled_end) {
        (None, None) => {}
        (Some(start), Some(end)) if end > start => {}
        (Some(_), Some(_)) => {
            return Err(AppError::Validation(
                "scheduled_end must be after scheduled_start".to_owned(),
            ));
        }
        _ => {
            return Err(AppError::Validation(
                "scheduled_start and scheduled_end must be provided together".to_owned(),
            ));
        }
    }

    Ok(())
}

fn validate_labels(labels: Vec<String>) -> AppResult<Vec<String>> {
    let mut normalized_labels = Vec::new();
    let mut seen = HashSet::new();
    for label in labels {
        let label = label.trim().to_lowercase();
        if label.is_empty() {
            continue;
        }
        if label.chars().count() > 32 {
            return Err(AppError::Validation(
                "task labels cannot exceed 32 characters".to_owned(),
            ));
        }
        if seen.insert(label.clone()) {
            normalized_labels.push(label);
        }
    }
    if normalized_labels.len() > 10 {
        return Err(AppError::Validation(
            "a task cannot have more than 10 labels".to_owned(),
        ));
    }

    Ok(normalized_labels)
}

fn next_recurrence_date(
    due_at: chrono::DateTime<Utc>,
    recurrence: TaskRecurrence,
) -> AppResult<chrono::DateTime<Utc>> {
    let next = match recurrence {
        TaskRecurrence::None => None,
        TaskRecurrence::Daily => due_at.checked_add_signed(TimeDelta::days(1)),
        TaskRecurrence::Weekly => due_at.checked_add_signed(TimeDelta::weeks(1)),
        TaskRecurrence::Monthly => due_at.checked_add_months(Months::new(1)),
        TaskRecurrence::Yearly => due_at.checked_add_months(Months::new(12)),
    };
    next.ok_or_else(|| AppError::Validation("recurrence date is out of range".to_owned()))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use crate::models::TaskRecurrence;

    use super::validate_task_fields;

    #[test]
    fn rejects_incomplete_schedule() {
        let error = validate_task_fields(
            "Write tests",
            "",
            None,
            Some(Utc::now()),
            None,
            TaskRecurrence::None,
            None,
        )
        .expect_err("incomplete schedule must fail");

        assert_eq!(
            error.to_string(),
            "scheduled_start and scheduled_end must be provided together"
        );
    }

    #[test]
    fn accepts_ordered_schedule() {
        let start = Utc::now();
        let end = start + Duration::hours(1);

        validate_task_fields(
            "Write tests",
            "",
            None,
            Some(start),
            Some(end),
            TaskRecurrence::None,
            None,
        )
        .expect("ordered schedule should be valid");
    }
}
