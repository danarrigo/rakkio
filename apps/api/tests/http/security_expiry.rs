use super::groups::{action, call, create, detail, fixture, invite, join, member};
use super::*;
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

async fn waiting_start(pool: &PgPool, pid: i32) -> anyhow::Result<DateTime<Utc>> {
    Ok(tokio::time::timeout(std::time::Duration::from_secs(5),async {loop {
  let began:Option<DateTime<Utc>>=sqlx::query_scalar("SELECT xact_start FROM pg_stat_activity WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)) LIMIT 1").bind(pid).fetch_optional(pool).await?;
  if let Some(t)=began {break Ok::<_,sqlx::Error>(t)}
  tokio::time::sleep(std::time::Duration::from_millis(10)).await;
 }}).await??)
}
async fn lock_actor(
    pool: &PgPool,
    user: Uuid,
) -> anyhow::Result<(sqlx::Transaction<'static, sqlx::Postgres>, i32)> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(user)
        .execute(&mut *tx)
        .await?;
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await?;
    Ok((tx, pid))
}

#[sqlx::test(migrations = "../../migrations")]
async fn expired_invitation_cannot_admit_after_actor_lock_wait(pool: PgPool) -> anyhow::Result<()> {
    let (router, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let g = create(&router, owner).await?;
    let invitation = invite(&router, owner, &g, "direct", Some("alice"), 1).await?;
    let (blocker, pid) = lock_actor(&pool, alice).await?;
    let r = router.clone();
    let reference = invitation.clone();
    let pending = tokio::spawn(async move { join(&r, alice, &reference).await });
    let began = waiting_start(&pool, pid).await?;
    sqlx::query("UPDATE group_invitations SET expires_at=$2+INTERVAL '1 microsecond' WHERE id=$1")
        .bind(
            invitation["invitation"]["id"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()?,
        )
        .bind(began)
        .execute(&pool)
        .await?;
    blocker.commit().await?;
    let (status, value) = pending.await??;
    assert_eq!(status, StatusCode::NOT_FOUND, "{value}");
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn expired_ownership_offer_cannot_transfer_after_actor_lock_wait(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let g = create(&router, owner).await?;
    let invitation = invite(&router, owner, &g, "direct", Some("alice"), 1).await?;
    assert_eq!(join(&router, alice, &invitation).await?.0, StatusCode::OK);
    let view = detail(&router, owner, &g).await?;
    let m = member(&view, "alice");
    assert_eq!(
        action(
            &router,
            owner,
            &g,
            json!({"kind":"offer_ownership","membership_id":m["id"]})
        )
        .await?
        .0,
        StatusCode::OK
    );
    let view = detail(&router, alice, &g).await?;
    let offer = view["ownership_offer"]["id"].clone();
    let (blocker, pid) = lock_actor(&pool, alice).await?;
    let r = router.clone();
    let group = g.clone();
    let offer_id = offer.clone();
    let pending = tokio::spawn(async move {
        action(
            &r,
            alice,
            &group,
            json!({"kind":"accept_ownership","offer_id":offer_id}),
        )
        .await
    });
    let began = waiting_start(&pool, pid).await?;
    sqlx::query(
        "UPDATE group_ownership_offers SET expires_at=$2+INTERVAL '1 microsecond' WHERE id=$1",
    )
    .bind(offer.as_str().unwrap().parse::<Uuid>()?)
    .bind(began)
    .execute(&pool)
    .await?;
    blocker.commit().await?;
    let (status, value) = pending.await??;
    assert_eq!(status, StatusCode::NOT_FOUND, "{value}");
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn expired_invitation_cannot_be_approved_after_actor_lock_wait(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let group = create(&router, owner).await?;
    let invitation = invite(&router, owner, &group, "link", None, 1).await?;
    assert_eq!(
        join(&router, alice, &invitation).await?.1["status"],
        "pending"
    );
    let view = detail(&router, owner, &group).await?;
    let request = view["join_requests"][0].clone();
    let (blocker, pid) = lock_actor(&pool, owner).await?;
    let r = router.clone();
    let g = group.clone();
    let pending = tokio::spawn(async move {
        action(&r,owner,&g,json!({"kind":"review_join","request_id":request["id"],"expected_request_version":request["version"],"approve":true})).await
    });
    let began = waiting_start(&pool, pid).await?;
    sqlx::query("UPDATE group_invitations SET expires_at=$2+INTERVAL '1 microsecond' WHERE id=$1")
        .bind(
            invitation["invitation"]["id"]
                .as_str()
                .unwrap()
                .parse::<Uuid>()?,
        )
        .bind(began)
        .execute(&pool)
        .await?;
    blocker.commit().await?;
    let (status, value) = pending.await??;
    assert_eq!(status, StatusCode::CONFLICT, "{value}");
    let members: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM group_memberships WHERE group_id=$1 AND user_id=$2",
    )
    .bind(group["id"].as_str().unwrap().parse::<Uuid>()?)
    .bind(alice)
    .fetch_one(&pool)
    .await?;
    assert_eq!(members, 0);
    Ok(())
}

#[derive(Clone, Copy)]
enum Recovery {
    Group,
    Assignment,
}
async fn recovery_expiry(pool: PgPool, target: Recovery) -> anyhow::Result<()> {
    let (router, _, [owner, _, _, _]) = fixture(&pool).await?;
    let g = create(&router, owner).await?;
    let root = format!("/api/v1/groups/{}", g["id"].as_str().unwrap());
    let (id, path, body) = match target {
        Recovery::Group => {
            let (s, v) = action(
                &router,
                owner,
                &g,
                json!({"kind":"delete","confirmation":"DELETE GROUP"}),
            )
            .await?;
            assert_eq!(s, StatusCode::OK);
            (
                g["id"].clone(),
                format!("{root}/actions"),
                json!({"expected_version":v["group"]["version"],"action":{"kind":"restore"}}),
            )
        }
        Recovery::Assignment => {
            let (s, e) = call(
                &router,
                owner,
                "POST",
                &format!("{root}/assignments"),
                json!({"title":"Recovery clock"}),
            )
            .await?;
            assert_eq!(s, StatusCode::OK);
            let p = format!("{root}/assignments/{}", e["id"].as_str().unwrap());
            let (s, v) = call(
                &router,
                owner,
                "POST",
                &format!("{p}/delete-with-undo"),
                json!({"expected_version":e["version"]}),
            )
            .await?;
            assert_eq!(s, StatusCode::OK);
            (
                e["id"].clone(),
                format!("{p}/restore"),
                json!({"expected_version":v["version"]}),
            )
        }
    };
    let (blocker, pid) = lock_actor(&pool, owner).await?;
    let r = router.clone();
    let pending = tokio::spawn(async move { call(&r, owner, "POST", &path, body).await });
    let began = waiting_start(&pool, pid).await?;
    let query = match target {
        Recovery::Group => {
            "UPDATE groups SET deleted_at=$2-INTERVAL '1 second',undo_until=$2+INTERVAL '1 microsecond' WHERE id=$1"
        }
        Recovery::Assignment => {
            "UPDATE group_assignments SET deleted_at=$2-INTERVAL '1 second',undo_until=$2+INTERVAL '1 microsecond' WHERE id=$1"
        }
    };
    sqlx::query(query)
        .bind(id.as_str().unwrap().parse::<Uuid>()?)
        .bind(began)
        .execute(&pool)
        .await?;
    blocker.commit().await?;
    assert_eq!(pending.await??.0, StatusCode::CONFLICT);
    Ok(())
}
#[sqlx::test(migrations = "../../migrations")]
async fn group_recovery_cannot_outlive_wall_clock_deadline(pool: PgPool) -> anyhow::Result<()> {
    recovery_expiry(pool, Recovery::Group).await
}
#[sqlx::test(migrations = "../../migrations")]
async fn assignment_recovery_cannot_outlive_wall_clock_deadline(
    pool: PgPool,
) -> anyhow::Result<()> {
    recovery_expiry(pool, Recovery::Assignment).await
}
