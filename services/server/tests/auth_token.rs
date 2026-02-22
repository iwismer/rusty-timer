use server::auth::validate_token;
use sha2::{Digest, Sha256};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn test_pool() -> (testcontainers::ContainerAsync<Postgres>, sqlx::PgPool) {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    (container, pool)
}

async fn insert_token(pool: &sqlx::PgPool, device_id: &str, device_type: &str, raw_token: &str) {
    let hash = Sha256::digest(raw_token.as_bytes());
    sqlx::query(
        "INSERT INTO device_tokens (token_hash, device_type, device_id) VALUES ($1, $2, $3)",
    )
    .bind(hash.as_slice())
    .bind(device_type)
    .bind(device_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn revoke_token(pool: &sqlx::PgPool, raw_token: &str) {
    let hash = Sha256::digest(raw_token.as_bytes());
    sqlx::query("UPDATE device_tokens SET revoked_at = now() WHERE token_hash = $1")
        .bind(hash.as_slice())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn validate_token_returns_claims_for_known_active_token() {
    let (_container, pool) = test_pool().await;
    insert_token(&pool, "fwd-auth-001", "forwarder", "good-token").await;

    let claims = validate_token(&pool, "good-token").await;

    assert!(claims.is_some());
    let claims = claims.unwrap();
    assert_eq!(claims.device_id, "fwd-auth-001");
    assert_eq!(claims.device_type, "forwarder");
}

#[tokio::test]
async fn validate_token_returns_none_for_revoked_token() {
    let (_container, pool) = test_pool().await;
    insert_token(&pool, "rcv-auth-001", "receiver", "revoked-token").await;
    revoke_token(&pool, "revoked-token").await;

    let claims = validate_token(&pool, "revoked-token").await;

    assert!(claims.is_none());
}

#[tokio::test]
async fn validate_token_returns_none_for_unknown_token() {
    let (_container, pool) = test_pool().await;
    insert_token(&pool, "fwd-auth-002", "forwarder", "existing-token").await;

    let claims = validate_token(&pool, "missing-token").await;

    assert!(claims.is_none());
}

#[tokio::test]
async fn validate_token_returns_none_for_hash_mismatch() {
    let (_container, pool) = test_pool().await;
    insert_token(&pool, "fwd-auth-003", "forwarder", "stored-token").await;

    let claims = validate_token(&pool, "stored-token-typo").await;

    assert!(claims.is_none());
}
