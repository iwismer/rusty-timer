use std::collections::BTreeSet;

use crate::{Hello, HelloOk, ProtocolError};

/// Negotiates a server acknowledgement from client and server protocol ranges.
pub fn negotiate(client: &Hello, server: &Hello) -> Result<HelloOk, ProtocolError> {
    let protocol_minor = client.max_minor.min(server.max_minor);
    if protocol_minor < client.min_minor || protocol_minor < server.min_minor {
        return Err(ProtocolError::unsupported_version());
    }

    let client_capabilities = client.capabilities.iter().collect::<BTreeSet<_>>();
    let server_capabilities = server.capabilities.iter().collect::<BTreeSet<_>>();
    let capabilities = client_capabilities
        .intersection(&server_capabilities)
        .map(|capability| (*capability).clone())
        .collect();

    Ok(HelloOk {
        protocol_minor,
        capabilities,
        heartbeat_interval_secs: 0,
        max_batch_size: 0,
        max_frame_bytes: client.max_frame_bytes.min(server.max_frame_bytes),
        catalog_generation: server.catalog_generation,
    })
}
