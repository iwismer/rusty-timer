use sha2::{Digest, Sha256};
use sqlx::PgPool;

pub struct TokenClaims {
    pub device_id: String,
    pub device_type: String,
}

pub async fn validate_token(pool: &PgPool, raw_token: &str) -> Option<TokenClaims> {
    let hash = Sha256::digest(raw_token.as_bytes());
    let hash_bytes = hash.as_slice().to_vec();
    let row = sqlx::query!(
        r#"SELECT device_id, device_type
           FROM device_tokens
           WHERE token_hash = $1
             AND revoked_at IS NULL"#,
        hash_bytes.as_slice()
    )
    .fetch_optional(pool)
    .await
    .ok()??;
    Some(TokenClaims {
        device_id: row.device_id,
        device_type: row.device_type,
    })
}

pub fn extract_bearer(authorization: &str) -> Option<&str> {
    authorization.strip_prefix("Bearer ")
}

#[cfg(test)]
mod tests {
    use super::extract_bearer;

    #[test]
    fn extract_bearer_returns_token_for_valid_header() {
        assert_eq!(extract_bearer("Bearer token-123"), Some("token-123"));
    }

    #[test]
    fn extract_bearer_rejects_wrong_prefix_or_spacing() {
        assert_eq!(extract_bearer("bearer token-123"), None);
        assert_eq!(extract_bearer("Token token-123"), None);
        assert_eq!(extract_bearer("Bearer"), None);
    }

    #[test]
    fn extract_bearer_allows_empty_token_after_prefix() {
        assert_eq!(extract_bearer("Bearer "), Some(""));
    }
}
