use super::groups::{call, create, fixture, invite, join};
use super::*;
use serde_json::{Value, json};

fn path(g: &Value) -> String {
    format!("/api/v1/groups/{}/events", g["id"].as_str().unwrap())
}
fn event_body() -> Value {
    json!({"title":"Algorithms lecture","description":"Shared class event","location":"Room 12","starts_local":"2027-03-07T09:00:00","ends_local":"2027-03-07T10:00:00","all_day":false,"timezone":"America/New_York","recurrence":"weekly","recurrence_until":"2027-03-31"})
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
    update["title"] = json!("Updated lecture");
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
