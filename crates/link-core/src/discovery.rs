use crate::{CoreError, DeviceInfo, DISCOVERY_PORT, MDNS_SERVICE_TYPE};
use tokio::net::UdpSocket;

/// Discovered peer event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredDevice {
    pub info: DeviceInfo,
    pub last_seen_secs: u64,
    #[serde(default)]
    pub addresses: Vec<String>,
}

pub trait Discovery: Send + Sync {
    fn start(&self) -> impl std::future::Future<Output = Result<(), CoreError>> + Send;
    fn stop(&self) -> impl std::future::Future<Output = Result<(), CoreError>> + Send;
}

/// mDNS discovery (Phase 1b). [`MdnsDiscovery`] keeps the trait seam for the
/// future QUIC/mDNS overhaul; the free functions below are what daemon +
/// tests use today. Emulator NAT blocks mDNS — explicit peer IP there.
pub struct MdnsDiscovery {
    pub service_name: String,
}

impl MdnsDiscovery {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }
}

impl Discovery for MdnsDiscovery {
    async fn start(&self) -> Result<(), CoreError> {
        Err(CoreError::Discovery(
            "use register_mdns() handle; the trait seam is for Phase 2".into(),
        ))
    }

    async fn stop(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

/// Registered mDNS service. Drop unregisters. Keep alive for process lifetime.
pub struct MdnsRegistration {
    _daemon: mdns_sd::ServiceDaemon,
}

fn txt(info: &DeviceInfo) -> Vec<(String, String)> {
    vec![
        ("id".to_string(), info.id.to_string()),
        ("os".to_string(), info.os.clone()),
        ("proto".to_string(), info.protocol_version.to_string()),
        ("fp".to_string(), info.pubkey_fingerprint.clone()),
    ]
}

/// Register `_linktolinux._udp.local.` with our identity in TXT records.
pub fn register_mdns(
    instance: &str,
    port: u16,
    info: &DeviceInfo,
) -> Result<MdnsRegistration, CoreError> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| CoreError::Discovery(e.to_string()))?;
    let host = format!("{instance}.local.");
    let props = txt(info);
    let props_ref: Vec<(&str, &str)> = props
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    // Advertise a routable address: some browsers only resolve services with A records.
    let ip = local_ip();
    let service = mdns_sd::ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        instance,
        &host,
        &ip,
        port,
        &props_ref[..],
    )
    .map_err(|e| CoreError::Discovery(e.to_string()))?;
    daemon
        .register(service)
        .map_err(|e| CoreError::Discovery(e.to_string()))?;
    Ok(MdnsRegistration { _daemon: daemon })
}

/// Best-effort LAN IP via UDP `connect` (no packets are sent).
fn local_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

/// Browse for peers for `timeout_secs`. Resolves TXT records back into [`DeviceInfo`].
/// Own instance (matching `own_id`) is filtered when provided.
pub async fn browse_mdns(
    timeout_secs: u64,
    own_id: Option<crate::DeviceId>,
) -> Result<Vec<DeviceInfo>, CoreError> {
    Ok(browse_mdns_peers(timeout_secs, own_id)
        .await?
        .into_iter()
        .map(|d| d.info)
        .collect())
}

pub async fn browse_mdns_peers(
    timeout_secs: u64,
    own_id: Option<crate::DeviceId>,
) -> Result<Vec<DiscoveredDevice>, CoreError> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| CoreError::Discovery(e.to_string()))?;
    let receiver = daemon
        .browse(MDNS_SERVICE_TYPE)
        .map_err(|e| CoreError::Discovery(e.to_string()))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut out = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let step = remaining.min(std::time::Duration::from_millis(200));
        match tokio::time::timeout(step, receiver.recv_async()).await {
            Ok(Ok(mdns_sd::ServiceEvent::ServiceResolved(info))) => {
                if let Some(device) = device_from_service(&info) {
                    if Some(device.id) != own_id && device.validate().is_ok() {
                        out.push(DiscoveredDevice {
                            addresses: service_addresses(&info),
                            info: device,
                            last_seen_secs: 0,
                        });
                    }
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
    daemon.shutdown().ok();
    Ok(out)
}

fn service_addresses(info: &mdns_sd::ServiceInfo) -> Vec<String> {
    let mut addresses: Vec<_> = info
        .get_addresses()
        .iter()
        .map(|ip| std::net::SocketAddr::new(*ip, info.get_port()).to_string())
        .collect();
    addresses.sort_by_key(|address| address.starts_with('['));
    addresses
}

fn device_from_service(info: &mdns_sd::ServiceInfo) -> Option<DeviceInfo> {
    let id: crate::DeviceId = info.get_property_val_str("id")?.parse().ok()?;
    let os = info.get_property_val_str("os")?.to_string();
    let proto: u16 = info.get_property_val_str("proto")?.parse().ok()?;
    let fp = info.get_property_val_str("fp")?.to_string();
    // Instance name is "<name>-<shortid>"; recover the friendly part.
    let fullname = info.get_fullname();
    let instance = fullname
        .strip_suffix(&format!(".{MDNS_SERVICE_TYPE}"))
        .unwrap_or(fullname);
    let name = instance
        .rsplit_once('-')
        .map(|(n, _)| n)
        .unwrap_or(instance)
        .to_string();
    Some(DeviceInfo {
        id,
        name,
        os,
        protocol_version: proto,
        quic_port: info.get_port(),
        pubkey_fingerprint: fp,
        capabilities: vec![
            crate::Capability::Pair,
            crate::Capability::Notify,
            crate::Capability::File,
            crate::Capability::Clip,
        ],
    })
}

/// Broadcast our CBOR-encoded identity on UDP `DISCOVERY_PORT`.
/// Emulator NAT blocks broadcast — use explicit `tcp_ping` peer instead there.
pub async fn broadcast_identity(info: &DeviceInfo) -> Result<(), CoreError> {
    let bytes = crate::codec::encode_identity(info)?;
    let sock = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| CoreError::Discovery(e.to_string()))?;
    sock.set_broadcast(true)
        .map_err(|e| CoreError::Discovery(e.to_string()))?;
    sock.send_to(&bytes, format!("255.255.255.255:{DISCOVERY_PORT}"))
        .await
        .map_err(|e| CoreError::Discovery(e.to_string()))?;
    Ok(())
}

/// Listen for broadcast identities for `timeout_secs`. Returns validated peers.
/// Binds `0.0.0.0:DISCOVERY_PORT`.
pub async fn listen_for_identities(timeout_secs: u64) -> Result<Vec<DeviceInfo>, CoreError> {
    Ok(listen_for_peers(timeout_secs)
        .await?
        .into_iter()
        .map(|d| d.info)
        .collect())
}

pub async fn listen_for_peers(timeout_secs: u64) -> Result<Vec<DiscoveredDevice>, CoreError> {
    let sock = UdpSocket::bind(format!("0.0.0.0:{DISCOVERY_PORT}"))
        .await
        .map_err(|e| CoreError::Discovery(e.to_string()))?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut out = Vec::new();
    let mut buf = vec![0u8; 65535];
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let res = tokio::time::timeout(remaining, sock.recv_from(&mut buf)).await;
        match res {
            Ok(Ok((n, addr))) => {
                if let Ok(info) = crate::codec::decode::<DeviceInfo>(&buf[..n]) {
                    if info.validate().is_ok() {
                        let address =
                            std::net::SocketAddr::new(addr.ip(), info.quic_port).to_string();
                        out.push(DiscoveredDevice {
                            info,
                            addresses: vec![address],
                            last_seen_secs: 0,
                        });
                    }
                }
            }
            _ => break,
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Continuous loops (presence). One-shot helpers above stay for CLI + tests.
// Shutdown: `tokio::sync::watch::Receiver<bool>` (`true` = stop).
// ---------------------------------------------------------------------------

use crate::presence::PresenceTracker;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Re-broadcast our identity every `interval` until stopped.
pub async fn announce_loop(
    info: DeviceInfo,
    interval: Duration,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            break;
        }
        if let Err(e) = broadcast_identity(&info).await {
            tracing::debug!("announce broadcast failed: {e:#}");
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = stop.wait_for(|s| *s) => break,
        }
    }
}

/// Bind `0.0.0.0:DISCOVERY_PORT` once; feed every valid packet to the tracker.
pub async fn listen_loop_udp(
    tracker: Arc<PresenceTracker>,
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), CoreError> {
    let sock = UdpSocket::bind(format!("0.0.0.0:{DISCOVERY_PORT}"))
        .await
        .map_err(|e| CoreError::Discovery(e.to_string()))?;
    let mut buf = vec![0u8; 65535];
    loop {
        if *stop.borrow() {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(500), sock.recv_from(&mut buf)).await {
            Ok(Ok((n, addr))) => match crate::codec::decode::<DeviceInfo>(&buf[..n]) {
                // Invalid/own peers are ignored inside update(); undecodable
                // garbage is debug-logged, never fatal.
                Ok(info) => {
                    let address = std::net::SocketAddr::new(addr.ip(), info.quic_port).to_string();
                    if let Err(e) = tracker.update_at(info, vec![address]) {
                        tracing::debug!("ignoring invalid discovery peer: {e:#}");
                    }
                }
                Err(_) => tracing::debug!("ignoring undecodable discovery packet"),
            },
            Ok(Err(e)) => {
                tracing::debug!("discovery recv error: {e}");
            }
            Err(_) => {} // timeout: re-check stop flag
        }
    }
    Ok(())
}

/// Persistent mDNS browser feeding the tracker. `ServiceRemoved` drops the
/// peer immediately via an instance-name → id map built from resolves.
pub async fn browse_loop_mdns(
    tracker: Arc<PresenceTracker>,
    own_id: Option<crate::DeviceId>,
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), CoreError> {
    let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| CoreError::Discovery(e.to_string()))?;
    let receiver = daemon
        .browse(MDNS_SERVICE_TYPE)
        .map_err(|e| CoreError::Discovery(e.to_string()))?;
    let mut by_instance: HashMap<String, crate::DeviceId> = HashMap::new();
    loop {
        if *stop.borrow() {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(200), receiver.recv_async()).await {
            Ok(Ok(mdns_sd::ServiceEvent::ServiceResolved(info))) => {
                if let Some(device) = device_from_service(&info) {
                    if Some(device.id) != own_id {
                        by_instance.insert(info.get_fullname().to_string(), device.id);
                        if let Err(e) = tracker.update_at(device, service_addresses(&info)) {
                            tracing::debug!("ignoring invalid mdns peer: {e:#}");
                        }
                    }
                }
            }
            Ok(Ok(mdns_sd::ServiceEvent::ServiceRemoved(_ty, fullname))) => {
                if let Some(id) = by_instance.remove(&fullname) {
                    let _ = tracker.remove(&id);
                    tracing::debug!("mdns peer removed: {id}");
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {} // timeout: re-check stop flag
        }
    }
    daemon.shutdown().ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capability, DeviceId, PROTOCOL_VERSION, QUIC_PORT};

    fn test_info(name: &str) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::generate(),
            name: name.into(),
            os: "linux".into(),
            protocol_version: PROTOCOL_VERSION,
            quic_port: QUIC_PORT,
            pubkey_fingerprint: "sha256:test".into(),
            capabilities: vec![Capability::Pair],
        }
    }

    /// mDNS register → browse roundtrip on loopback. Needs multicast loopback;
    /// ignored where the sandbox blocks it.
    #[tokio::test]
    async fn mdns_register_browse_roundtrip() {
        let info = test_info("testbox");
        let short: String = info.id.to_string().chars().take(8).collect();
        let instance = format!("testbox-{short}");
        let _reg = match register_mdns(&instance, QUIC_PORT, &info) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("SKIP: mdns register blocked: {e}");
                return;
            }
        };
        // Give the responder a moment to announce.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let found = browse_mdns_peers(6, None).await.unwrap();
        assert!(
            found.iter().any(|d| d.info.id == info.id
                && d.info.name == "testbox"
                && !d.addresses.is_empty()
                && d.addresses.iter().all(|a| a.ends_with(":53536"))),
            "expected {instance} in {found:?}"
        );
        // Own-id filter hides us.
        let filtered = browse_mdns(2, Some(info.id)).await.unwrap();
        assert!(!filtered.iter().any(|d| d.id == info.id));
    }

    /// Announce + listen loops see each other on loopback, then go quiet.
    #[tokio::test]
    async fn announce_listen_loopback() {
        use crate::presence::PresenceTracker;
        use std::sync::Arc;

        let a = test_info("loop-a");
        let b = test_info("loop-b");
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

        // One listener (binds DISCOVERY_PORT); two announcers.
        let tracker = Arc::new(PresenceTracker::with_expiry(
            None,
            std::time::Duration::from_secs(60),
        ));
        let listen = tokio::spawn(listen_loop_udp(tracker.clone(), stop_rx.clone()));
        let ann_a = tokio::spawn(announce_loop(
            a.clone(),
            std::time::Duration::from_millis(100),
            stop_rx.clone(),
        ));
        let ann_b = tokio::spawn(announce_loop(
            b.clone(),
            std::time::Duration::from_millis(100),
            stop_rx.clone(),
        ));
        // Broadcast goes to 255.255.255.255 — loopback receipt needs ~1s.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let snap = tracker.snapshot().unwrap();
        assert!(
            snap.iter().any(|d| d.info.id == a.id),
            "expected loop-a in {snap:?}"
        );
        assert!(
            snap.iter().any(|d| d.info.id == b.id),
            "expected loop-b in {snap:?}"
        );
        stop_tx.send(true).unwrap();
        listen.await.unwrap().unwrap();
        ann_a.await.unwrap();
        ann_b.await.unwrap();
    }
}
