//! Bounded, owner-scoped daily calendar summary. No provider requests.
use chrono::NaiveDate;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{
    discord::{message, plain_title},
    error::AppResult,
};

#[derive(FromRow)]
struct Event {
    title: String,
    all_day: bool,
    start_label: String,
    end_label: String,
}

pub(crate) struct EventAgenda {
    pub lines: Vec<String>,
    pub more: bool,
}

pub(crate) async fn events(
    connection: &mut PgConnection,
    user: Uuid,
    timezone: &str,
    day: NaiveDate,
) -> AppResult<EventAgenda> {
    let rows = sqlx::query_as::<_, Event>(include_str!("discord_agenda.sql"))
        .bind(user)
        .bind(timezone)
        .bind(day)
        .fetch_all(connection)
        .await?;
    Ok(EventAgenda {
        more: rows.len() > 10,
        lines: rows
            .iter()
            .take(10)
            .map(|event| {
                let time = if event.all_day {
                    "All day".to_owned()
                } else {
                    format!("{}-{}", event.start_label, event.end_label)
                };
                format!("• {time} `{}`", plain_title(&event.title, 40))
            })
            .collect(),
    })
}

pub(crate) fn render(
    day: NaiveDate,
    timezone: &str,
    task_titles: &[String],
    events: Option<EventAgenda>,
) -> serde_json::Value {
    let sharing_events = events.is_some();
    let mut text = format!("Today ({day}, {timezone})\n");
    if let Some(events) = events {
        text.push_str("Events\n");
        if events.lines.is_empty() {
            text.push_str("No events today.\n");
        }
        for line in events.lines.iter().take(10) {
            text.push_str(line);
            text.push('\n');
        }
        if events.more {
            text.push_str("More events in rakkio.\n");
        }
    } else {
        text.push_str("Events are not shared yet. Review the updated Discord sharing notice in rakkio Settings.\n");
    }
    text.push_str("\nTasks\n");
    if task_titles.is_empty() {
        text.push_str("No unfinished focus, due, overdue, or scheduled tasks today.\n");
    }
    for title in task_titles.iter().take(25) {
        text.push_str(&format!(
            "• `{}`\n",
            plain_title(title, if sharing_events { 30 } else { 60 })
        ));
    }
    text.push_str("Use /done to complete a task. Open rakkio for the full plan.");
    message(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture(pool: &sqlx::PgPool) -> anyhow::Result<(Uuid, Uuid)> {
        let user = Uuid::now_v7();
        crate::store::Store::from_pool(pool.clone())
            .ensure_development_user(user)
            .await?;
        let calendar = Uuid::now_v7();
        sqlx::query("INSERT INTO calendars(id,user_id,name) VALUES($1,$2,'Agenda')")
            .bind(calendar)
            .bind(user)
            .execute(pool)
            .await?;
        Ok((user, calendar))
    }

    async fn seed(
        pool: &sqlx::PgPool,
        user: Uuid,
        calendar: Uuid,
        title: &str,
        start: &str,
        end: &str,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO calendar_events(id,user_id,calendar_id,title,starts_at,ends_at) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(id).bind(user).bind(calendar).bind(title).bind(start.parse::<chrono::DateTime<chrono::Utc>>()?).bind(end.parse::<chrono::DateTime<chrono::Utc>>()?).execute(pool).await?;
        Ok(id)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn date_only_and_exclusive_day_boundaries_are_timezone_safe(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let (user, calendar) = fixture(&pool).await?;
        for (title, start, end) in [
            (
                "All-day today",
                "2026-11-03T00:00:00Z",
                "2026-11-04T00:00:00Z",
            ),
            (
                "All-day ended",
                "2026-11-02T00:00:00Z",
                "2026-11-03T00:00:00Z",
            ),
            (
                "All-day tomorrow",
                "2026-11-04T00:00:00Z",
                "2026-11-05T00:00:00Z",
            ),
        ] {
            let id = seed(&pool, user, calendar, title, start, end).await?;
            sqlx::query("UPDATE calendar_events SET all_day=true WHERE id=$1")
                .bind(id)
                .execute(&pool)
                .await?;
        }
        seed(
            &pool,
            user,
            calendar,
            "Crossing midnight",
            "2026-11-02T09:30:00Z",
            "2026-11-02T10:30:00Z",
        )
        .await?;
        seed(
            &pool,
            user,
            calendar,
            "Ended at boundary",
            "2026-11-02T09:00:00Z",
            "2026-11-02T10:00:00Z",
        )
        .await?;
        seed(
            &pool,
            user,
            calendar,
            "Starts tomorrow",
            "2026-11-03T10:00:00Z",
            "2026-11-03T11:00:00Z",
        )
        .await?;
        let day = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let mut connection = pool.acquire().await?;
        for zone in ["Pacific/Kiritimati", "Etc/GMT+12"] {
            let agenda = events(&mut connection, user, zone, day).await?;
            let text = agenda.lines.join("\n");
            assert!(text.contains("All day `All-day today`"), "{text}");
            assert!(!text.contains("All-day ended"));
            assert!(!text.contains("All-day tomorrow"));
            if zone == "Pacific/Kiritimati" {
                assert!(
                    text.contains("11-02 23:30-00:30 `Crossing midnight`"),
                    "{text}"
                );
                assert!(!text.contains("Ended at boundary"));
                assert!(!text.contains("Starts tomorrow"));
            }
        }
        Ok(())
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn recurrence_preserves_local_time_skips_missing_dates_and_honors_until(
        pool: sqlx::PgPool,
    ) -> anyhow::Result<()> {
        let (user, calendar) = fixture(&pool).await?;
        let id = seed(
            &pool,
            user,
            calendar,
            "DST meeting",
            "2026-03-01T14:00:00Z",
            "2026-03-01T15:00:00Z",
        )
        .await?;
        sqlx::query("UPDATE calendar_events SET timezone='America/New_York',recurrence='weekly' WHERE id=$1").bind(id).execute(&pool).await?;
        let id = seed(
            &pool,
            user,
            calendar,
            "Month end",
            "2026-01-31T09:00:00Z",
            "2026-01-31T10:00:00Z",
        )
        .await?;
        sqlx::query("UPDATE calendar_events SET recurrence='monthly' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await?;
        let id = seed(
            &pool,
            user,
            calendar,
            "Leap day",
            "2024-02-29T09:00:00Z",
            "2024-02-29T10:00:00Z",
        )
        .await?;
        sqlx::query("UPDATE calendar_events SET recurrence='yearly' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await?;
        let id = seed(
            &pool,
            user,
            calendar,
            "Expired series",
            "2026-01-01T09:00:00Z",
            "2026-01-01T10:00:00Z",
        )
        .await?;
        sqlx::query("UPDATE calendar_events SET recurrence='daily',recurrence_until='2026-01-02T08:00:00Z' WHERE id=$1").bind(id).execute(&pool).await?;
        let mut connection = pool.acquire().await?;
        for (date, present, absent) in [
            ("2026-03-08", "13:00-14:00 `DST meeting`", "Expired series"),
            ("2026-02-28", "", "Month end"),
            ("2026-03-31", "Month end", "Leap day"),
            ("2025-02-28", "", "Leap day"),
            ("2028-02-29", "Leap day", "Expired series"),
        ] {
            let text = events(&mut connection, user, "UTC", date.parse()?)
                .await?
                .lines
                .join("\n");
            assert!(text.contains(present), "{date}: {text}");
            assert!(!text.contains(absent), "{date}: {text}");
        }
        Ok(())
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn event_scan_and_response_are_bounded(pool: sqlx::PgPool) -> anyhow::Result<()> {
        let (user, calendar) = fixture(&pool).await?;
        for index in 0..12 {
            seed(
                &pool,
                user,
                calendar,
                &format!("Event {index} {}", "😀".repeat(110)),
                "2026-11-03T08:00:00Z",
                "2026-11-03T09:00:00Z",
            )
            .await?;
        }
        let day = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let mut connection = pool.acquire().await?;
        let agenda = events(&mut connection, user, "UTC", day).await?;
        assert_eq!(agenda.lines.len(), 10);
        assert!(agenda.more);
        let response = render(day, "UTC", &vec!["😀".repeat(120); 25], Some(agenda));
        assert!(response["content"].as_str().unwrap().encode_utf16().count() <= 2000);
        assert!(serde_json::to_vec(&response)?.len() <= 8192);
        Ok(())
    }

    #[test]
    fn both_notice_modes_fit_discord_and_disable_mentions() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let tasks = vec!["😀`\n\u{202e}".repeat(120); 25];
        for events in [
            None,
            Some(EventAgenda {
                lines: vec![format!("• 11-02 23:00-11-04 01:00 `{}`", "😀".repeat(20)); 10],
                more: true,
            }),
        ] {
            let response = render(day, &"x".repeat(128), &tasks, events);
            assert!(response["content"].as_str().unwrap().encode_utf16().count() <= 2000);
            assert_eq!(response["allowed_mentions"]["parse"], serde_json::json!([]));
        }
    }
}
