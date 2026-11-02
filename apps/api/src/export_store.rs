use serde::Serialize;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{Calendar, CalendarEvent, FileRecord, Note, Project, Task, UserProfile},
    store::Store,
};

impl Store {
    pub async fn export_json(&self, user_id: Uuid) -> AppResult<Vec<u8>> {
        let bundle = self.export_bundle(user_id).await?;
        serde_json::to_vec_pretty(&bundle).map_err(|error| {
            tracing::error!(error = ?error, "failed to serialize account export");
            AppError::InvalidRequest {
                status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                message: "could not create account export".to_owned(),
            }
        })
    }

    pub async fn export_tasks_csv(&self, user_id: Uuid) -> AppResult<Vec<u8>> {
        let tasks = self.export_tasks(user_id).await?;
        let mut csv = String::from(
            "id,project_id,parent_task_id,title,description,due_at,scheduled_start,scheduled_end,status,priority,recurrence,labels,completed_at,created_at,updated_at\r\n",
        );
        for task in tasks {
            let fields = [
                task.id.to_string(),
                task.project_id.map(|id| id.to_string()).unwrap_or_default(),
                task.parent_task_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                task.title,
                task.description,
                optional_display(task.due_at),
                optional_display(task.scheduled_start),
                optional_display(task.scheduled_end),
                format!("{:?}", task.status).to_lowercase(),
                format!("{:?}", task.priority).to_lowercase(),
                format!("{:?}", task.recurrence).to_lowercase(),
                task.labels.join(";"),
                optional_display(task.completed_at),
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
            ];
            csv.push_str(
                &fields
                    .into_iter()
                    .map(|field| csv_field(&field))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            csv.push_str("\r\n");
        }
        Ok(csv.into_bytes())
    }

    pub async fn export_notes_markdown(&self, user_id: Uuid) -> AppResult<Vec<u8>> {
        let notes = sqlx::query_as::<_, Note>(
            r#"
            SELECT id, project_id, task_id, event_id, title, markdown,
                   created_at, updated_at, version
            FROM notes WHERE user_id = $1 ORDER BY created_at, id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut markdown = String::from("# rakkio notes\n\n");
        for note in notes {
            markdown.push_str(&format!(
                "## {}\n\n{}\n\n---\n\n",
                note.title, note.markdown
            ));
        }
        Ok(markdown.into_bytes())
    }

    pub async fn export_calendars_ics(&self, user_id: Uuid) -> AppResult<Vec<u8>> {
        let events = sqlx::query_as::<_, CalendarEvent>(
            r#"
            SELECT id, calendar_id, linked_task_id, title, description, starts_at, ends_at,
                   all_day, timezone, location, attendees, recurrence, recurrence_until,
                   created_at, updated_at, version
            FROM calendar_events WHERE user_id = $1 ORDER BY starts_at, id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut ics = String::from(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//rakkio//EN\r\nCALSCALE:GREGORIAN\r\n",
        );
        for event in events {
            ics.push_str("BEGIN:VEVENT\r\n");
            ics.push_str(&format!("UID:{}@prosepect\r\n", event.id));
            ics.push_str(&format!("DTSTAMP:{}\r\n", ics_time(event.updated_at)));
            ics.push_str(&format!("DTSTART:{}\r\n", ics_time(event.starts_at)));
            ics.push_str(&format!("DTEND:{}\r\n", ics_time(event.ends_at)));
            ics.push_str(&format!("SUMMARY:{}\r\n", ics_escape(&event.title)));
            if !event.description.is_empty() {
                ics.push_str(&format!(
                    "DESCRIPTION:{}\r\n",
                    ics_escape(&event.description)
                ));
            }
            if !event.location.is_empty() {
                ics.push_str(&format!("LOCATION:{}\r\n", ics_escape(&event.location)));
            }
            let frequency = format!("{:?}", event.recurrence).to_uppercase();
            if frequency != "NONE" {
                ics.push_str(&format!("RRULE:FREQ={frequency}"));
                if let Some(until) = event.recurrence_until {
                    ics.push_str(&format!(";UNTIL={}", ics_time(until)));
                }
                ics.push_str("\r\n");
            }
            ics.push_str("END:VEVENT\r\n");
        }
        ics.push_str("END:VCALENDAR\r\n");
        Ok(ics.into_bytes())
    }

    pub async fn delete_account(&self, user_id: Uuid, confirmation: &str) -> AppResult<()> {
        if confirmation != "DELETE" {
            return Err(AppError::Validation(
                "account deletion confirmation must equal DELETE".to_owned(),
            ));
        }
        let result = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::Unauthorized);
        }
        Ok(())
    }

    async fn export_bundle(&self, user_id: Uuid) -> AppResult<AccountExport> {
        let user = self.user_profile(user_id).await?;
        let projects = sqlx::query_as::<_, Project>(
            r#"
            SELECT p.id, p.name, p.outcome, p.target_date, p.status,
                   COUNT(t.id)::BIGINT AS total_tasks,
                   COUNT(t.id) FILTER (WHERE t.status = 'completed')::BIGINT AS completed_tasks,
                   p.created_at, p.updated_at, p.version
            FROM projects p
            LEFT JOIN tasks t ON t.project_id = p.id AND t.user_id = p.user_id
            WHERE p.user_id = $1 GROUP BY p.id ORDER BY p.created_at, p.id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let tasks = self.export_tasks(user_id).await?;
        let notes = self.list_notes(user_id).await?.items;
        let calendars = sqlx::query_as::<_, Calendar>(
            r#"
            SELECT id, name, color, source, external_id, selected, is_default,
                   created_at, updated_at, version
            FROM calendars WHERE user_id = $1 ORDER BY created_at, id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let events = sqlx::query_as::<_, CalendarEvent>(
            r#"
            SELECT id, calendar_id, linked_task_id, title, description, starts_at, ends_at,
                   all_day, timezone, location, attendees, recurrence, recurrence_until,
                   created_at, updated_at, version
            FROM calendar_events WHERE user_id = $1 ORDER BY starts_at, id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let files = sqlx::query_as::<_, FileRecord>(
            r#"
            SELECT id, project_id, task_id, note_id, event_id, filename,
                   content_type, byte_size, created_at
            FROM files WHERE user_id = $1 ORDER BY created_at, id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let saved_task_views = self.saved_task_views(user_id).await?;
        let recurrence_templates = self.recurrence_templates(user_id).await?;
        let discord_connection = sqlx::query_scalar::<_, serde_json::Value>("SELECT jsonb_build_object('discord_user_id',discord_user_id,'username',username,'timezone',timezone,'consent_version',consent_version,'connected_at',connected_at) FROM discord_connections WHERE user_id=$1")
            .bind(user_id).fetch_optional(&self.pool).await?;
        Ok(AccountExport {
            discord_connection,
            recurrence_templates,
            saved_task_views,
            user,
            projects,
            tasks,
            notes,
            calendars,
            events,
            files,
        })
    }

    async fn export_tasks(&self, user_id: Uuid) -> AppResult<Vec<Task>> {
        sqlx::query_as::<_, Task>(
            r#"
            SELECT id, project_id, parent_task_id, title, description, due_at,
                   scheduled_start, scheduled_end, status, priority, recurrence, labels,
                   remind_at, position, completed_at, created_at, updated_at, version
            FROM tasks WHERE user_id = $1 ORDER BY created_at, id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }
}

#[derive(Serialize)]
struct AccountExport {
    discord_connection: Option<serde_json::Value>,
    recurrence_templates: Vec<crate::task_recurrence::RecurrenceTemplateExport>,
    saved_task_views: Vec<crate::saved_views::SavedTaskView>,
    user: UserProfile,
    projects: Vec<Project>,
    tasks: Vec<Task>,
    notes: Vec<Note>,
    calendars: Vec<Calendar>,
    events: Vec<CalendarEvent>,
    files: Vec<FileRecord>,
}

fn optional_display<T: ToString>(value: Option<T>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn csv_field(value: &str) -> String {
    // CSV quoting alone does not stop spreadsheet evaluation. Neutralize the
    // exported cell, never the canonical task, including hidden prefixes.
    let trimmed =
        value.trim_start_matches(|c: char| c.is_whitespace() || c.is_control() || c == '\u{feff}');
    let unsafe_cell = value.starts_with(['\t', '\r', '\n'])
        || trimmed.starts_with(['=', '+', '-', '@', '＝', '＋', '－', '＠']);
    let value = if unsafe_cell {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if unsafe_cell || value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn ics_time(value: chrono::DateTime<chrono::Utc>) -> String {
    value.format("%Y%m%dT%H%M%SZ").to_string()
}

fn ics_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

#[cfg(test)]
mod security_tests {
    use super::csv_field;

    #[test]
    fn csv_guards_formulas_control_prefixes_and_delimiters() {
        for value in [
            "=1+1",
            "+1",
            "-1",
            "@SUM(1,2)",
            "\ttext",
            "\rtext",
            "\ntext",
            "  =1+1",
            "\u{feff}=1",
            "\0=1",
            "＝1",
            "＋1",
            "－1",
            "＠SUM(1,2)",
        ] {
            assert!(csv_field(value).starts_with("\"'"));
        }
        for value in ["", "text", "42", "  text", "person@example.test", "'=1"] {
            assert_eq!(csv_field(value), value);
        }
        assert_eq!(csv_field("a,\"b\"\r\nc"), "\"a,\"\"b\"\"\r\nc\"");
    }
}
