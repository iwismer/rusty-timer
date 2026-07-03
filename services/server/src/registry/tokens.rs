use rand::Rng;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::{ApprovalState, DeviceKind, DeviceRecord, get_device};

/// Length in bytes of the random per-token salt.
const TOKEN_SALT_LEN: usize = 16;

/// Hash a raw bearer token for storage/comparison. Tokens are never persisted
/// in plaintext.
///
/// Each call generates a fresh random salt and returns the UTF-8 bytes of
/// `"<salt_hex>$<sha256(salt || token)_hex>"`. Salting means identical tokens
/// produce distinct stored hashes and defeats precomputed (rainbow-table)
/// attacks against a leaked registry. Use [`verify_token`] to check a candidate
/// token against a stored hash — never compare hashes with `==`, which is not
/// constant-time.
#[must_use]
pub fn hash_token(raw_token: &str) -> Vec<u8> {
    let mut salt = [0u8; TOKEN_SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    encode_salted_hash(&salt, raw_token).into_bytes()
}

/// Verify a candidate raw token against a salted hash produced by [`hash_token`].
///
/// Returns `false` for any malformed stored hash. The final digest comparison
/// is constant-time to avoid leaking how many leading bytes matched.
#[must_use]
pub fn verify_token(raw_token: &str, stored_hash: &[u8]) -> bool {
    let Ok(stored) = std::str::from_utf8(stored_hash) else {
        return false;
    };
    let Some((salt_hex, expected_digest_hex)) = stored.split_once('$') else {
        return false;
    };
    let Some(salt) = decode_hex(salt_hex) else {
        return false;
    };
    let Some(expected_digest) = decode_hex(expected_digest_hex) else {
        return false;
    };
    let actual_digest = Sha256::digest(salted_input(&salt, raw_token));
    actual_digest.as_slice().ct_eq(&expected_digest).into()
}

fn salted_input(salt: &[u8], raw_token: &str) -> Vec<u8> {
    let mut input = Vec::with_capacity(salt.len() + raw_token.len());
    input.extend_from_slice(salt);
    input.extend_from_slice(raw_token.as_bytes());
    input
}

fn encode_salted_hash(salt: &[u8], raw_token: &str) -> String {
    let digest = Sha256::digest(salted_input(salt, raw_token));
    format!("{}${}", encode_hex(salt), encode_hex(&digest))
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    // Decode byte-wise (never slice the `str`, which would panic on a
    // multi-byte UTF-8 boundary). A malformed stored hash must fail closed by
    // returning `None`, never by panicking.
    let bytes = hex.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks_exact(2)
        .map(|pair| Some((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Prefix that identifies a minted per-device bearer token.
pub(super) const DEVICE_TOKEN_PREFIX: &str = "rtk_";
/// Byte length of the random per-device `token_id` (hex-encoded, ~128-bit).
pub(super) const DEVICE_TOKEN_ID_LEN: usize = 16;
/// Byte length of the random per-device token secret.
pub(super) const DEVICE_TOKEN_SECRET_LEN: usize = 32;

pub(super) fn random_hex(len_bytes: usize) -> String {
    let mut buf = vec![0u8; len_bytes];
    rand::rng().fill_bytes(&mut buf);
    encode_hex(&buf)
}

/// Format a minted device token as `rtk_<token_id>_<secret>`.
pub(super) fn format_device_token(token_id: &str, secret: &str) -> String {
    format!("{DEVICE_TOKEN_PREFIX}{token_id}_{secret}")
}

/// Split a raw bearer into `(token_id, secret)` iff it is a well-formed device
/// token. The `token_id` is the underscore-free hex segment after the prefix.
fn parse_device_token(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix(DEVICE_TOKEN_PREFIX)?;
    let (token_id, secret) = rest.split_once('_')?;
    if token_id.is_empty() || secret.is_empty() {
        return None;
    }
    Some((token_id, secret))
}

/// Resolve a raw bearer token to its device record via the indexed `token_id`,
/// verifying the secret in constant time.
///
/// Returns `None` for any token that is not a well-formed device token, has no
/// matching `devices` row, or fails verification — i.e. fail-closed. The caller
/// asserts device kind / approval state per endpoint.
pub fn authenticate_device(
    conn: &Connection,
    raw_token: &str,
) -> rusqlite::Result<Option<DeviceRecord>> {
    let Some((token_id, secret)) = parse_device_token(raw_token) else {
        return Ok(None);
    };
    let row = conn
        .query_row(
            "SELECT endpoint_id, device_kind, approval_state, token_hash
             FROM devices WHERE token_id = ?1",
            [token_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((endpoint_id, kind_raw, approval_raw, token_hash)) = row else {
        return Ok(None);
    };
    if !verify_token(secret, &token_hash) {
        return Ok(None);
    }
    // Reject a corrupt row fail-closed (deny) rather than erroring the auth path.
    if DeviceKind::parse(&kind_raw).is_none() || ApprovalState::parse(&approval_raw).is_none() {
        return Ok(None);
    }
    // Return the canonical record so the resolved `display_name` is populated
    // consistently with `get_device`/`list_devices`.
    get_device(conn, &endpoint_id)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::registry::{approve_device, migrate, register_device, register_device_minted};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn authenticate_device_roundtrips_and_fails_closed() {
        let conn = test_conn();
        let minted = register_device_minted(&conn, "ep-1", DeviceKind::Forwarder).unwrap();
        assert!(minted.device_token.starts_with("rtk_"));
        let record = authenticate_device(&conn, &minted.device_token)
            .unwrap()
            .expect("token resolves");
        assert_eq!(record.endpoint_id, "ep-1");
        assert_eq!(record.device_kind, DeviceKind::Forwarder);
        assert_eq!(record.approval_state, ApprovalState::Pending);
        // Tampered secret, and non-device-token shapes, fail closed.
        assert!(
            authenticate_device(&conn, &format!("{}x", minted.device_token))
                .unwrap()
                .is_none()
        );
        assert!(authenticate_device(&conn, "not-a-token").unwrap().is_none());
        assert!(authenticate_device(&conn, "rtk_onlyid").unwrap().is_none());
    }

    #[test]
    fn tokens_are_hashed_not_plaintext() {
        let conn = test_conn();
        register_device(&conn, "ep-3", DeviceKind::Forwarder, "super-secret").unwrap();

        let stored: Vec<u8> = conn
            .query_row(
                "SELECT token_hash FROM devices WHERE endpoint_id = 'ep-3'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(verify_token("super-secret", &stored));
        assert!(!verify_token("wrong-secret", &stored));
        assert_ne!(stored, b"super-secret".to_vec());
    }

    #[test]
    fn hash_token_is_salted_and_verifiable() {
        // Two hashes of the same token differ (random salt) but both verify.
        let a = hash_token("same-token");
        let b = hash_token("same-token");
        assert_ne!(a, b, "salted hashes of the same token must differ");
        assert!(verify_token("same-token", &a));
        assert!(verify_token("same-token", &b));
        assert!(!verify_token("same-token ", &a));
    }

    #[test]
    fn verify_token_rejects_malformed_stored_hash() {
        assert!(!verify_token("x", b"not-hex"));
        assert!(!verify_token("x", b"deadbeef"));
        assert!(!verify_token("x", b"zz$zz"));
        assert!(!verify_token("x", &[0xff, 0xfe]));
        // Valid UTF-8 but multi-byte hex parts must fail closed, not panic
        // (would have panicked when slicing the str by byte index).
        assert!(!verify_token("x", "é$é".as_bytes()));
        assert!(!verify_token("x", "00$é".as_bytes()));
        assert!(!verify_token("x", "abcd€xyz$0011".as_bytes()));
    }

    #[test]
    fn register_device_minted_preserves_approval_and_rotates() {
        let conn = test_conn();
        let first = register_device_minted(&conn, "ep-1", DeviceKind::Receiver).unwrap();
        approve_device(&conn, "ep-1").unwrap().unwrap();
        let second = register_device_minted(&conn, "ep-1", DeviceKind::Receiver).unwrap();
        assert_eq!(second.record.approval_state, ApprovalState::Active);
        // Re-mint rotates: the old token no longer authenticates.
        assert!(
            authenticate_device(&conn, &first.device_token)
                .unwrap()
                .is_none()
        );
        assert!(
            authenticate_device(&conn, &second.device_token)
                .unwrap()
                .is_some()
        );
    }
}
