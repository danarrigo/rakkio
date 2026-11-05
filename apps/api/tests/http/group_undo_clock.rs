use super::groups::{call, create, fixture};
use super::*;
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

#[derive(Clone, Copy, Debug)]
enum Target {
    Group,
    Assignment,
    Event,
}
async fn expiry_after_lock_wait(pool: PgPool, target: Target) -> anyhow::Result<()> {
    let (router, _, [owner, _, _, _]) = fixture(&pool).await?;
    let group = create(&router, owner).await?;
    let root = format!("/api/v1/groups/{}", group["id"].as_str().unwrap());
    let (entity, delete_path, delete_body, restore_path) = match target {
        Target::Group => (
            group.clone(),
            format!("{root}/actions"),
            json!({"expected_version":group["version"],"action":{"kind":"delete","confirmation":"DELETE GROUP"}}),
            format!("{root}/actions"),
        ),
        Target::Assignment => {
            let (s, e) = call(
                &router,
                owner,
                "POST",
                &format!("{root}/assignments"),
                json!({"title":"Clock guard"}),
            )
            .await?;
            assert_eq!(s, StatusCode::OK);
            let p = format!("{root}/assignments/{}", e["id"].as_str().unwrap());
            let b = json!({"expected_version":e["version"]});
            (
                e,
                format!("{p}/delete-with-undo"),
                b,
                format!("{p}/restore"),
            )
        }
        Target::Event => {
            let (s,e)=call(&router,owner,"POST",&format!("{root}/events"),json!({"event":{"title":"Clock guard","starts_local":"2027-01-01T09:00:00","ends_local":"2027-01-01T10:00:00","all_day":false,"timezone":"UTC","recurrence":"none"}})).await?;
            assert_eq!(s, StatusCode::OK, "{e}");
            let p = format!("{root}/events/{}", e["id"].as_str().unwrap());
            let b = json!({"expected_version":e["version"],"scope":"series"});
            (
                e,
                format!("{p}/delete-with-undo"),
                b,
                format!("{p}/restore"),
            )
        }
    };
    let (s, receipt) = call(&router, owner, "POST", &delete_path, delete_body).await?;
    assert_eq!(s, StatusCode::OK, "{receipt}");
    let body = match target {
        Target::Group => {
            json!({"expected_version":receipt["group"]["version"],"action":{"kind":"restore"}})
        }
        Target::Assignment => json!({"expected_version":receipt["version"]}),
        Target::Event => json!({"expected_version":receipt["version"],"scope":"series"}),
    };
    let mut blocker = pool.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .execute(&mut *blocker)
        .await?;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let pending =
        tokio::spawn(async move { call(&router, owner, "POST", &restore_path, body).await });
    // Observe a real admitted HTTP request waiting inside its transaction. No
    // scheduling sleep is used to guess when BEGIN or the lock attempt happened.
    let began=tokio::time::timeout(std::time::Duration::from_secs(5),async {
  loop {
   let began:Option<DateTime<Utc>>=sqlx::query_scalar("SELECT xact_start FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) LIMIT 1").bind(blocker_pid).fetch_optional(&pool).await?;
   if let Some(began)=began {break Ok::<_,sqlx::Error>(began);}
   tokio::time::sleep(std::time::Duration::from_millis(10)).await;
  }
 }).await??;
    // The deadline is later than the waiter's transaction-start NOW(), but earlier
    // than actual DB wall time. Using NOW() after the wait would wrongly restore.
    let query = match target {
        Target::Group => {
            "UPDATE groups SET deleted_at=$2-INTERVAL '1 second',undo_until=$2+INTERVAL '1 microsecond' WHERE id=$1"
        }
        Target::Assignment => {
            "UPDATE group_assignments SET deleted_at=$2-INTERVAL '1 second',undo_until=$2+INTERVAL '1 microsecond' WHERE id=$1"
        }
        Target::Event => {
            "UPDATE group_events SET deleted_at=$2-INTERVAL '1 second',undo_until=$2+INTERVAL '1 microsecond' WHERE id=$1"
        }
    };
    sqlx::query(query)
        .bind(entity["id"].as_str().unwrap().parse::<Uuid>()?)
        .bind(began)
        .execute(&pool)
        .await?;
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT clock_timestamp()>$1+INTERVAL '1 microsecond'")
            .bind(began)
            .fetch_one(&pool)
            .await?
    );
    blocker.commit().await?;
    let (status, result) = pending.await??;
    assert_eq!(status, StatusCode::CONFLICT, "{target:?}: {result}");
    Ok(())
}
#[sqlx::test(migrations = "../../migrations")]
async fn group_undo_expires_while_waiting_for_actor_lock(pool: PgPool) -> anyhow::Result<()> {
    expiry_after_lock_wait(pool, Target::Group).await
}
#[sqlx::test(migrations = "../../migrations")]
async fn assignment_undo_expires_while_waiting_for_actor_lock(pool: PgPool) -> anyhow::Result<()> {
    expiry_after_lock_wait(pool, Target::Assignment).await
}
#[sqlx::test(migrations = "../../migrations")]
async fn event_undo_expires_while_waiting_for_actor_lock(pool: PgPool) -> anyhow::Result<()> {
    expiry_after_lock_wait(pool, Target::Event).await
}
