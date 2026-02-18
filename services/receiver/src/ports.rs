//! Deterministic local port mapping for stream re-exposure.
//!
//! Default mapping: `10000 + reader_ip_last_octet`.
//! Port collisions: affected stream marked degraded, non-conflicting streams start normally.

use crate::db::Subscription;
use std::collections::HashMap;

/// Result of resolving the local port for a subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortAssignment {
    /// The stream gets this TCP port.
    Assigned(u16),
    /// The stream could not get a unique port (collision with another stream's assignment).
    Collision { wanted: u16, collides_with: String },
}

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

/// Compute the default port: `10000 + last_octet`.
/// Returns `None` if the IP cannot be parsed as IPv4.
pub fn default_port(ip: &str) -> Option<u16> {
    last_octet(ip).map(|o| 10000u16 + o as u16)
}

/// Resolve port assignments for a list of subscriptions.
///
/// For each subscription:
/// - Use `local_port_override` if set.
/// - Otherwise use `10000 + last_octet(reader_ip)`.
///
/// If two subscriptions map to the same port, both are marked as `Collision`.
pub fn resolve_ports(subs: &[Subscription]) -> HashMap<String, PortAssignment> {
    // First pass: compute wanted port for each stream key.
    let mut wanted: Vec<(String, u16)> = Vec::new();
    for s in subs {
        let key = stream_key(&s.forwarder_id, &s.reader_ip);
        let port = s
            .local_port_override
            .or_else(|| default_port(&s.reader_ip))
            .unwrap_or(0);
        wanted.push((key, port));
    }

    // Second pass: detect collisions.
    // port -> first stream that claimed it
    let mut claimed: HashMap<u16, String> = HashMap::new();
    let mut assignments: HashMap<String, PortAssignment> = HashMap::new();

    for (key, port) in &wanted {
        if *port == 0 {
            // Could not compute port (unparseable IP + no override) - skip
            continue;
        }
        match claimed.get(port) {
            None => {
                claimed.insert(*port, key.clone());
            }
            Some(first) => {
                // Both this stream and the first claimant are colliding.
                // Mark first as collision if not already.
                let first_key = first.clone();
                assignments
                    .entry(first_key.clone())
                    .or_insert_with(|| PortAssignment::Collision {
                        wanted: *port,
                        collides_with: key.clone(),
                    });
                assignments.insert(
                    key.clone(),
                    PortAssignment::Collision {
                        wanted: *port,
                        collides_with: first_key,
                    },
                );
            }
        }
    }

    // Fill in non-colliding streams.
    for (key, port) in &wanted {
        if *port == 0 {
            continue;
        }
        assignments
            .entry(key.clone())
            .or_insert(PortAssignment::Assigned(*port));
    }

    assignments
}

/// Build a canonical stream key string.
pub fn stream_key(forwarder_id: &str, reader_ip: &str) -> String {
    format!("{forwarder_id}:{reader_ip}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(fwd: &str, ip: &str, port: Option<u16>) -> Subscription {
        Subscription {
            forwarder_id: fwd.to_owned(),
            reader_ip: ip.to_owned(),
            local_port_override: port,
        }
    }

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
    fn override_port_takes_priority() {
        let subs = vec![sub("f", "192.168.1.100:10000", Some(9999))];
        let r = resolve_ports(&subs);
        assert_eq!(
            r[&stream_key("f", "192.168.1.100:10000")],
            PortAssignment::Assigned(9999)
        );
    }

    #[test]
    fn no_collision_different_ips() {
        let subs = vec![
            sub("f", "192.168.1.100:10000", None),
            sub("f", "192.168.1.200:10000", None),
        ];
        let r = resolve_ports(&subs);
        assert_eq!(
            r[&stream_key("f", "192.168.1.100:10000")],
            PortAssignment::Assigned(10100)
        );
        assert_eq!(
            r[&stream_key("f", "192.168.1.200:10000")],
            PortAssignment::Assigned(10200)
        );
    }

    #[test]
    fn collision_same_last_octet_different_forwarder() {
        let subs = vec![
            sub("f1", "192.168.1.100:10000", None),
            sub("f2", "10.0.0.100:10000", None),
        ];
        let r = resolve_ports(&subs);
        // Both map to port 10100 - both should be Collision
        let k1 = stream_key("f1", "192.168.1.100:10000");
        let k2 = stream_key("f2", "10.0.0.100:10000");
        assert!(
            matches!(r[&k1], PortAssignment::Collision { wanted: 10100, .. }),
            "f1 should collide"
        );
        assert!(
            matches!(r[&k2], PortAssignment::Collision { wanted: 10100, .. }),
            "f2 should collide"
        );
    }

    #[test]
    fn collision_via_override() {
        let subs = vec![
            sub("f", "192.168.1.100:10000", Some(9500)),
            sub("f", "192.168.1.200:10000", Some(9500)),
        ];
        let r = resolve_ports(&subs);
        let k1 = stream_key("f", "192.168.1.100:10000");
        let k2 = stream_key("f", "192.168.1.200:10000");
        assert!(matches!(
            r[&k1],
            PortAssignment::Collision { wanted: 9500, .. }
        ));
        assert!(matches!(
            r[&k2],
            PortAssignment::Collision { wanted: 9500, .. }
        ));
    }

    #[test]
    fn non_colliding_streams_not_marked_degraded() {
        let subs = vec![
            sub("f", "10.0.0.1:10000", None), // 10001
            sub("f", "10.0.0.2:10000", None), // 10002
            sub("f", "10.0.0.3:10000", None), // 10003
        ];
        let r = resolve_ports(&subs);
        for ip in ["10.0.0.1:10000", "10.0.0.2:10000", "10.0.0.3:10000"] {
            assert!(matches!(
                r[&stream_key("f", ip)],
                PortAssignment::Assigned(_)
            ));
        }
    }

    #[test]
    fn only_colliding_pair_marked_degraded_others_fine() {
        let subs = vec![
            sub("f", "10.0.0.1:10000", None),  // 10001 - ok
            sub("f", "10.0.0.2:10000", None),  // 10002 - ok
            sub("f1", "10.0.0.1:10000", None), // 10001 - collides with f:10.0.0.1:10000
        ];
        let r = resolve_ports(&subs);
        assert!(matches!(
            r[&stream_key("f", "10.0.0.2:10000")],
            PortAssignment::Assigned(10002)
        ));
        assert!(matches!(
            r[&stream_key("f", "10.0.0.1:10000")],
            PortAssignment::Collision { .. }
        ));
        assert!(matches!(
            r[&stream_key("f1", "10.0.0.1:10000")],
            PortAssignment::Collision { .. }
        ));
    }

    #[test]
    fn empty_subscriptions_returns_empty_map() {
        assert!(resolve_ports(&[]).is_empty());
    }

    #[test]
    fn stream_key_format() {
        assert_eq!(
            stream_key("fwd-001", "192.168.1.100:10000"),
            "fwd-001:192.168.1.100:10000"
        );
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
