use prosepect_api::store::Store;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
async fn retired_reservations_are_bounded_without_blocking_personal_access(
    pool: PgPool,
) -> anyhow::Result<()> {
    let store = Store::from_pool(pool.clone());
    let user = Uuid::now_v7();
    store.ensure_development_user(user).await?;
    sqlx::query("INSERT INTO user_handles(handle_hash) SELECT sha256(convert_to('retired-' || n::text,'UTF8')) FROM generate_series(1,10000) n")
        .execute(&pool).await?;
    let error = store
        .claim_handle(user, "new_classmate")
        .await
        .err()
        .expect("registry must be bounded");
    assert!(matches!(
        error,
        prosepect_api::error::AppError::Forbidden(_)
    ));
    assert!(store.own_handle(user).await?.handle.is_none());
    assert_eq!(store.user_profile(user).await?.id, user);
    Ok(())
}
