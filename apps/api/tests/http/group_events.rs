use super::groups::{call, create, fixture, invite, join};
use super::*;
use serde_json::{Value, json};

fn path(g: &Value) -> String {
    format!("/api/v1/groups/{}/events", g["id"].as_str().unwrap())
}
fn event_body() -> Value {
    json!({"event":{"title":"Algorithms lecture","description":"Shared class event","location":"Room 12","starts_local":"2027-03-07T09:00:00","ends_local":"2027-03-07T10:00:00","all_day":false,"timezone":"America/New_York","recurrence":"weekly","recurrence_until":"2027-03-31"}})
}

#[sqlx::test(migrations = "../../migrations")]
async fn shared_event_series_is_read_only_for_members_and_preserves_wall_time_through_dst(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, alice, _, outsider]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    let i = invite(&r, owner, &g, "direct", Some("alice"), 1).await?;
    assert_eq!(join(&r, alice, &i).await?.0, StatusCode::OK);
    let (s, e) = call(&r, owner, "POST", &path(&g), event_body()).await?;
    assert_eq!(s, StatusCode::OK, "{e}");
    assert_eq!(
        call(&r, alice, "POST", &path(&g), event_body()).await?.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&r, outsider, "GET", &path(&g), Value::Null).await?.0,
        StatusCode::NOT_FOUND
    );
    let occurrences = format!(
        "/api/v1/groups/{}/event-occurrences?from=2027-03-13&until=2027-03-16&timezone=America%2FNew_York",
        g["id"].as_str().unwrap()
    );
    let (s, page) = call(&r, alice, "GET", &occurrences, Value::Null).await?;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let item = &page["items"][0];
    assert_eq!(item["origin"], "group_event");
    assert_eq!(item["group_id"], g["id"]);
    assert_eq!(item["group_name"], g["name"]);
    assert_eq!(item["can_edit"], false);
    assert_eq!(item["starts_at"], "2027-03-14T13:00:00Z");
    assert_eq!(item["ends_at"], "2027-03-14T14:00:00Z");
    let endpoint = format!("{}/{}", path(&g), e["id"].as_str().unwrap());
    let mut update = event_body();
    update["expected_version"] = e["version"].clone();
    update["scope"] = json!("series");
    update["event"]["title"] = json!("Updated lecture");
    assert_eq!(
        call(&r, alice, "PUT", &endpoint, update.clone()).await?.0,
        StatusCode::FORBIDDEN
    );
    let (s, edited) = call(&r, owner, "PUT", &endpoint, update.clone()).await?;
    assert_eq!(s, StatusCode::OK, "{edited}");
    assert_eq!(
        call(&r, owner, "PUT", &endpoint, update).await?.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&r, alice, "GET", &occurrences, Value::Null).await?.1["items"][0]["title"],
        "Updated lecture"
    );
    let (s, receipt) = call(
        &r,
        owner,
        "POST",
        &format!("{endpoint}/delete-with-undo"),
        json!({"scope":"series","expected_version":edited["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "{receipt}");
    assert_eq!(
        call(&r, alice, "GET", &occurrences, Value::Null).await?.1["items"],
        json!([])
    );
    let (s, _) = call(
        &r,
        owner,
        "POST",
        &format!("{endpoint}/restore"),
        json!({"scope":"series","expected_version":receipt["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        call(&r, alice, "GET", &occurrences, Value::Null).await?.1["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM calendar_events")
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}

fn fields(start: &str, end: &str, recurrence: &str, all_day: bool) -> Value {
    json!({"event":{"title":"Shared event","description":"Group instructions","location":"Room 1","starts_local":start,"ends_local":end,"all_day":all_day,"timezone":"America/New_York","recurrence":recurrence,"recurrence_until":null}})
}
async fn publish(
    r: &axum::Router,
    user: uuid::Uuid,
    g: &Value,
    body: Value,
) -> anyhow::Result<Value> {
    let (s, e) = call(r, user, "POST", &path(g), body).await?;
    assert_eq!(s, StatusCode::OK, "{e}");
    Ok(e)
}
async fn occurrences(
    r: &axum::Router,
    user: uuid::Uuid,
    g: &Value,
    from: &str,
    until: &str,
    zone: &str,
) -> anyhow::Result<Value> {
    let endpoint = format!(
        "/api/v1/groups/{}/event-occurrences?from={from}&until={until}&timezone={zone}",
        g["id"].as_str().unwrap()
    );
    let (s, p) = call(r, user, "GET", &endpoint, Value::Null).await?;
    assert_eq!(s, StatusCode::OK, "{p}");
    Ok(p)
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_event_all_day_dates_are_floating_exclusive_and_monthly_spans_do_not_clamp_end_dates(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, _, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    publish(
        &r,
        owner,
        &g,
        fields(
            "2027-01-31T00:00:00",
            "2027-02-02T00:00:00",
            "monthly",
            true,
        ),
    )
    .await?;
    let feb = occurrences(
        &r,
        owner,
        &g,
        "2027-02-01",
        "2027-02-03",
        "Pacific%2FKiritimati",
    )
    .await?;
    assert_eq!(feb["items"].as_array().unwrap().len(), 1);
    assert_eq!(feb["items"][0]["starts_at"], "2027-01-31T00:00:00Z");
    assert_eq!(feb["items"][0]["ends_at"], "2027-02-02T00:00:00Z");
    assert_eq!(
        occurrences(
            &r,
            owner,
            &g,
            "2027-02-02",
            "2027-03-02",
            "Pacific%2FKiritimati"
        )
        .await?["items"],
        json!([])
    );
    let march = occurrences(&r, owner, &g, "2027-03-31", "2027-04-03", "UTC").await?;
    assert_eq!(march["items"][0]["starts_at"], "2027-03-31T00:00:00Z");
    assert_eq!(march["items"][0]["ends_at"], "2027-04-02T00:00:00Z");
    publish(
        &r,
        owner,
        &g,
        fields(
            "2024-02-29T09:00:00",
            "2024-03-01T09:00:00",
            "yearly",
            false,
        ),
    )
    .await?;
    assert_eq!(
        occurrences(&r, owner, &g, "2025-02-28", "2025-03-02", "UTC").await?["items"],
        json!([])
    );
    let leap = occurrences(&r, owner, &g, "2028-02-29", "2028-03-02", "UTC").await?;
    assert_eq!(leap["items"].as_array().unwrap().len(), 1);
    assert_eq!(leap["items"][0]["original_date"], "2028-02-29");
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_event_daily_gaps_short_and_long_days_and_overnight_overlap_use_civil_time(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, _, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    let mut daily = fields("2027-03-13T02:30:00", "2027-03-13T03:30:00", "daily", false);
    daily["event"]["recurrence_until"] = json!("2027-03-15");
    publish(&r, owner, &g, daily).await?;
    let gap = occurrences(
        &r,
        owner,
        &g,
        "2027-03-13",
        "2027-03-16",
        "America%2FNew_York",
    )
    .await?;
    assert_eq!(gap["items"].as_array().unwrap().len(), 2);
    assert_eq!(gap["items"][0]["original_date"], "2027-03-13");
    assert_eq!(gap["items"][1]["original_date"], "2027-03-15");
    assert_eq!(
        occurrences(&r, owner, &g, "2027-03-16", "2027-03-17", "UTC").await?["items"],
        json!([])
    );
    let short = publish(
        &r,
        owner,
        &g,
        fields("2027-03-14T00:00:00", "2027-03-15T00:00:00", "none", false),
    )
    .await?;
    let p = occurrences(
        &r,
        owner,
        &g,
        "2027-03-14",
        "2027-03-15",
        "America%2FNew_York",
    )
    .await?;
    let row = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["event_id"] == short["id"])
        .unwrap();
    assert_eq!(row["starts_at"], "2027-03-14T05:00:00Z");
    assert_eq!(row["ends_at"], "2027-03-15T04:00:00Z");
    let long = publish(
        &r,
        owner,
        &g,
        fields("2027-11-07T00:00:00", "2027-11-08T00:00:00", "none", false),
    )
    .await?;
    let p = occurrences(
        &r,
        owner,
        &g,
        "2027-11-07",
        "2027-11-08",
        "America%2FNew_York",
    )
    .await?;
    let row = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["event_id"] == long["id"])
        .unwrap();
    assert_eq!(row["starts_at"], "2027-11-07T04:00:00Z");
    assert_eq!(row["ends_at"], "2027-11-08T05:00:00Z");
    let overnight = publish(
        &r,
        owner,
        &g,
        fields("2027-11-06T23:30:00", "2027-11-07T02:30:00", "none", false),
    )
    .await?;
    let p = occurrences(
        &r,
        owner,
        &g,
        "2027-11-07",
        "2027-11-08",
        "America%2FNew_York",
    )
    .await?;
    let row = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["event_id"] == overnight["id"])
        .unwrap();
    assert_eq!(row["starts_at"], "2027-11-07T03:30:00Z");
    assert_eq!(row["ends_at"], "2027-11-07T07:30:00Z");
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_event_validation_requires_explicit_series_scope_and_bounded_windows(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    for body in [
        fields("2027-03-14T02:30:00", "2027-03-14T03:30:00", "none", false),
        fields("2027-03-14T01:30:00", "2027-03-14T02:30:00", "none", false),
        fields("2027-01-01T01:00:00", "2027-01-02T00:00:00", "none", true),
        fields("2027-01-01T00:00:00", "2027-03-02T00:00:00", "none", false),
    ] {
        let (s, e) = call(&r, owner, "POST", &path(&g), body).await?;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    }
    let e = publish(&r, owner, &g, event_body()).await?;
    let endpoint = format!("{}/{}", path(&g), e["id"].as_str().unwrap());
    let mut body = event_body();
    body["expected_version"] = e["version"].clone();
    assert_eq!(
        call(&r, owner, "PUT", &endpoint, body.clone()).await?.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    body["scope"] = json!("occurrence");
    assert_eq!(
        call(&r, owner, "PUT", &endpoint, body.clone()).await?.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    body["scope"] = json!("series");
    body["expected_user_id"] = json!(alice);
    assert_eq!(
        call(&r, owner, "PUT", &endpoint, body).await?.0,
        StatusCode::FORBIDDEN
    );
    for suffix in [
        "from=2027-01-01&until=2027-04-01&timezone=UTC",
        "from=2027-01-01&until=2027-01-02&timezone=Invalid%2FZone",
        "from=2027-01-02&until=2027-01-01&timezone=UTC",
    ] {
        let (s, e) = call(
            &r,
            owner,
            "GET",
            &format!(
                "/api/v1/groups/{}/event-occurrences?{suffix}",
                g["id"].as_str().unwrap()
            ),
            Value::Null,
        )
        .await?;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    }
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_events_bill_once_not_per_member_and_purge_releases_quota_with_full_budget_undo(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    let e = publish(&r, owner, &g, event_body()).await?;
    sqlx::query("UPDATE workspace_content_limits SET total_records=1,total_bytes=(SELECT bytes FROM workspace_content_total)").execute(&pool).await?;
    let i = invite(&r, owner, &g, "direct", Some("alice"), 1).await?;
    assert_eq!(join(&r, alice, &i).await?.0, StatusCode::OK);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT records FROM workspace_content_total")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        call(&r, owner, "POST", &path(&g), event_body()).await?.0,
        StatusCode::FORBIDDEN
    );
    let endpoint = format!("{}/{}", path(&g), e["id"].as_str().unwrap());
    let (s, u) = call(
        &r,
        owner,
        "POST",
        &format!("{endpoint}/delete-with-undo"),
        json!({"scope":"series","expected_version":e["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK, "{u}");
    assert_eq!(
        call(
            &r,
            owner,
            "POST",
            &format!("{endpoint}/restore"),
            json!({"scope":"series","expected_version":u["version"]})
        )
        .await?
        .0,
        StatusCode::OK
    );
    sqlx::query("DELETE FROM groups WHERE id=$1")
        .bind(g["id"].as_str().unwrap().parse::<uuid::Uuid>()?)
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
async fn group_event_occurrence_pages_keep_every_overlapping_instance_and_expose_empty_page_continuations(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, _, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    for _ in 0..3 {
        publish(
            &r,
            owner,
            &g,
            fields("1990-01-01T00:00:00", "1990-01-31T00:00:00", "daily", false),
        )
        .await?;
    }
    publish(
        &r,
        owner,
        &g,
        fields(
            "2024-02-29T09:00:00",
            "2024-02-29T10:00:00",
            "yearly",
            false,
        ),
    )
    .await?;
    let base = format!(
        "/api/v1/groups/{}/event-occurrences?from=2027-03-01&until=2027-03-02&timezone=America%2FNew_York&limit=1",
        g["id"].as_str().unwrap()
    );
    let (s, first) = call(&r, owner, "GET", &base, Value::Null).await?;
    assert_eq!(s, StatusCode::OK, "{first}");
    assert_eq!(first["items"], json!([]));
    assert!(first["next_cursor"].is_string());
    let mut cursor = first["next_cursor"].as_str().map(str::to_owned);
    let mut keys = std::collections::HashSet::new();
    for _ in 0..4 {
        let Some(current) = cursor.take() else {
            break;
        };
        let (s, p) = call(
            &r,
            owner,
            "GET",
            &format!("{base}&cursor={current}"),
            Value::Null,
        )
        .await?;
        assert_eq!(s, StatusCode::OK, "{p}");
        assert_eq!(p["items"].as_array().unwrap().len(), 30);
        for e in p["items"].as_array().unwrap() {
            assert_eq!(e["can_edit"], true);
            assert!(keys.insert(format!("{}:{}", e["event_id"], e["original_date"])));
        }
        cursor = p["next_cursor"].as_str().map(str::to_owned);
    }
    assert!(cursor.is_none());
    assert_eq!(keys.len(), 90);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_event_undo_rechecks_public_membership_changes_but_not_private_progress_preferences(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    let e = publish(&r, owner, &g, event_body()).await?;
    let endpoint = format!("{}/{}", path(&g), e["id"].as_str().unwrap());
    let (s, u) = call(
        &r,
        owner,
        "POST",
        &format!("{endpoint}/delete-with-undo"),
        json!({"scope":"series","expected_version":e["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK);
    let (s,_)=call(&r,owner,"POST",&format!("/api/v1/groups/{}/preferences",g["id"].as_str().unwrap()),json!({"expected_membership_id":g["my_membership_id"],"expected_version":1,"share_progress":true,"notice_version":"2026-11-04-progress"})).await?;
    assert_eq!(s, StatusCode::OK);
    let (s, restored) = call(
        &r,
        owner,
        "POST",
        &format!("{endpoint}/restore"),
        json!({"scope":"series","expected_version":u["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK);
    let (s, u) = call(
        &r,
        owner,
        "POST",
        &format!("{endpoint}/delete-with-undo"),
        json!({"scope":"series","expected_version":restored["version"]}),
    )
    .await?;
    assert_eq!(s, StatusCode::OK);
    let i = invite(&r, owner, &g, "direct", Some("alice"), 1).await?;
    assert_eq!(join(&r, alice, &i).await?.0, StatusCode::OK);
    assert_eq!(
        call(
            &r,
            owner,
            "POST",
            &format!("{endpoint}/restore"),
            json!({"scope":"series","expected_version":u["version"]})
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn leader_created_events_survive_creator_departure_and_uncommitted_group_purge_clears_all_billing(
    pool: PgPool,
) -> anyhow::Result<()> {
    use super::groups::{action, detail, member};
    let (r, _, [owner, alice, _, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    let i = invite(&r, owner, &g, "direct", Some("alice"), 1).await?;
    assert_eq!(join(&r, alice, &i).await?.0, StatusCode::OK);
    let view = detail(&r, owner, &g).await?;
    let m = member(&view, "alice");
    assert_eq!(action(&r,owner,&g,json!({"kind":"set_role","membership_id":m["id"],"expected_membership_version":m["version"],"role":"leader"})).await?.0,StatusCode::OK);
    let e = publish(&r, alice, &g, event_body()).await?;
    let view = detail(&r, owner, &g).await?;
    let m = member(&view, "alice");
    assert_eq!(action(&r,owner,&g,json!({"kind":"remove","membership_id":m["id"],"expected_membership_version":m["version"]})).await?.0,StatusCode::OK);
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(alice)
        .execute(&pool)
        .await?;
    assert_eq!(
        call(
            &r,
            owner,
            "GET",
            &format!("{}/{}", path(&g), e["id"].as_str().unwrap()),
            Value::Null
        )
        .await?
        .0,
        StatusCode::OK
    );
    let mut tx = pool.begin().await?;
    let gid = g["id"].as_str().unwrap().parse::<uuid::Uuid>()?;
    sqlx::query("INSERT INTO group_events(id,group_id,title,starts_local,ends_local,timezone) VALUES($1,$2,'Uncommitted event','2027-03-01T09:00:00','2027-03-01T10:00:00','UTC')").bind(uuid::Uuid::now_v7()).bind(gid).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM groups WHERE id=$1")
        .bind(gid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
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
async fn group_event_exports_are_scoped_and_personal_id_collisions_do_not_cross_ownership(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (r, store, [owner, alice, bob, _]) = fixture(&pool).await?;
    let g = create(&r, owner).await?;
    let h = create(&r, bob).await?;
    let i = invite(&r, owner, &g, "direct", Some("alice"), 1).await?;
    assert_eq!(join(&r, alice, &i).await?.0, StatusCode::OK);
    let e = publish(&r, owner, &g, event_body()).await?;
    let id = e["id"].as_str().unwrap().parse::<uuid::Uuid>()?;
    let calendar = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO calendars(id,user_id,name) VALUES($1,$2,'Private')")
        .bind(calendar)
        .bind(owner)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO calendar_events(id,user_id,calendar_id,title,starts_at,ends_at) VALUES($1,$2,$3,'Personal secret meeting','2027-03-14T13:00:00Z','2027-03-14T14:00:00Z')").bind(id).bind(owner).bind(calendar).execute(&pool).await?;
    assert_eq!(
        call(&r, bob, "GET", &format!("{}/{id}", path(&h)), Value::Null)
            .await?
            .0,
        StatusCode::NOT_FOUND
    );
    let own: Value = serde_json::from_slice(&store.export_json(alice).await?)?;
    assert_eq!(own["group_events"].as_array().unwrap().len(), 1);
    assert_eq!(
        own["group_events"][0]["event"]["title"],
        "Algorithms lecture"
    );
    assert!(!own.to_string().contains("Personal secret meeting"));
    let other: Value = serde_json::from_slice(&store.export_json(bob).await?)?;
    assert_eq!(other["group_events"], json!([]));
    let p = occurrences(&r, alice, &g, "2027-03-14", "2027-03-15", "UTC").await?;
    assert_eq!(p["items"].as_array().unwrap().len(), 1);
    assert_eq!(p["items"][0]["origin"], "group_event");
    assert_eq!(p["items"][0]["event_id"], e["id"]);
    assert!(!p.to_string().contains("Personal secret"));
    Ok(())
}
