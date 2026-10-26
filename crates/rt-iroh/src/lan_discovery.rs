use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use iroh::EndpointId;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::Endpoint;

pub const DEFAULT_FORWARDER_DISCOVERY_PORT: u16 = 7448;
pub const DEFAULT_RECEIVER_DISCOVERY_PORT: u16 = 7449;

const MULTICAST_ADDR_V4: Ipv4Addr = Ipv4Addr::new(239, 255, 82, 84);
const QUERY_PACKET: &[u8] = b"RT-DISC-V1-QUERY\n";
const BEACON_PREFIX: &str = "RT-DISC-V1-FWD\n";
const BEACON_INTERVAL: Duration = Duration::from_secs(5);
const QUERY_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanForwarderBeacon {
    pub endpoint_id: EndpointId,
    pub p2p_port: u16,
    pub advertised_ips: Vec<IpAddr>,
    pub display_name: Option<String>,
}

impl LanForwarderBeacon {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let ips = self
            .advertised_ips
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let display = self
            .display_name
            .as_deref()
            .unwrap_or("")
            .replace(['\n', '\r'], " ");
        format!(
            "{BEACON_PREFIX}{}\n{}\n{ips}\n{}\n",
            self.endpoint_id,
            self.p2p_port,
            display.trim()
        )
        .into_bytes()
    }

    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let rest = text.strip_prefix(BEACON_PREFIX)?;
        let mut lines = rest.lines();
        let endpoint_id = lines.next()?.trim().parse::<EndpointId>().ok()?;
        let p2p_port = lines.next()?.trim().parse::<u16>().ok()?;
        if p2p_port == 0 {
            return None;
        }
        let ips_line = lines.next().unwrap_or("");
        let advertised_ips = ips_line
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<IpAddr>().ok())
            .collect::<Vec<_>>();
        let display_name = lines
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        Some(Self {
            endpoint_id,
            p2p_port,
            advertised_ips,
            display_name,
        })
    }

    /// Resolve the candidate socket addresses for this forwarder given the UDP
    /// packet's source IP. The source IP is always placed first because it is
    /// the interface that delivered the discovery packet to the receiver.
    #[must_use]
    pub fn direct_addrs(&self, sender_ip: IpAddr) -> Vec<SocketAddr> {
        let mut addrs = Vec::new();
        if !sender_ip.is_unspecified() {
            addrs.push(SocketAddr::new(sender_ip, self.p2p_port));
        }
        for &ip in &self.advertised_ips {
            if ip.is_unspecified() {
                continue;
            }
            if ip.is_loopback() && !sender_ip.is_loopback() {
                continue;
            }
            let candidate = SocketAddr::new(ip, self.p2p_port);
            if !addrs.contains(&candidate) {
                addrs.push(candidate);
            }
        }
        addrs
    }
}

#[must_use]
pub fn is_query_packet(bytes: &[u8]) -> bool {
    bytes.starts_with(QUERY_PACKET)
}

async fn bind_discovery_socket(preferred_port: u16) -> Option<UdpSocket> {
    let socket =
        match UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, preferred_port)).await {
            Ok(sock) => sock,
            Err(_) => UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
                .await
                .ok()?,
        };
    let _ = socket.set_broadcast(true);
    let _ = socket.join_multicast_v4(MULTICAST_ADDR_V4, Ipv4Addr::UNSPECIFIED);
    Some(socket)
}

async fn current_beacon(endpoint: &Endpoint, display_name: Option<&str>) -> Option<Vec<u8>> {
    let addr = endpoint.endpoint_addr().await;
    let ip_addrs: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
    let p2p_port = ip_addrs.first().map(SocketAddr::port)?;
    let advertised_ips = ip_addrs.into_iter().map(|a| a.ip()).collect();
    Some(
        LanForwarderBeacon {
            endpoint_id: endpoint.endpoint_id(),
            p2p_port,
            advertised_ips,
            display_name: display_name.map(ToOwned::to_owned),
        }
        .encode(),
    )
}

async fn broadcast_packet(socket: &UdpSocket, payload: &[u8], target_port: u16) {
    let targets = [
        SocketAddrV4::new(Ipv4Addr::BROADCAST, target_port),
        SocketAddrV4::new(MULTICAST_ADDR_V4, target_port),
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, target_port),
    ];
    for target in targets {
        let _ = socket.send_to(payload, target).await;
    }
}

/// Spawns a background task on the forwarder that broadcasts its P2P identity
/// and UDP port on the local network and replies to receiver discovery queries.
#[must_use]
pub fn spawn_forwarder_lan_beacon(
    endpoint: Endpoint,
    display_name: Option<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Some(socket) = bind_discovery_socket(DEFAULT_FORWARDER_DISCOVERY_PORT).await else {
            tracing::warn!("p2p: failed to bind LAN discovery socket");
            return;
        };
        if let Some(payload) = current_beacon(&endpoint, display_name.as_deref()).await {
            broadcast_packet(&socket, &payload, DEFAULT_RECEIVER_DISCOVERY_PORT).await;
        }

        let mut ticker = tokio::time::interval(BEACON_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        let mut buf = [0u8; 1024];

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Some(payload) = current_beacon(&endpoint, display_name.as_deref()).await {
                        broadcast_packet(&socket, &payload, DEFAULT_RECEIVER_DISCOVERY_PORT).await;
                    }
                }
                recv = socket.recv_from(&mut buf) => {
                    if let Ok((len, peer_addr)) = recv
                        && is_query_packet(&buf[..len])
                        && let Some(payload) = current_beacon(&endpoint, display_name.as_deref()).await
                    {
                        let _ = socket.send_to(&payload, peer_addr).await;
                    }
                }
            }
        }
    })
}

/// Spawns a background task on the receiver that queries the local network and
/// listens for forwarder LAN discovery beacons.
#[must_use]
pub fn spawn_receiver_lan_listener(
    tx: mpsc::Sender<(LanForwarderBeacon, IpAddr)>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Some(socket) = bind_discovery_socket(DEFAULT_RECEIVER_DISCOVERY_PORT).await else {
            tracing::warn!("p2p: failed to bind receiver LAN discovery listener");
            return;
        };
        broadcast_packet(&socket, QUERY_PACKET, DEFAULT_FORWARDER_DISCOVERY_PORT).await;

        let mut ticker = tokio::time::interval(QUERY_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        let mut buf = [0u8; 1024];

        loop {
            tokio::select! {
                biased;
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    broadcast_packet(&socket, QUERY_PACKET, DEFAULT_FORWARDER_DISCOVERY_PORT).await;
                }
                recv = socket.recv_from(&mut buf) => {
                    if let Ok((len, src_addr)) = recv
                        && let Some(beacon) = LanForwarderBeacon::decode(&buf[..len])
                        && tx.send((beacon, src_addr.ip())).await.is_err()
                    {
                        break;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn beacon_encode_decode_roundtrip() {
        let endpoint_id = SecretKey::from_bytes(&[7; 32]).public();
        let beacon = LanForwarderBeacon {
            endpoint_id,
            p2p_port: 7447,
            advertised_ips: vec!["192.168.1.50".parse().unwrap(), "10.0.0.5".parse().unwrap()],
            display_name: Some("Finish Line".to_owned()),
        };
        let encoded = beacon.encode();
        let decoded = LanForwarderBeacon::decode(&encoded).expect("decode beacon");
        assert_eq!(decoded, beacon);

        let sender_ip: IpAddr = "192.168.1.50".parse().unwrap();
        let addrs = decoded.direct_addrs(sender_ip);
        assert_eq!(
            addrs,
            vec![
                "192.168.1.50:7447".parse().unwrap(),
                "10.0.0.5:7447".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn beacon_direct_addrs_puts_sender_ip_first_and_filters_loopback_for_lan_sender() {
        let endpoint_id = SecretKey::from_bytes(&[8; 32]).public();
        let beacon = LanForwarderBeacon {
            endpoint_id,
            p2p_port: 9000,
            advertised_ips: vec![
                "127.0.0.1".parse().unwrap(),
                "192.168.1.99".parse().unwrap(),
            ],
            display_name: None,
        };
        let sender_ip: IpAddr = "192.168.1.42".parse().unwrap();
        let addrs = beacon.direct_addrs(sender_ip);
        assert_eq!(
            addrs,
            vec![
                "192.168.1.42:9000".parse().unwrap(),
                "192.168.1.99:9000".parse().unwrap(),
            ]
        );
    }
}
