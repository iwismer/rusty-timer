/// Integration tests for reader target range expansion.
///
/// Tests single IP parsing, range expansion, error cases for
/// unsupported syntaxes (CIDR, wildcard), and edge cases.
use forwarder::discovery::expand_target;

// ---------------------------------------------------------------------------
// Single IP
// ---------------------------------------------------------------------------

#[test]
fn single_ip_port_parses_to_one_entry() {
    let result = expand_target("192.168.2.156:10000").expect("should parse");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].ip, "192.168.2.156");
    assert_eq!(result[0].port, 10000);
}

#[test]
fn single_ip_last_octet_accessible() {
    let result = expand_target("10.0.0.200:9000").expect("should parse");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].ip, "10.0.0.200");
    assert_eq!(result[0].port, 9000);
    assert_eq!(result[0].last_octet, 200);
}

// ---------------------------------------------------------------------------
// Range expansion
// ---------------------------------------------------------------------------

#[test]
fn range_expands_inclusive() {
    let result = expand_target("192.168.2.150-160:10000").expect("should parse");
    assert_eq!(result.len(), 11); // 150 through 160 inclusive = 11 entries
    assert_eq!(result[0].ip, "192.168.2.150");
    assert_eq!(result[10].ip, "192.168.2.160");
    for ep in &result {
        assert_eq!(ep.port, 10000);
    }
}

#[test]
fn range_ips_are_in_ascending_order() {
    let result = expand_target("10.0.0.1-5:8080").expect("should parse");
    let ips: Vec<_> = result.iter().map(|e| e.ip.as_str()).collect();
    assert_eq!(
        ips,
        ["10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4", "10.0.0.5"]
    );
}

#[test]
fn range_start_equals_end_is_valid_single_host() {
    let result = expand_target("192.168.1.50-50:7000").expect("should parse");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].ip, "192.168.1.50");
}

#[test]
fn range_start_greater_than_end_returns_error() {
    let result = expand_target("192.168.1.160-150:10000");
    assert!(result.is_err(), "start > end must be an error");
}

// ---------------------------------------------------------------------------
// Unsupported syntaxes (must be rejected)
// ---------------------------------------------------------------------------

#[test]
fn cidr_notation_is_rejected() {
    let result = expand_target("192.168.1.0/24:10000");
    assert!(result.is_err(), "CIDR notation must be rejected");
}

#[test]
fn wildcard_notation_is_rejected() {
    let result = expand_target("192.168.1.*:10000");
    assert!(result.is_err(), "Wildcard notation must be rejected");
}

// ---------------------------------------------------------------------------
// Malformed targets
// ---------------------------------------------------------------------------

#[test]
fn target_without_port_returns_error() {
    let result = expand_target("192.168.1.10");
    assert!(result.is_err(), "target without port must be an error");
}

#[test]
fn empty_string_returns_error() {
    let result = expand_target("");
    assert!(result.is_err(), "empty string must be an error");
}

#[test]
fn non_numeric_octets_returns_error() {
    let result = expand_target("192.168.foo.10:8000");
    assert!(result.is_err(), "non-numeric octets must be an error");
}

#[test]
fn invalid_port_returns_error() {
    let result = expand_target("192.168.1.10:notaport");
    assert!(result.is_err(), "non-numeric port must be an error");
}

#[test]
fn range_with_invalid_start_returns_error() {
    let result = expand_target("192.168.1.foo-10:8000");
    assert!(result.is_err(), "invalid range start must fail");
}

#[test]
fn range_with_invalid_end_returns_error() {
    let result = expand_target("192.168.1.1-bar:8000");
    assert!(result.is_err(), "invalid range end must fail");
}

// ---------------------------------------------------------------------------
// Port edge cases
// ---------------------------------------------------------------------------

#[test]
fn port_zero_is_valid() {
    let result = expand_target("192.168.1.1:0").expect("should parse");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].ip, "192.168.1.1");
    assert_eq!(result[0].port, 0);
}

#[test]
fn port_max_is_valid() {
    let result = expand_target("192.168.1.1:65535").expect("should parse");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].ip, "192.168.1.1");
    assert_eq!(result[0].port, u16::MAX);
}
