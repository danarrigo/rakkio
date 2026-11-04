use super::*;
use serde_json::{Value, json};
use uuid::Uuid;

async fn call(
    router: &axum::Router,
    user: Uuid,
    method: &str,
    path: &str,
    mut body: Value,
) -> anyhow::Result<(StatusCode, Value)> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(DEVELOPMENT_USER_HEADER, user.to_string());
    let data = if method == "GET" {
        Body::empty()
    } else {
        if path != "/api/v1/groups/join/preview" && method != "DELETE" {
            body.as_object_mut()
                .unwrap()
                .entry("expected_user_id")
                .or_insert(json!(user));
        }
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    };
    let response = router.clone().oneshot(request.body(data)?).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok((
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        },
    ))
}
async fn fixture(pool: &PgPool) -> anyhow::Result<(axum::Router, Store, [Uuid; 4])> {
    let store = Store::from_pool(pool.clone());
    let users = [
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    ];
    for (id, name) in users
        .into_iter()
        .zip(["lecturer", "alice", "bob", "visitor"])
    {
        store.ensure_development_user(id).await?;
        store.claim_handle(id, name).await?;
    }
    let mut config = test_config();
    config.groups_enabled = true;
    Ok((app::build(&config, store.clone())?, store, users))
}
async fn create(router: &axum::Router, user: Uuid) -> anyhow::Result<Value> {
    let (status, group) = call(
        router,
        user,
        "POST",
        "/api/v1/groups",
        json!({"name":"College class","timezone":"Asia/Jakarta"}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{group}");
    Ok(group)
}
async fn detail(router: &axum::Router, user: Uuid, group: &Value) -> anyhow::Result<Value> {
    let (status, body) = call(
        router,
        user,
        "GET",
        &format!("/api/v1/groups/{}", group["id"].as_str().unwrap()),
        Value::Null,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    Ok(body)
}
async fn action(
    router: &axum::Router,
    user: Uuid,
    group: &Value,
    action: Value,
) -> anyhow::Result<(StatusCode, Value)> {
    call(
        router,
        user,
        "POST",
        &format!("/api/v1/groups/{}/actions", group["id"].as_str().unwrap()),
        json!({"expected_version":group["version"],"action":action}),
    )
    .await
}
async fn invite(
    router: &axum::Router,
    user: Uuid,
    group: &Value,
    kind: &str,
    handle: Option<&str>,
    uses: i32,
) -> anyhow::Result<Value> {
    let (status, body) = call(
        router,
        user,
        "POST",
        &format!(
            "/api/v1/groups/{}/invitations",
            group["id"].as_str().unwrap()
        ),
        json!({"expected_version":group["version"],"kind":kind,"handle":handle,"max_uses":uses}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    Ok(body)
}
async fn join(
    router: &axum::Router,
    user: Uuid,
    issued: &Value,
) -> anyhow::Result<(StatusCode, Value)> {
    let reference = if issued["credential"].is_string() {
        json!({"credential":issued["credential"]})
    } else {
        json!({"invitation_id":issued["invitation"]["id"]})
    };
    let mut body = reference;
    body["confirm_roster_visibility"] = json!(true);
    call(router, user, "POST", "/api/v1/groups/join", body).await
}
fn member<'a>(view: &'a Value, handle: &str) -> &'a Value {
    view["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["handle"] == handle)
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn groups_are_authenticated_and_unavailable_until_server_rollout(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    store.ensure_development_user(DEVELOPMENT_USER_ID).await?;
    let router = app::build(&test_config(), store)?;
    let anonymous = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/groups/capability")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let (status, body) = call(
        &router,
        DEVELOPMENT_USER_ID,
        "GET",
        "/api/v1/groups/capability",
        Value::Null,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"enabled":false}));
    let (status, _) = call(
        &router,
        DEVELOPMENT_USER_ID,
        "POST",
        "/api/v1/groups",
        json!({"name":"Class","timezone":"UTC"}),
    )
    .await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_roles_direct_invitations_removal_and_ownership_are_scoped(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, store, [owner, alice, bob, visitor]) = fixture(&pool).await?;
    let mut group = create(&router, owner).await?;
    let path = format!("/api/v1/groups/{}", group["id"].as_str().unwrap());
    assert_eq!(
        call(&router, visitor, "GET", &path, Value::Null).await?.0,
        StatusCode::NOT_FOUND
    );
    let invitation = invite(&router, owner, &group, "direct", Some("@ALICE"), 1).await?;
    assert_eq!(
        join(&router, bob, &invitation).await?.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&router, alice, "GET", "/api/v1/groups", Value::Null)
            .await?
            .1,
        json!([])
    );
    let inbox = call(
        &router,
        alice,
        "GET",
        "/api/v1/groups/invitations/inbox",
        Value::Null,
    )
    .await?
    .1;
    assert_eq!(inbox["invitations"].as_array().unwrap().len(), 1);
    let (status, joined) = join(&router, alice, &invitation).await?;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["status"], "joined");
    assert_eq!(join(&router, alice, &invitation).await?.0, StatusCode::OK);
    let view = detail(&router, alice, &group).await?;
    assert_eq!(view["group"]["my_role"], "member");
    assert_eq!(view["invitations"], json!([]));
    for private in [
        "email",
        "share_progress",
        "discord_enabled",
        "secret_hash",
        "development.invalid",
    ] {
        assert!(!view.to_string().contains(private), "{private}");
    }
    let a = member(&view, "alice");
    let old_membership = a["id"].clone();
    let (status,_)=action(&router,owner,&group,json!({"kind":"set_role","membership_id":a["id"],"expected_membership_version":a["version"],"role":"leader"})).await?;
    assert_eq!(status, StatusCode::OK);
    let invitation = invite(&router, alice, &group, "direct", Some("bob"), 1).await?;
    assert_eq!(join(&router, bob, &invitation).await?.0, StatusCode::OK);
    let view = detail(&router, owner, &group).await?;
    let b = member(&view, "bob");
    assert_eq!(action(&router,owner,&group,json!({"kind":"set_role","membership_id":b["id"],"expected_membership_version":b["version"],"role":"leader"})).await?.0,StatusCode::OK);
    assert_eq!(
        action(
            &router,
            alice,
            &group,
            json!({"kind":"delete","confirmation":"DELETE GROUP"})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        action(
            &router,
            owner,
            &group,
            json!({"kind":"leave","expected_membership_version":group["my_membership_version"]})
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &router,
            owner,
            "DELETE",
            "/api/v1/account",
            json!({"confirmation":"DELETE"})
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT group_admission_blocked FROM users WHERE id=$1")
            .bind(owner)
            .fetch_one(&pool)
            .await?
    );
    let link = invite(&router, owner, &group, "link", None, 100).await?;
    let view = detail(&router, owner, &group).await?;
    let a = member(&view, "alice");
    assert_eq!(action(&router,owner,&group,json!({"kind":"remove","membership_id":a["id"],"expected_membership_version":a["version"]})).await?.0,StatusCode::OK);
    assert_eq!(
        call(&router, alice, "GET", &path, Value::Null).await?.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(join(&router, alice, &link).await?.0, StatusCode::FORBIDDEN);
    let view = detail(&router, owner, &group).await?;
    let removed = &view["removed_members"][0];
    assert_eq!(action(&router,owner,&group,json!({"kind":"allow_rejoin","membership_id":removed["id"],"expected_membership_version":removed["version"]})).await?.0,StatusCode::OK);
    assert_eq!(join(&router, alice, &link).await?.1["status"], "pending");
    let view = detail(&router, owner, &group).await?;
    let request = &view["join_requests"][0];
    assert_eq!(action(&router,owner,&group,json!({"kind":"review_join","request_id":request["id"],"expected_request_version":request["version"],"approve":true})).await?.0,StatusCode::OK);
    let view = detail(&router, alice, &group).await?;
    let a = member(&view, "alice");
    assert_ne!(a["id"], old_membership);
    assert_eq!(a["role"], "member");
    let b = member(&view, "bob");
    assert_eq!(
        action(
            &router,
            owner,
            &group,
            json!({"kind":"offer_ownership","membership_id":b["id"]})
        )
        .await?
        .0,
        StatusCode::OK
    );
    let view = detail(&router, bob, &group).await?;
    let (status, result) = action(
        &router,
        bob,
        &group,
        json!({"kind":"accept_ownership","offer_id":view["ownership_offer"]["id"]}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{result}");
    group = result["group"].clone();
    assert_eq!(group["owner_id"], json!(bob));
    assert_eq!(
        detail(&router, owner, &group).await?["group"]["my_role"],
        "leader"
    );
    store.delete_account(owner, "DELETE").await?;
    assert_eq!(
        detail(&router, bob, &group).await?["group"]["owner_id"],
        json!(bob)
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn code_approval_rechecks_revocation_and_does_not_auto_enroll(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, _, [owner, alice, bob, visitor]) = fixture(&pool).await?;
    let group = create(&router, owner).await?;
    let code = invite(&router, owner, &group, "code", None, 2).await?;
    let credential = code["credential"].as_str().unwrap();
    assert_eq!(credential.len(), 10);
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT secret_hash FROM group_invitations WHERE id=$1")
            .bind(code["invitation"]["id"].as_str().unwrap().parse::<Uuid>()?)
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored, Sha256::digest(credential.as_bytes()).to_vec());
    assert_eq!(
        call(
            &router,
            visitor,
            "POST",
            "/api/v1/groups/join/preview",
            json!({"credential":"0000000000"})
        )
        .await?
        .0,
        StatusCode::NOT_FOUND
    );
    let preview = call(
        &router,
        alice,
        "POST",
        "/api/v1/groups/join/preview",
        json!({"credential":credential}),
    )
    .await?;
    assert_eq!(preview.0, StatusCode::OK);
    assert_eq!(preview.1["approval_required"], true);
    assert_eq!(
        call(&router, alice, "GET", "/api/v1/groups", Value::Null)
            .await?
            .1,
        json!([])
    );
    let join_without_consent = call(
        &router,
        alice,
        "POST",
        "/api/v1/groups/join",
        json!({"credential":credential,"confirm_roster_visibility":false}),
    )
    .await?;
    assert_eq!(join_without_consent.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(join(&router, alice, &code).await?.1["status"], "pending");
    assert_eq!(join(&router, alice, &code).await?.1["status"], "pending");
    let inbox = call(
        &router,
        alice,
        "GET",
        "/api/v1/groups/invitations/inbox",
        Value::Null,
    )
    .await?
    .1;
    assert_eq!(inbox["requests"][0]["state"], "pending");
    let view = detail(&router, owner, &group).await?;
    let old_request = view["join_requests"][0].clone();
    let revoke = format!(
        "/api/v1/groups/{}/invitations/{}/revoke",
        group["id"].as_str().unwrap(),
        code["invitation"]["id"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &router,
            owner,
            "POST",
            &revoke,
            json!({"expected_version":code["invitation"]["version"]})
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(action(&router,owner,&group,json!({"kind":"review_join","request_id":old_request["id"],"expected_request_version":old_request["version"],"approve":true})).await?.0,StatusCode::CONFLICT);
    assert_eq!(join(&router, alice, &code).await?.0, StatusCode::NOT_FOUND);
    let fresh = invite(&router, owner, &group, "code", None, 2).await?;
    assert_eq!(join(&router, alice, &fresh).await?.1["status"], "pending");
    let view = detail(&router, owner, &group).await?;
    let request = &view["join_requests"][0];
    assert_ne!(request["id"], old_request["id"]);
    let approve = json!({"kind":"review_join","request_id":request["id"],"expected_request_version":request["version"],"approve":true});
    assert_eq!(
        action(&router, owner, &group, approve.clone()).await?.0,
        StatusCode::OK
    );
    assert_eq!(
        action(&router, owner, &group, approve).await?.0,
        StatusCode::CONFLICT
    );
    assert_eq!(join(&router, bob, &fresh).await?.1["status"], "pending");
    let view = detail(&router, owner, &group).await?;
    let request = &view["join_requests"][0];
    assert_eq!(action(&router,owner,&group,json!({"kind":"review_join","request_id":request["id"],"expected_request_version":request["version"],"approve":false})).await?.0,StatusCode::OK);
    assert_eq!(join(&router, bob, &fresh).await?.0, StatusCode::CONFLICT);
    let direct = invite(&router, owner, &group, "direct", Some("bob"), 1).await?;
    assert_eq!(join(&router, bob, &direct).await?.0, StatusCode::OK);
    assert_eq!(
        call(
            &router,
            bob,
            "GET",
            "/api/v1/groups/invitations/inbox",
            Value::Null
        )
        .await?
        .1["requests"],
        json!([])
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_undo_and_account_erasure_never_restore_erased_members(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, store, [owner, alice, _, _]) = fixture(&pool).await?;
    let mut group = create(&router, owner).await?;
    let direct = invite(&router, owner, &group, "direct", Some("alice"), 1).await?;
    assert_eq!(join(&router, alice, &direct).await?.0, StatusCode::OK);
    let (status, result) = action(
        &router,
        owner,
        &group,
        json!({"kind":"delete","confirmation":"DELETE GROUP"}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    group = result["group"].clone();
    assert!(group["undo_until"].is_string());
    assert_eq!(
        call(&router, alice, "GET", "/api/v1/groups", Value::Null)
            .await?
            .1,
        json!([])
    );
    let (status, result) = action(&router, owner, &group, json!({"kind":"restore"})).await?;
    assert_eq!(status, StatusCode::OK);
    group = result["group"].clone();
    assert_eq!(
        detail(&router, alice, &group).await?["group"]["name"],
        "College class"
    );
    group = action(
        &router,
        owner,
        &group,
        json!({"kind":"delete","confirmation":"DELETE GROUP"}),
    )
    .await?
    .1["group"]
        .clone();
    store.delete_account(alice, "DELETE").await?;
    assert_eq!(
        action(&router, owner, &group, json!({"kind":"restore"}))
            .await?
            .0,
        StatusCode::CONFLICT
    );
    sqlx::query("UPDATE groups SET deleted_at=NOW()-INTERVAL '2 minutes',undo_until=NOW()-INTERVAL '1 minute' WHERE id=$1").bind(group["id"].as_str().unwrap().parse::<Uuid>()?).execute(&pool).await?;
    store.delete_account(owner, "DELETE").await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM groups")
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn final_membership_slot_is_atomic_and_account_deletion_blocks_new_admissions(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, store, [owner, alice, bob, visitor]) = fixture(&pool).await?;
    let mut group = create(&router, owner).await?;
    group=action(&router,owner,&group,json!({"kind":"update","name":"College class","timezone":"Asia/Jakarta","approval_required":false})).await?.1["group"].clone();
    let id = group["id"].as_str().unwrap().parse::<Uuid>()?;
    sqlx::query("WITH u AS (INSERT INTO users(id,email,display_name) SELECT gen_random_uuid(),n::text||'@slot.test','' FROM generate_series(1,98) n RETURNING id,'fill'||left(replace(id::text,'-',''),24) AS handle),h AS (INSERT INTO user_handles(handle_hash,user_id,handle) SELECT sha256(convert_to(handle,'UTF8')),id,handle FROM u RETURNING user_id) INSERT INTO group_memberships(id,group_id,user_id) SELECT gen_random_uuid(),$1,user_id FROM h").bind(id).execute(&pool).await?;
    let code = invite(&router, owner, &group, "code", None, 100).await?;
    let (a, b) = tokio::join!(join(&router, alice, &code), join(&router, bob, &code));
    let mut statuses = [a?.0.as_u16(), b?.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 403]);
    assert_eq!(
        detail(&router, owner, &group).await?["group"]["member_count"],
        100
    );
    store.prepare_account_deletion(visitor).await?;
    assert_eq!(
        call(
            &router,
            visitor,
            "POST",
            "/api/v1/groups",
            json!({"name":"Too late","timezone":"UTC"})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(store.user_profile(visitor).await?.id, visitor);
    Ok(())
}

#[sqlx::test(migrations = "../../migrations")]
async fn group_mutations_bind_actor_cookie_csrf_and_strict_bodies(
    pool: PgPool,
) -> anyhow::Result<()> {
    let (router, store, [owner, alice, _, _]) = fixture(&pool).await?;
    let (status, _) = call(
        &router,
        owner,
        "POST",
        "/api/v1/groups",
        json!({"expected_user_id":alice,"name":"Wrong account","timezone":"UTC"}),
    )
    .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let group = create(&router, owner).await?;
    let (status, _) = call(
        &router,
        alice,
        "POST",
        &format!(
            "/api/v1/groups/{}/invitations",
            group["id"].as_str().unwrap()
        ),
        json!({"expected_version":group["version"],"kind":"direct","handle":"bob","max_uses":1}),
    )
    .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let session = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/development/session")
                .body(Body::empty())?,
        )
        .await?;
    let cookie = session.headers()[header::SET_COOKIE]
        .to_str()?
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let session: Value = serde_json::from_slice(&to_bytes(session.into_body(), 65536).await?)?;
    let id: Uuid = session["user"]["id"].as_str().unwrap().parse()?;
    store.claim_handle(id, "cookie_member").await?;
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/api/v1/groups")
            .header(header::COOKIE, &cookie)
            .header(header::CONTENT_TYPE, "application/json")
    };
    let valid = json!({"expected_user_id":id,"name":"Cookie class","timezone":"UTC"});
    let rejected = router
        .clone()
        .oneshot(request().body(Body::from(valid.to_string()))?)
        .await?;
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    let mut invalid = valid.clone();
    invalid["owner_id"] = json!(owner);
    let rejected = router
        .clone()
        .oneshot(
            request()
                .header("x-csrf-token", session["csrf_token"].as_str().unwrap())
                .body(Body::from(invalid.to_string()))?,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = router
        .oneshot(
            request()
                .header("x-csrf-token", session["csrf_token"].as_str().unwrap())
                .body(Body::from(valid.to_string()))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}
