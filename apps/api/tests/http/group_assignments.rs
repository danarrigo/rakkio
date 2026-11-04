use super::groups::{call, create, detail, fixture, invite, join};
use super::*;
use serde_json::{Value, json};
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
async fn shared_assignments_have_independent_private_progress_and_keep_completed_work(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, _, [owner, alice, bob, _]) = fixture(&pool).await?;
    let group = create(&router, owner).await?;
    for (user, handle) in [(alice, "alice"), (bob, "bob")] {
        let invitation = invite(&router, owner, &group, "direct", Some(handle), 1).await?;
        assert_eq!(join(&router, user, &invitation).await?.0, StatusCode::OK);
    }
    let path = format!(
        "/api/v1/groups/{}/assignments",
        group["id"].as_str().unwrap()
    );
    let (status,assignment)=call(&router,owner,"POST",&path,json!({"title":"Algorithms homework","description":"Shared definition, not personal copies","due_on":"2026-11-12","due_time":"10:00"})).await?;
    assert_eq!(status, StatusCode::OK, "{assignment}");
    assert_eq!(assignment["timezone"], "Asia/Jakarta");
    let id = assignment["id"].as_str().unwrap();
    let current = detail(&router, alice, &group).await?;
    let progress_path = format!("{path}/{id}/progress");
    let (status,_)=call(&router,alice,"POST",&progress_path,json!({"expected_membership_id":current["group"]["my_membership_id"],"expected_assignment_version":assignment["version"],"expected_progress_version":0,"completed":true})).await?;
    assert_eq!(status, StatusCode::OK);
    let alice_work = call(&router, alice, "GET", &path, Value::Null).await?.1;
    let bob_work = call(&router, bob, "GET", &path, Value::Null).await?.1;
    assert!(alice_work["items"][0]["my_progress"]["completed_at"].is_string());
    assert!(bob_work["items"][0]["my_progress"]["completed_at"].is_null());
    let report_path = format!("{path}/{id}/progress-report");
    assert_eq!(
        call(&router, bob, "GET", &report_path, Value::Null)
            .await?
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&router, owner, "GET", &report_path, Value::Null)
            .await?
            .1["items"],
        json!([])
    );
    let prefs_path = format!(
        "/api/v1/groups/{}/preferences",
        group["id"].as_str().unwrap()
    );
    let (status, prefs) = call(&router, alice, "GET", &prefs_path, Value::Null).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prefs["share_progress"], false);
    let (status,_)=call(&router,alice,"POST",&prefs_path,json!({"expected_membership_id":current["group"]["my_membership_id"],"expected_version":prefs["version"],"share_progress":true,"notice_version":"2026-11-04-progress"})).await?;
    assert_eq!(status, StatusCode::OK);
    let report = call(&router, owner, "GET", &report_path, Value::Null)
        .await?
        .1;
    assert_eq!(report["items"].as_array().unwrap().len(), 1);
    assert_eq!(report["items"][0]["handle"], "alice");
    assert!(report["items"][0]["completed_at"].is_string());
    let (status,_)=call(&router,owner,"PUT",&format!("{path}/{id}"),json!({"expected_user_id":owner,"expected_version":assignment["version"],"title":"Algorithms homework - updated","description":"Revised instructions","due_on":"2026-11-12","due_time":"10:00","state":"open"})).await?;
    assert_eq!(status, StatusCode::OK);
    let after = call(&router, alice, "GET", &path, Value::Null).await?.1;
    assert!(after["items"][0]["my_progress"]["completed_at"].is_string());
    assert_eq!(after["items"][0]["changed_since_completion"], true);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks")
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}

fn base(group: &Value) -> String {
    format!("/api/v1/groups/{}", group["id"].as_str().unwrap())
}
fn work(group: &Value) -> String {
    format!("{}/assignments", base(group))
}
fn item(group: &Value, a: &Value) -> String {
    format!("{}/{}", work(group), a["id"].as_str().unwrap())
}
fn edit(a: &Value) -> Value {
    json!({"expected_version":a["version"],"title":a["title"],"description":a["description"],"due_on":a["due_on"],"due_time":a["due_time"],"state":a["state"]})
}
async fn publish(router: &axum::Router, user: Uuid, g: &Value) -> anyhow::Result<Value> {
    let (status, a) = call(
        router,
        user,
        "POST",
        &work(g),
        json!({"title":"Shared assignment"}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{a}");
    Ok(a)
}
async fn enroll(
    router: &axum::Router,
    owner: Uuid,
    user: Uuid,
    g: &Value,
    handle: &str,
) -> anyhow::Result<()> {
    let i = invite(router, owner, g, "direct", Some(handle), 1).await?;
    let (s, b) = join(router, user, &i).await?;
    assert_eq!(s, StatusCode::OK, "{b}");
    Ok(())
}
async fn progress(
    router: &axum::Router,
    user: Uuid,
    g: &Value,
    a: &Value,
    completed: bool,
) -> anyhow::Result<(StatusCode, Value)> {
    let d = detail(router, user, g).await?;
    call(router,user,"POST",&format!("{}/progress",item(g,a)),json!({"expected_membership_id":d["group"]["my_membership_id"],"expected_assignment_version":a["version"],"expected_progress_version":a["my_progress"]["version"],"completed":completed})).await
}
async fn share(
    router: &axum::Router,
    user: Uuid,
    g: &Value,
    enabled: bool,
) -> anyhow::Result<(StatusCode, Value)> {
    let path = format!("{}/preferences", base(g));
    let p = call(router, user, "GET", &path, Value::Null).await?.1;
    call(router,user,"POST",&path,json!({"expected_membership_id":p["membership_id"],"expected_version":p["version"],"share_progress":enabled,"notice_version":"2026-11-04-progress"})).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn private_progress_and_consent_have_no_public_version_or_quota_signal_and_export_only_self(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, store, [owner, alice, bob, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    enroll(&r, owner, alice, &g, "alice").await?;
    enroll(&r, owner, bob, &g, "bob").await?;
    let a = publish(&r, owner, &g).await?;
    let before = detail(&r, owner, &g).await?;
    let gid: Uuid = g["id"].as_str().unwrap().parse()?;
    let epoch: i64 = sqlx::query_scalar("SELECT membership_epoch FROM groups WHERE id=$1")
        .bind(gid)
        .fetch_one(&pool)
        .await?;
    let usage = call(
        &r,
        owner,
        "GET",
        &format!("{}/content-usage", base(&g)),
        Value::Null,
    )
    .await?
    .1;
    assert_eq!(usage["used_records"], 4);
    assert_eq!(progress(&r, alice, &g, &a, true).await?.0, StatusCode::OK);
    let report = format!("{}/progress-report", item(&g, &a));
    assert_eq!(
        call(&r, owner, "GET", &report, Value::Null).await?.1["items"],
        json!([])
    );
    assert_eq!(share(&r, alice, &g, true).await?.0, StatusCode::OK);
    assert_eq!(
        call(&r, owner, "GET", &report, Value::Null).await?.1["items"][0]["handle"],
        "alice"
    );
    let personal: Value = serde_json::from_slice(&store.export_json(alice).await?)?;
    assert_eq!(
        personal["group_assignment_progress"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        personal["group_membership_notices"][0]["progress_notice_version"],
        "2026-11-04-progress"
    );
    let bob_export: Value = serde_json::from_slice(&store.export_json(bob).await?)?;
    assert!(
        bob_export["group_assignment_progress"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(share(&r, alice, &g, false).await?.0, StatusCode::OK);
    assert_eq!(
        call(&r, owner, "GET", &report, Value::Null).await?.1["items"],
        json!([])
    );
    assert_eq!(detail(&r, owner, &g).await?, before);
    assert_eq!(
        call(
            &r,
            owner,
            "GET",
            &format!("{}/content-usage", base(&g)),
            Value::Null
        )
        .await?
        .1,
        usage
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT membership_epoch FROM groups WHERE id=$1")
            .bind(gid)
            .fetch_one(&pool)
            .await?,
        epoch
    );
    let p = call(
        &r,
        alice,
        "GET",
        &format!("{}/preferences", base(&g)),
        Value::Null,
    )
    .await?
    .1;
    assert_eq!(p["version"], 3);
    assert!(p["shared_at"].is_null());
    assert!(p["notice_version"].is_null());
    let (s,_)=call(&r,alice,"POST",&format!("{}/preferences",base(&g)),json!({"expected_membership_id":p["membership_id"],"expected_version":p["version"],"share_progress":true,"notice_version":"unknown-future"})).await?;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn reserved_capacity_keeps_completion_closing_and_undo_available_but_rejects_new_work_and_members_atomically(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, alice, bob, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    enroll(&r, owner, alice, &g, "alice").await?;
    let a = publish(&r, owner, &g).await?;
    sqlx::query("UPDATE workspace_content_limits SET total_records=(SELECT records FROM workspace_content_total),total_bytes=(SELECT bytes FROM workspace_content_total),group_records=3").execute(&pool).await?;
    assert_eq!(progress(&r, alice, &g, &a, true).await?.0, StatusCode::OK);
    let (s, _) = call(
        &r,
        owner,
        "POST",
        &work(&g),
        json!({"title":"Cannot exceed capacity"}),
    )
    .await?;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let i = invite(&r, owner, &g, "direct", Some("bob"), 1).await?;
    assert_eq!(join(&r, bob, &i).await?.0, StatusCode::FORBIDDEN);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT uses::bigint FROM group_invitations WHERE id=$1")
            .bind(i["invitation"]["id"].as_str().unwrap().parse::<Uuid>()?)
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        call(&r, bob, "GET", &work(&g), Value::Null).await?.0,
        StatusCode::NOT_FOUND
    );
    let mut b = edit(&a);
    b["state"] = json!("closed");
    let (s, closed) = call(&r, owner, "PUT", &item(&g, &a), b).await?;
    assert_eq!(s, StatusCode::OK, "{closed}");
    assert_eq!(
        progress(&r, owner, &g, &closed, true).await?.0,
        StatusCode::OK
    );
    let (s, u) = call(
        &r,
        owner,
        "POST",
        &format!("{}/delete-with-undo", item(&g, &closed)),
        json!({"expected_version":closed["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "{u}");
    assert_eq!(
        call(&r, alice, "GET", &work(&g), Value::Null).await?.1["items"],
        json!([])
    );
    let (s, restored) = call(
        &r,
        owner,
        "POST",
        &format!("{}/restore", item(&g, &a)),
        json!({"expected_version":u["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "{restored}");
    assert!(restored["my_progress"]["completed_at"].is_string());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT records FROM workspace_content_total")
            .fetch_one(&pool)
            .await?,
        3
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn shared_definitions_reject_member_edits_cross_group_ids_wrong_actor_and_stale_writes(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, alice, bob, outsider]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    enroll(&r, owner, alice, &g, "alice").await?;
    let other = create(&r, bob).await?;
    let a = publish(&r, owner, &g).await?;
    assert_eq!(
        call(
            &r,
            alice,
            "POST",
            &work(&g),
            json!({"title":"Not a leader"})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&r, alice, "PUT", &item(&g, &a), edit(&a)).await?.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&r, outsider, "GET", &work(&g), Value::Null).await?.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&r, bob, "PUT", &item(&other, &a), edit(&a)).await?.0,
        StatusCode::NOT_FOUND
    );
    let mut b = edit(&a);
    b["expected_user_id"] = json!(alice);
    assert_eq!(
        call(&r, owner, "PUT", &item(&g, &a), b).await?.0,
        StatusCode::FORBIDDEN
    );
    let (s, updated) = call(&r, owner, "PUT", &item(&g, &a), edit(&a)).await?;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        call(&r, owner, "PUT", &item(&g, &a), edit(&a)).await?.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        progress(&r, alice, &g, &a, true).await?.0,
        StatusCode::CONFLICT
    );
    let (s, done) = progress(&r, owner, &g, &updated, true).await?;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        progress(&r, owner, &g, &updated, true).await?.0,
        StatusCode::CONFLICT
    );
    let (s, reopened) = progress(&r, owner, &g, &done, false).await?;
    assert_eq!(s, StatusCode::OK);
    assert!(reopened["my_progress"]["completed_at"].is_null());
    assert_eq!(
        call(
            &r,
            owner,
            "POST",
            &work(&g),
            json!({"title":"Forbidden unknown input","user_id":alice})
        )
        .await?
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn leaving_purges_progress_sharing_and_rejoining_requires_fresh_membership_identity(
    pool: PgPool,
) -> anyhow::Result<()> {
    use super::groups::action;
    let (r, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    enroll(&r, owner, alice, &g, "alice").await?;
    let a = publish(&r, owner, &g).await?;
    let old = detail(&r, alice, &g).await?;
    let (s, done) = progress(&r, alice, &g, &a, true).await?;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(share(&r, alice, &g, true).await?.0, StatusCode::OK);
    let (s, b) = action(
        &r,
        alice,
        &g,
        json!({"kind":"leave","expected_membership_version":old["group"]["my_membership_version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM group_assignment_progress")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        call(&r, alice, "GET", &work(&g), Value::Null).await?.0,
        StatusCode::NOT_FOUND
    );
    enroll(&r, owner, alice, &g, "alice").await?;
    let now = detail(&r, alice, &g).await?;
    assert_ne!(
        now["group"]["my_membership_id"],
        old["group"]["my_membership_id"]
    );
    let (s,_)=call(&r,alice,"POST",&format!("{}/progress",item(&g,&a)),json!({"expected_membership_id":old["group"]["my_membership_id"],"expected_assignment_version":a["version"],"expected_progress_version":done["my_progress"]["version"],"completed":true})).await?;
    assert_eq!(s, StatusCode::CONFLICT);
    let after = call(&r, alice, "GET", &work(&g), Value::Null).await?.1;
    assert_eq!(after["items"][0]["my_progress"]["version"], 0);
    assert_eq!(
        call(
            &r,
            alice,
            "GET",
            &format!("{}/preferences", base(&g)),
            Value::Null
        )
        .await?
        .1["share_progress"],
        false
    );
    assert_eq!(
        call(
            &r,
            owner,
            "GET",
            &format!("{}/progress-report", item(&g, &a)),
            Value::Null
        )
        .await?
        .1["items"],
        json!([])
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn shared_assignment_deadlines_snapshot_timezone_and_reject_nonexistent_clock_times(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, _, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    let (s, a) = call(
        &r,
        owner,
        "POST",
        &work(&g),
        json!({"title":"Date only","due_on":"2026-11-12"}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!(a["due_time"], "23:59:00");
    assert_eq!(a["due_at"], "2026-11-12T16:59:00Z");
    sqlx::query("UPDATE groups SET timezone='America/New_York' WHERE id=$1")
        .bind(g["id"].as_str().unwrap().parse::<Uuid>()?)
        .execute(&pool)
        .await?;
    let (_, b) = call(&r, owner, "PUT", &item(&g, &a), edit(&a)).await?;
    assert_eq!(b["timezone"], "Asia/Jakarta");
    assert_eq!(b["due_at"], a["due_at"]);
    for body in [
        json!({"title":"Gap","due_on":"2027-03-14","due_time":"02:30"}),
        json!({"title":"No date","due_time":"10:00"}),
        json!({"title":"Invalid","due_on":"2026-11-31"}),
        json!({"title":"Bad\nline"}),
    ] {
        let (s, b) = call(&r, owner, "POST", &work(&g), body).await?;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
    }
    let (s, b) = call(
        &r,
        owner,
        "POST",
        &work(&g),
        json!({"title":"Fold uses standard time","due_on":"2026-11-01","due_time":"01:30"}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["due_at"], "2026-11-01T06:30:00Z");
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_content_reservations_serialize_at_global_limit_and_full_group_purge_releases_every_slot(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, _, bob, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    let h = create(&r, bob).await?;
    sqlx::query("UPDATE workspace_content_limits SET total_records=3")
        .execute(&pool)
        .await?;
    let gp = work(&g);
    let hp = work(&h);
    let (a, b) = tokio::join!(
        call(&r, owner, "POST", &gp, json!({"title":"One"})),
        call(&r, bob, "POST", &hp, json!({"title":"Two"}))
    );
    let (a, b) = (a?, b?);
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::FORBIDDEN]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT records FROM workspace_content_total")
            .fetch_one(&pool)
            .await?,
        2
    );
    let winning = if a.0 == StatusCode::OK { &g } else { &h };
    let gid: Uuid = winning["id"].as_str().unwrap().parse()?;
    // Simulate the bounded purge after an explicitly approved group deletion.
    sqlx::query("DELETE FROM groups WHERE id=$1")
        .bind(gid)
        .execute(&pool)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT records FROM workspace_content_total")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT bytes FROM workspace_content_total")
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn streaming_account_export_rechecks_membership_before_delivering_shared_definitions(
    pool: PgPool,
) -> anyhow::Result<()> {
    use super::groups::{action, member};
    use futures_util::TryStreamExt;
    let (r, store, [owner, alice, bob, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    enroll(&r, owner, alice, &g, "alice").await?;
    let a = publish(&r, owner, &g).await?;
    assert_eq!(progress(&r, alice, &g, &a, true).await?.0, StatusCode::OK);
    let own: Value = serde_json::from_slice(&store.export_json(alice).await?)?;
    assert_eq!(own["group_assignments"][0]["assignment"]["id"], a["id"]);
    assert!(own["group_assignments"][0]["my_progress"]["completed_at"].is_string());
    let outsider: Value = serde_json::from_slice(&store.export_json(bob).await?)?;
    assert_eq!(outsider["group_assignments"], json!([]));
    let stream = store.export_json_stream(alice).await?;
    futures_util::pin_mut!(stream);
    let mut bytes = stream.try_next().await?.unwrap().to_vec();
    let d = detail(&r, owner, &g).await?;
    let m = member(&d, "alice");
    let (s, b) = action(
        &r,
        owner,
        &g,
        json!({"kind":"remove","membership_id":m["id"],"expected_membership_version":m["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "{b}");
    while let Some(chunk) = stream.try_next().await? {
        bytes.extend_from_slice(&chunk);
    }
    let exported: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(exported["group_assignments"], json!([]));
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn streaming_download_holds_account_admission_until_body_is_consumed_or_dropped(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, _, _, _]) = fixture(&pool).await?;
    let request = || {
        Request::builder()
            .uri("/api/v1/exports/json")
            .header(DEVELOPMENT_USER_HEADER, owner.to_string())
            .body(Body::empty())
            .unwrap()
    };
    let first = r.clone().oneshot(request()).await?;
    assert_eq!(first.status(), StatusCode::OK);
    let busy = r.clone().oneshot(request()).await?;
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(busy);
    drop(first);
    let available = r.clone().oneshot(request()).await?;
    assert_eq!(available.status(), StatusCode::OK);
    let bytes = to_bytes(available.into_body(), 1024 * 1024).await?;
    let json: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(json["group_assignments"], json!([]));
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn mixed_personal_and_group_content_account_for_final_transaction_net_without_transient_quota_failure(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    enroll(&r, owner, alice, &g, "alice").await?;
    let a = publish(&r, owner, &g).await?;
    sqlx::query("UPDATE workspace_content_limits SET total_records=3")
        .execute(&pool)
        .await?;
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO projects(id,user_id,name) VALUES($1,$2,'Personal')")
        .bind(Uuid::now_v7())
        .bind(owner)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM group_assignments WHERE id=$1")
        .bind(a["id"].as_str().unwrap().parse::<Uuid>()?)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT records FROM workspace_content_total")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT records FROM group_content_usage")
            .fetch_one(&pool)
            .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT records FROM workspace_content_usage WHERE user_id=$1"
        )
        .bind(owner)
        .fetch_one(&pool)
        .await?,
        1
    );
    Ok(())
}
