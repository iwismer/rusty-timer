//! Deterministic local port mapping for stream re-exposure.
//!
//! Default mapping:
//! - Legacy/default reader port (`:10000`): `10000 + reader_ip_last_octet`
//! - Non-default reader port: deterministic hash in `12000..=65535`

/// Parse the last octet of an IPv4 address string.
/// Returns `None` if the address is not a parseable IPv4 address.
pub fn last_octet(ip: &str) -> Option<u8> {
    // Strip port suffix if present (e.g., "192.168.1.100:10000")
    let ip_part = ip.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(ip);
    let parts: Vec<&str> = ip_part.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    parts[3].parse::<u8>().ok()
}

fn parse_reader_source_port(reader_ip: &str) -> Option<Option<u16>> {
    match reader_ip.rsplit_once(':') {
        Some((_ip, port_str)) => {
            let port = port_str.parse::<u16>().ok()?;
            if port == 0 {
                return None;
            }
            Some(Some(port))
        }
        None => Some(None),
    }
}

fn fnv1a_64(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in input.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Compute the default port: `10000 + last_octet`.
/// For non-default reader ports (`!=10000`), map to a deterministic hashed port range.
/// Returns `None` if the IP/port cannot be parsed.
pub fn default_port(ip: &str) -> Option<u16> {
    const LEGACY_READER_PORT: u16 = 10000;
    const DYNAMIC_MIN_PORT: u16 = 12000;

    let source_port = parse_reader_source_port(ip)?;
    let octet = last_octet(ip)?;
    let legacy = 10000u16 + u16::from(octet);
    if source_port.is_none() || source_port == Some(LEGACY_READER_PORT) {
        return Some(legacy);
    }

    let span = (u16::MAX as u32) - (DYNAMIC_MIN_PORT as u32) + 1;
    let offset = (fnv1a_64(ip) % u64::from(span)) as u32;
    Some((DYNAMIC_MIN_PORT as u32 + offset) as u16)
}

/// Return `value` when it is a reader network address that can drive display
/// metadata and default local-port resolution.
pub fn reader_addr_if_port_mappable(value: &str) -> Option<&str> {
    default_port(value).map(|_| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_from_last_octet() {
        assert_eq!(default_port("192.168.1.100"), Some(10100));
        assert_eq!(default_port("10.0.0.1"), Some(10001));
        assert_eq!(default_port("10.0.0.200"), Some(10200));
        assert_eq!(default_port("10.0.0.255"), Some(10255));
        // ip:port format
        assert_eq!(default_port("192.168.1.100:10000"), Some(10100));
        assert_eq!(default_port("10.0.0.1:10000"), Some(10001));
        assert_eq!(default_port("10.0.0.200:10000"), Some(10200));
        assert_eq!(default_port("10.0.0.255:10000"), Some(10255));
    }

    #[test]
    fn default_port_from_last_octet_zero() {
        assert_eq!(default_port("10.0.0.0"), Some(10000));
        assert_eq!(default_port("10.0.0.0:10000"), Some(10000));
    }

    #[test]
    fn default_port_non_default_reader_port_uses_distinct_values() {
        let p1 = default_port("10.0.0.1:10001").expect("parse :10001");
        let p2 = default_port("10.0.0.1:10002").expect("parse :10002");
        assert_ne!(p1, p2, "same IP should not collide across source ports");
        assert!(p1 >= 12000);
        assert!(p2 >= 12000);
    }

    #[test]
    fn last_octet_with_port_suffix() {
        assert_eq!(last_octet("192.168.1.100:10000"), Some(100));
        assert_eq!(last_octet("10.0.0.1:10000"), Some(1));
        assert_eq!(last_octet("10.0.0.255:10000"), Some(255));
    }

    #[test]
    fn last_octet_invalid_ip_returns_none() {
        assert_eq!(last_octet("not-an-ip"), None);
        assert_eq!(last_octet("192.168.1"), None);
        assert_eq!(last_octet(""), None);
    }
}
