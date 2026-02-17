//! Reader target range expansion.
//!
//! Supported syntaxes:
//! - Single: `A.B.C.D:PORT` — e.g. `192.168.2.156:10000`
//! - Range: `A.B.C.START-END:PORT` — e.g. `192.168.2.150-160:10000`
//!
//! NOT supported (explicitly rejected):
//! - CIDR notation (`192.168.1.0/24:port`)
//! - Wildcard (`192.168.1.*:port`)
//! - Subnet crawl / other formats

/// A fully-resolved reader endpoint after target expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderEndpoint {
    /// Dotted-decimal IP address string, e.g. "192.168.2.156".
    pub ip: String,
    /// TCP port.
    pub port: u16,
    /// Last octet of the IP address (used to compute default local_fallback_port).
    pub last_octet: u8,
}

impl ReaderEndpoint {
    /// Compute the default local fallback port: `10000 + last_octet`.
    pub fn default_local_fallback_port(&self) -> u16 {
        10000u16.saturating_add(self.last_octet as u16)
    }
}

/// Expand a reader target string into one or more `ReaderEndpoint`s.
///
/// Returns an error for unsupported syntaxes or malformed inputs.
pub fn expand_target(target: &str) -> Result<Vec<ReaderEndpoint>, DiscoveryError> {
    if target.is_empty() {
        return Err(DiscoveryError::InvalidFormat("empty target string".to_owned()));
    }

    // Reject CIDR notation
    if target.contains('/') {
        return Err(DiscoveryError::UnsupportedSyntax(
            "CIDR notation is not supported (use explicit IP or range)".to_owned(),
        ));
    }

    // Reject wildcard notation
    if target.contains('*') {
        return Err(DiscoveryError::UnsupportedSyntax(
            "wildcard notation is not supported (use explicit IP or range)".to_owned(),
        ));
    }

    // Split host and port on the last ':' to correctly handle IPv4 with ranges.
    // e.g. "192.168.2.150-160:10000" → host="192.168.2.150-160", port="10000"
    let colon_pos = target
        .rfind(':')
        .ok_or_else(|| DiscoveryError::InvalidFormat("missing port (expected HOST:PORT)".to_owned()))?;

    let host_part = &target[..colon_pos];
    let port_str = &target[colon_pos + 1..];

    if host_part.is_empty() {
        return Err(DiscoveryError::InvalidFormat("empty host part".to_owned()));
    }
    if port_str.is_empty() {
        return Err(DiscoveryError::InvalidFormat("empty port part".to_owned()));
    }

    let port: u16 = port_str
        .parse()
        .map_err(|_| DiscoveryError::InvalidFormat(format!("invalid port: '{}'", port_str)))?;

    // Parse the host part: detect range vs single IP
    // Split on '.' to get octets. The last octet field may contain a range like "150-160".
    let parts: Vec<&str> = host_part.splitn(4, '.').collect();
    if parts.len() != 4 {
        return Err(DiscoveryError::InvalidFormat(format!(
            "expected 4 octets, got {} in '{}'",
            parts.len(),
            host_part
        )));
    }

    let prefix_a: u8 = parse_octet(parts[0], "first octet")?;
    let prefix_b: u8 = parse_octet(parts[1], "second octet")?;
    let prefix_c: u8 = parse_octet(parts[2], "third octet")?;
    let last_field = parts[3];

    // Detect range in last field
    if let Some(dash_pos) = last_field.find('-') {
        let start_str = &last_field[..dash_pos];
        let end_str = &last_field[dash_pos + 1..];

        let start: u8 = parse_octet(start_str, "range start")?;
        let end: u8 = parse_octet(end_str, "range end")?;

        if start > end {
            return Err(DiscoveryError::InvalidRange(format!(
                "range start {} > end {} in '{}'",
                start, end, target
            )));
        }

        let mut endpoints = Vec::with_capacity((end - start + 1) as usize);
        for octet in start..=end {
            endpoints.push(ReaderEndpoint {
                ip: format!("{}.{}.{}.{}", prefix_a, prefix_b, prefix_c, octet),
                port,
                last_octet: octet,
            });
        }
        Ok(endpoints)
    } else {
        // Single IP
        let last_octet: u8 = parse_octet(last_field, "fourth octet")?;
        Ok(vec![ReaderEndpoint {
            ip: format!("{}.{}.{}.{}", prefix_a, prefix_b, prefix_c, last_octet),
            port,
            last_octet,
        }])
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum DiscoveryError {
    InvalidFormat(String),
    InvalidRange(String),
    UnsupportedSyntax(String),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryError::InvalidFormat(s) => write!(f, "Invalid target format: {}", s),
            DiscoveryError::InvalidRange(s) => write!(f, "Invalid range: {}", s),
            DiscoveryError::UnsupportedSyntax(s) => write!(f, "Unsupported syntax: {}", s),
        }
    }
}

impl std::error::Error for DiscoveryError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_octet(s: &str, label: &str) -> Result<u8, DiscoveryError> {
    s.parse::<u8>().map_err(|_| {
        DiscoveryError::InvalidFormat(format!("invalid {} '{}' (expected 0-255)", label, s))
    })
}
