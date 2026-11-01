use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Acquire, FromRow, PgConnection};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    discord::{self, Interaction, message, plain_title},
    error::{AppError, AppResult},
    models::{CreateTaskRequest, Task, TaskStatus, UpdateTaskRequest},
    store::Store,
};

#[derive(Serialize, ToSchema, FromRow)]
pub struct DiscordConnection {
    pub id: Uuid,
    pub username: String,
    pub timezone: String,
    pub connected_at: DateTime<Utc>,
}

#[derive(FromRow)]
pub(crate) struct DiscordAttempt {
    pub user_id: Uuid,
    pub pkce_verifier: String,
    pub timezone: String,
}

#[derive(FromRow)]
pub(crate) struct CommandOwner {
    pub id: Uuid,
    pub user_id: Uuid,
    pub timezone: String,
}

impl Store {
    pub(crate) async fn cleanup_discord(&self) -> AppResult<()> {
        sqlx::query("DELETE FROM discord_interactions WHERE id IN (SELECT id FROM discord_interactions WHERE created_at < NOW()-INTERVAL '15 minutes' ORDER BY created_at LIMIT 100 FOR UPDATE SKIP LOCKED)")
            .execute(&self.pool).await?;
        sqlx::query("DELETE FROM discord_oauth_attempts WHERE state_hash IN (SELECT state_hash FROM discord_oauth_attempts WHERE expires_at < NOW() ORDER BY expires_at LIMIT 100 FOR UPDATE SKIP LOCKED)")
            .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn discord_connection(&self, user: Uuid) -> AppResult<Option<DiscordConnection>> {
        Ok(sqlx::query_as(
            "SELECT id,username,timezone,connected_at FROM discord_connections WHERE user_id=$1",
        )
        .bind(user)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub(crate) async fn begin_discord_link(
        &self,
        user: Uuid,
        state: &str,
        browser: &str,
        verifier: &str,
        timezone: &str,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        Self::lock_task_graph(&mut tx, user).await?;
        let valid: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_timezone_names WHERE name=$1)")
                .bind(timezone)
                .fetch_one(&mut *tx)
                .await?;
        if !valid || timezone.len() > 128 {
            return Err(AppError::Validation("Choose a valid IANA timezone.".into()));
        }
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM discord_connections WHERE user_id=$1)",
        )
        .bind(user)
        .fetch_one(&mut *tx)
        .await?
        {
            return Err(AppError::Conflict(
                "Disconnect the current Discord account before linking another.".into(),
            ));
        }
        sqlx::query("DELETE FROM discord_oauth_attempts WHERE user_id=$1 AND expires_at < NOW()")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        let attempts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM discord_oauth_attempts WHERE user_id=$1")
                .bind(user)
                .fetch_one(&mut *tx)
                .await?;
        if attempts >= 5 {
            return Err(AppError::Validation(
                "Too many pending connections. Wait ten minutes before retrying.".into(),
            ));
        }
        sqlx::query("INSERT INTO discord_oauth_attempts(state_hash,browser_binding_hash,user_id,pkce_verifier,timezone) VALUES($1,$2,$3,$4,$5)")
            .bind(Sha256::digest(state).as_slice()).bind(Sha256::digest(browser).as_slice())
            .bind(user).bind(verifier).bind(timezone).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn consume_discord_link(
        &self,
        user: Uuid,
        state: &str,
        browser: &str,
    ) -> AppResult<DiscordAttempt> {
        // Wrong browser or owner cannot consume another account's flow.
        sqlx::query_as("DELETE FROM discord_oauth_attempts WHERE state_hash=$1 AND browser_binding_hash=$2 AND user_id=$3 AND expires_at>NOW() RETURNING user_id,pkce_verifier,timezone")
            .bind(Sha256::digest(state).as_slice()).bind(Sha256::digest(browser).as_slice()).bind(user)
            .fetch_optional(&self.pool).await?.ok_or(AppError::Forbidden("Discord connection expired or belongs to another browser/account"))
    }

    pub(crate) async fn finish_discord_link(
        &self,
        attempt: DiscordAttempt,
        identity: discord::DiscordIdentity,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        Self::lock_task_graph(&mut tx, attempt.user_id).await?;
        // Never silently move a Discord identity between rakkio accounts or
        // replace a connection that completed in another tab.
        let inserted = sqlx::query("INSERT INTO discord_connections(id,user_id,discord_user_id,username,timezone,consent_version) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING")
            .bind(Uuid::now_v7()).bind(attempt.user_id).bind(identity.id).bind(identity.username)
            .bind(attempt.timezone).bind(discord::CONSENT_VERSION).execute(&mut *tx).await?;
        if inserted.rows_affected() != 1 {
            return Err(AppError::Conflict("That Discord or rakkio account is already linked. Disconnect its existing connection first.".into()));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn disconnect_discord(&self, user: Uuid, expected_id: Uuid) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        Self::lock_task_graph(&mut tx, user).await?;
        let deleted = sqlx::query("DELETE FROM discord_connections WHERE user_id=$1 AND id=$2")
            .bind(user)
            .bind(expected_id)
            .execute(&mut *tx)
            .await?;
        if deleted.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "Discord connection changed. Refresh before disconnecting.".into(),
            ));
        }
        sqlx::query("DELETE FROM discord_oauth_attempts WHERE user_id=$1")
            .bind(user)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn discord_owner(&self, discord_id: &str) -> AppResult<Option<CommandOwner>> {
        Ok(sqlx::query_as(
            "SELECT id,user_id,timezone FROM discord_connections WHERE discord_user_id=$1",
        )
        .bind(discord_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub(crate) async fn run_discord_command(
        &self,
        owner: CommandOwner,
        interaction: &Interaction,
    ) -> AppResult<Value> {
        let mut tx = self.pool.begin().await?;
        Self::lock_task_graph(&mut tx, owner.user_id).await?;
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM discord_connections WHERE id=$1 AND user_id=$2 AND discord_user_id=$3)")
            .bind(owner.id).bind(owner.user_id).bind(interaction.actor()).fetch_one(&mut *tx).await?;
        if !current {
            return Ok(message(
                "Your Discord connection changed. Connect again in rakkio Settings.",
            ));
        }
        if let Some(response) = sqlx::query_scalar::<_, Value>(
            "SELECT response FROM discord_interactions WHERE id=$1 AND connection_id=$2",
        )
        .bind(&interaction.id)
        .bind(owner.id)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(response);
        }
        sqlx::query("DELETE FROM discord_interactions WHERE connection_id=$1 AND created_at<NOW()-INTERVAL '15 minutes'")
            .bind(owner.id).execute(&mut *tx).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM discord_interactions WHERE connection_id=$1")
                .bind(owner.id)
                .fetch_one(&mut *tx)
                .await?;
        if count >= 100 {
            return Ok(message(
                "Too many recent commands. Use rakkio on the web or wait 15 minutes.",
            ));
        }
        // Keep validation/conflict/quota failures idempotent too, without
        // retaining partial task writes from a failed operation.
        let mut operation = tx.begin().await?;
        let response = match self
            .discord_operation(&mut operation, &owner, interaction)
            .await
        {
            Ok(response) => {
                operation.commit().await?;
                response
            }
            Err(error) => {
                operation.rollback().await?;
                match error {
                    AppError::Validation(text) => message(text),
                    AppError::Conflict(_) => message(
                        "The task changed. Run /done again to choose its current version. No changes were made.",
                    ),
                    AppError::NotFound(_) => {
                        message("That task is no longer available. Run /done again.")
                    }
                    AppError::Database(ref error)
                        if error.as_database_error().is_some_and(|error| {
                            error.constraint() == Some("workspace_content_quota")
                        }) =>
                    {
                        message(
                            "Workspace content limit reached. Delete unused content in rakkio before adding tasks.",
                        )
                    }
                    other => return Err(other),
                }
            }
        };
        sqlx::query("INSERT INTO discord_interactions(id,connection_id,response) VALUES($1,$2,$3)")
            .bind(&interaction.id)
            .bind(owner.id)
            .bind(&response)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(response)
    }

    async fn discord_operation(
        &self,
        connection: &mut PgConnection,
        owner: &CommandOwner,
        interaction: &Interaction,
    ) -> AppResult<Value> {
        let data = interaction
            .data
            .as_ref()
            .ok_or_else(|| AppError::Validation("Missing command.".into()))?;
        if interaction.kind == 3 {
            return self.discord_complete(connection, owner, data).await;
        }
        if interaction.kind != 2 {
            return Err(AppError::Validation("Unsupported interaction.".into()));
        }
        match data["name"].as_str() {
            Some("add") => {
                let title = option(data, "title")
                    .ok_or_else(|| AppError::Validation("Provide a task title.".into()))?;
                let due_at = if let Some(due) = option(data, "due") {
                    let date: NaiveDate = match due {
                        "today" => {
                            sqlx::query_scalar("SELECT (NOW() AT TIME ZONE $1)::date")
                                .bind(&owner.timezone)
                                .fetch_one(&mut *connection)
                                .await?
                        }
                        "tomorrow" => {
                            sqlx::query_scalar("SELECT (NOW() AT TIME ZONE $1)::date+1")
                                .bind(&owner.timezone)
                                .fetch_one(&mut *connection)
                                .await?
                        }
                        _ => due.parse().map_err(|_| {
                            AppError::Validation(
                                "Use today, tomorrow, or YYYY-MM-DD for the deadline.".into(),
                            )
                        })?,
                    };
                    Some(
                        sqlx::query_scalar::<_, DateTime<Utc>>(
                            "SELECT ($1::date + TIME '23:59:00') AT TIME ZONE $2",
                        )
                        .bind(date)
                        .bind(&owner.timezone)
                        .fetch_one(&mut *connection)
                        .await?,
                    )
                } else {
                    None
                };
                let request: CreateTaskRequest =
                    serde_json::from_value(json!({ "title": title, "due_at": due_at }))
                        .map_err(|_| AppError::Validation("Invalid task.".into()))?;
                let task = self
                    .create_task_in(connection, owner.user_id, request)
                    .await?;
                Ok(message(format!(
                    "Added to Inbox: {}{}",
                    plain_title(&task.title, 200),
                    option(data, "due")
                        .map(|date| format!(" (deadline: {date}, {})", owner.timezone))
                        .unwrap_or_default()
                )))
            }
            Some("today") => {
                let tasks = selected_tasks(connection, owner, true).await?;
                if tasks.is_empty() {
                    return Ok(message(format!(
                        "No unfinished focus, due, overdue, or scheduled tasks today ({}).",
                        owner.timezone
                    )));
                }
                let mut text = format!("Today ({}) - up to 25 unfinished tasks:\n", owner.timezone);
                for task in tasks {
                    text.push_str(&format!("• {}\n", plain_title(&task.title, 65)));
                }
                text.push_str("Use /done to complete a task. Open rakkio for the full plan.");
                Ok(message(text))
            }
            Some("done") => {
                let tasks = selected_tasks(connection, owner, false).await?;
                if tasks.is_empty() {
                    return Ok(message("No unfinished tasks to complete."));
                }
                let options: Vec<_> = tasks
                    .iter()
                    .map(|task| {
                        let title = plain_title(&task.title, 80);
                        let label = if title.trim().is_empty() {
                            "Untitled task"
                        } else {
                            title.trim()
                        };
                        json!({ "label": label, "value": format!("{}:{}",task.id,task.version) })
                    })
                    .collect();
                let mut response = message(
                    "Choose a task to complete (up to 25 shown, due tasks first). This selection expires in 15 minutes. Use rakkio for other tasks.",
                );
                response["components"] = json!([{ "type":1, "components":[{
                    "type":3, "custom_id":format!("done:{}:{}",owner.id,interaction.id),
                    "placeholder":"Choose a task to complete", "min_values":1, "max_values":1, "options":options,
                }] }]);
                Ok(response)
            }
            _ => Err(AppError::Validation("Use /add, /today, or /done.".into())),
        }
    }

    async fn discord_complete(
        &self,
        connection: &mut PgConnection,
        owner: &CommandOwner,
        data: &Value,
    ) -> AppResult<Value> {
        let invalid =
            || AppError::Validation("This selection expired or changed. Run /done again.".into());
        if data["component_type"] != 3 {
            return Err(invalid());
        }
        let custom = data["custom_id"].as_str().ok_or_else(invalid)?;
        let prefix = format!("done:{}:", owner.id);
        let receipt = custom
            .strip_prefix(&prefix)
            .filter(|id| discord::snowflake(id))
            .ok_or_else(invalid)?;
        let values = data["values"]
            .as_array()
            .filter(|values| values.len() == 1)
            .ok_or_else(invalid)?;
        let selected = values[0].as_str().ok_or_else(invalid)?;
        let previous: Value = sqlx::query_scalar("SELECT response FROM discord_interactions WHERE id=$1 AND connection_id=$2 AND created_at>NOW()-INTERVAL '15 minutes'")
            .bind(receipt).bind(owner.id).fetch_optional(&mut *connection).await?.ok_or_else(invalid)?;
        let offered = previous["components"][0]["components"][0]["options"]
            .as_array()
            .is_some_and(|options| {
                options
                    .iter()
                    .any(|option| option["value"].as_str() == Some(selected))
            });
        if !offered || previous["components"][0]["components"][0]["custom_id"] != custom {
            return Err(invalid());
        }
        let (id, version) = selected.split_once(':').ok_or_else(invalid)?;
        let id: Uuid = id.parse().map_err(|_| invalid())?;
        let version: i32 = version.parse().map_err(|_| invalid())?;
        let task: Task = sqlx::query_as("SELECT * FROM tasks WHERE user_id=$1 AND id=$2")
            .bind(owner.user_id)
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or(AppError::NotFound("task"))?;
        if task.version != version || task.status == TaskStatus::Completed {
            return Err(AppError::Conflict("Task changed".into()));
        }
        let update = UpdateTaskRequest {
            project_id: task.project_id,
            parent_task_id: task.parent_task_id,
            title: task.title,
            description: task.description,
            due_at: task.due_at,
            scheduled_start: task.scheduled_start,
            scheduled_end: task.scheduled_end,
            status: TaskStatus::Completed,
            priority: task.priority,
            recurrence: task.recurrence,
            labels: task.labels,
            remind_at: task.remind_at,
            expected_version: version,
        };
        let task = self
            .update_task_scoped_in(connection, owner.user_id, id, update, None)
            .await?;
        Ok(message(format!(
            "Completed: {}",
            plain_title(&task.title, 200)
        )))
    }
}

fn option<'a>(data: &'a Value, name: &str) -> Option<&'a str> {
    data["options"]
        .as_array()?
        .iter()
        .find(|option| option["name"] == name)?["value"]
        .as_str()
}

async fn selected_tasks(
    connection: &mut PgConnection,
    owner: &CommandOwner,
    today_only: bool,
) -> AppResult<Vec<Task>> {
    Ok(sqlx::query_as("SELECT tasks.* FROM tasks WHERE user_id=$1 AND status<>'completed' AND (NOT $3 OR (due_at AT TIME ZONE $2)::date <= (NOW() AT TIME ZONE $2)::date OR (scheduled_start AT TIME ZONE $2)::date=(NOW() AT TIME ZONE $2)::date OR EXISTS(SELECT 1 FROM daily_focus_tasks focus WHERE focus.user_id=$1 AND focus.task_id=tasks.id AND focus.focus_date=(NOW() AT TIME ZONE $2)::date)) ORDER BY due_at ASC NULLS LAST,position,id LIMIT 25")
        .bind(owner.user_id).bind(&owner.timezone).bind(today_only).fetch_all(connection).await?)
}
