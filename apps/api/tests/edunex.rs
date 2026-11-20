use chrono::{DateTime, Duration, Utc};
use prosepect_api::{
    edunex::{self, EdunexAssignment, EdunexSyncSummary},
    error::AppError,
    models::TaskStatus,
    store::Store,
};
use sqlx::PgPool;
use uuid::Uuid;

async fn user(pool: &PgPool) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users(id,email,display_name) VALUES($1,$2,'Edunex fixture')")
        .bind(id)
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await?;
    Ok(id)
}

fn task(id: &str, title: &str, due_at: DateTime<Utc>, submitted: bool) -> EdunexAssignment {
    EdunexAssignment {
        id: id.into(),
        title: title.into(),
        course_name: "Algoritma".into(),
        due_at,
        submitted,
    }
}

async fn imported(pool: &PgPool, user: Uuid) -> anyhow::Result<Vec<(String, TaskStatus)>> {
    Ok(sqlx::query_as(
        "SELECT title, status FROM tasks WHERE user_id = $1 AND 'edunex' = ANY(labels) ORDER BY title",
    )
    .bind(user)
    .fetch_all(pool)
    .await?)
}

#[sqlx::test(migrations = "../../migrations")]
async fn imports_once_follows_edunex_changes_and_completes_on_submission(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let owner = user(&pool).await?;
    let now: DateTime<Utc> = "2026-11-20T03:00:00Z".parse()?;
    let due = now + Duration::days(2);
    let first = [
        task("1", "[IF2110] Tugas 1", due, false),
        // Already submitted or long past: never imported.
        task("2", "[IF2110] Done", due, true),
        task("3", "[IF2110] Old", now - Duration::days(30), false),
    ];
    let summary = edunex::apply(&store, owner, &first, now).await?;
    assert_eq!(
        summary,
        EdunexSyncSummary {
            created: 1,
            updated: 0,
            completed: 0
        }
    );
    assert_eq!(
        imported(&pool, owner).await?,
        [("[IF2110] Tugas 1".to_owned(), TaskStatus::Todo)]
    );

    // Repeating the same data is a no-op.
    assert_eq!(
        edunex::apply(&store, owner, &first, now).await?,
        EdunexSyncSummary::default()
    );

    // A deadline extension on Edunex is copied; a local rename is kept.
    sqlx::query("UPDATE tasks SET description = 'my notes' WHERE user_id = $1")
        .bind(owner)
        .execute(&pool)
        .await?;
    let extended = due + Duration::days(3);
    let summary = edunex::apply(
        &store,
        owner,
        &[task("1", "[IF2110] Tugas 1", extended, false)],
        now,
    )
    .await?;
    assert_eq!(summary.updated, 1);
    let (due_at, description): (DateTime<Utc>, String) =
        sqlx::query_as("SELECT due_at, description FROM tasks WHERE user_id = $1")
            .bind(owner)
            .fetch_one(&pool)
            .await?;
    assert_eq!(due_at, extended);
    assert_eq!(description, "my notes");

    // Submitting on Edunex completes the task.
    let summary = edunex::apply(
        &store,
        owner,
        &[task("1", "[IF2110] Tugas 1", extended, true)],
        now,
    )
    .await?;
    assert_eq!(summary.completed, 1);
    assert_eq!(
        imported(&pool, owner).await?,
        [("[IF2110] Tugas 1".to_owned(), TaskStatus::Completed)]
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleted_tasks_are_not_imported_again(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let owner = user(&pool).await?;
    let now = Utc::now();
    let remote = [task("9", "[IF1] Quiz", now + Duration::days(1), false)];
    edunex::apply(&store, owner, &remote, now).await?;
    sqlx::query("DELETE FROM tasks WHERE user_id = $1")
        .bind(owner)
        .execute(&pool)
        .await?;
    assert_eq!(
        edunex::apply(&store, owner, &remote, now).await?,
        EdunexSyncSummary::default()
    );
    assert!(imported(&pool, owner).await?.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_malformed_assignments_without_importing_any(pool: PgPool) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let owner = user(&pool).await?;
    let now = Utc::now();
    let remote = [
        task("1", "[IF1] Valid", now + Duration::days(1), false),
        task("", "Missing id", now + Duration::days(1), false),
    ];
    assert!(matches!(
        edunex::apply(&store, owner, &remote, now).await,
        Err(AppError::Validation(_))
    ));
    assert!(imported(&pool, owner).await?.is_empty());
    Ok(())
}
