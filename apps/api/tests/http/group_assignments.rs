use super::groups::{call, create, detail, fixture, invite, join};
use super::*;
use serde_json::{Value, json};

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
