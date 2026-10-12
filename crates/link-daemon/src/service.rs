//! Daemon run-loop: Phase 1b TLS ping server + pairing + discovery.

use link_core::device::Capability;
use link_core::pairing::{JsonFileStore, PairingStore};
use link_core::platform::{LinuxPlatform, OsPlatform};
use link_core::{DeviceId, DeviceInfo, PROTOCOL_VERSION};
use rand::RngCore;

fn data_dir() -> anyhow::Result<std::path::PathBuf> {
    Ok(LinuxPlatform.data_dir()?)
}

fn load_or_create_device_id(dir: &std::path::Path) -> anyhow::Result<DeviceId> {
    let path = dir.join("device.json");
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(id) = serde_json::from_slice::<DeviceId>(&bytes) {
            return Ok(id);
        }
    }
    let id = DeviceId::generate();
    std::fs::create_dir_all(dir).ok();
    if let Ok(bytes) = serde_json::to_vec_pretty(&id) {
        std::fs::write(&path, bytes).ok();
    }
    Ok(id)
}

fn store(dir: &std::path::Path) -> JsonFileStore {
    JsonFileStore::new(dir.join("paired.json"))
}

fn local_info(name: &str, id: DeviceId) -> DeviceInfo {
    DeviceInfo {
        id,
        name: name.to_string(),
        os: "linux".to_string(),
        protocol_version: PROTOCOL_VERSION,
        quic_port: link_core::QUIC_PORT,
        pubkey_fingerprint: "sha256:see-cert.der".to_string(),
        capabilities: vec![Capability::Pair],
    }
}

pub async fn serve(name: &str, port: u16) -> anyhow::Result<()> {
    let dir = data_dir()?;
    let id = load_or_create_device_id(&dir)?;
    let identity = link_core::tls::load_or_create_identity(&dir)?;
    let mut info = local_info(name, id);
    let tls_listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    let port = tls_listener.local_addr()?.port();
    info.quic_port = port;
    info.pubkey_fingerprint = identity.fingerprint();
    info.validate()?;
    tracing::info!(
        device = %info.id,
        fingerprint = %identity.fingerprint(),
        port,
        data_dir = %dir.display(),
        "link-daemon serving TLS ping (Phase 1b). Ctrl-C to stop."
    );
    let ipc_listener = crate::ipc::bind().await?;
    let manager = std::sync::Arc::new(link_core::pair_session::PairManager::new(
        info.clone(),
        identity.clone(),
        std::sync::Arc::new(store(&dir)),
    ));
    // mDNS registration for LAN discovery (own record; kept alive while serving).
    let short = info.id.to_string().chars().take(8).collect::<String>();
    let instance = format!("{}-{short}", info.name);
    let _mdns = match link_core::discovery::register_mdns(&instance, port, &info) {
        Ok(reg) => {
            tracing::info!("mDNS registered as {instance}");
            Some(reg)
        }
        Err(e) => {
            tracing::warn!("mDNS register failed: {e:#}");
            None
        }
    };
    // Continuous presence: announce + listen on both transports, prune, snapshot.
    let tracker = std::sync::Arc::new(link_core::presence::PresenceTracker::new(Some(info.id)));
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let ipc = tokio::spawn(crate::ipc::serve(
        ipc_listener,
        manager.clone(),
        tracker.clone(),
        stop_rx.clone(),
    ));
    let announce = tokio::spawn(link_core::discovery::announce_loop(
        info.clone(),
        std::time::Duration::from_secs(link_core::presence::ANNOUNCE_INTERVAL_SECS),
        stop_rx.clone(),
    ));
    let listen = tokio::spawn(link_core::discovery::listen_loop_udp(
        tracker.clone(),
        stop_rx.clone(),
    ));
    let browse = tokio::spawn(link_core::discovery::browse_loop_mdns(
        tracker.clone(),
        Some(info.id),
        stop_rx.clone(),
    ));
    let maintain = tokio::spawn(maintain_presence(
        dir.clone(),
        tracker.clone(),
        stop_rx.clone(),
    ));
    let notifications = tokio::spawn(forward_notifications(manager.clone(), stop_rx.clone()));
    // TLS server runs until Ctrl-C; then stop the background tasks.
    let tls_result = link_core::tls::tls_serve_with_pairing_until(
        info.id,
        tls_listener,
        link_core::tls::pairing_server_config(&identity)?,
        Some(manager),
        async {
            let _ = tokio::signal::ctrl_c().await;
        },
    )
    .await;
    let _ = stop_tx.send(true);
    let _ = tokio::join!(announce, listen, browse, maintain, ipc, notifications);
    tls_result?;
    Ok(())
}

/// Show Android notifications from the background daemon so delivery continues
/// when the desktop window is closed.
async fn forward_notifications(
    manager: std::sync::Arc<link_core::pair_session::PairManager>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            _ = stop.wait_for(|s| *s) => break,
        }
        let Ok(Some(notification)) = manager.take_notification() else {
            continue;
        };
        let sender = manager
            .paired_device(&notification.from)
            .ok()
            .flatten()
            .map(|peer| peer.name)
            .unwrap_or_else(|| "Android device".into());
        let summary = if notification.title.trim().is_empty() {
            format!("{} · {}", sender, notification.app_name)
        } else {
            format!("{} · {}", sender, notification.title)
        };
        let body = if notification.body.trim().is_empty() {
            notification.app_name
        } else {
            notification.body
        };
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        notification.from.hash(&mut hasher);
        notification.id.hash(&mut hasher);
        let notification_id = hasher.finish() as u32;
        let result = tokio::task::spawn_blocking(move || {
            let mut toast = notify_rust::Notification::new();
            toast
                .appname("Link to Linux")
                .summary(&summary)
                .body(&body)
                .id(notification_id)
                .show()
        })
        .await;
        match result {
            Err(error) => tracing::warn!("notification worker failed: {error}"),
            Ok(Err(error)) => {
                tracing::debug!("desktop notification service rejected notification: {error}")
            }
            Ok(Ok(_)) => {}
        }
    }
}

/// Prune expired peers every 5s; persist `peers.json` on membership change.
async fn maintain_presence(
    dir: std::path::PathBuf,
    tracker: std::sync::Arc<link_core::presence::PresenceTracker>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut last = String::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            _ = stop.wait_for(|s| *s) => break,
        }
        if let Ok(lost) = tracker.prune() {
            for id in &lost {
                tracing::info!("peer lost: {id}");
            }
        }
        match tracker.snapshot() {
            Ok(devices) => {
                let doc = serde_json::json!({
                    "devices": devices,
                });
                let text = doc.to_string();
                if text != last {
                    last = text.clone();
                    let path = dir.join("peers.json");
                    if let Err(e) = std::fs::write(&path, text) {
                        tracing::warn!("peers.json write failed: {e}");
                    }
                }
            }
            Err(e) => tracing::warn!("presence snapshot failed: {e:#}"),
        }
    }
}

/// Instant nearby-device list from the serving daemon's `peers.json` cache.
/// Empty (not an error) when no daemon is serving. Use `discover` for an
/// active blocking scan instead.
pub async fn devices(_json: bool) -> anyhow::Result<()> {
    println!(
        "{}",
        crate::ipc::request(&crate::ipc::Request::Devices).await?
    );
    Ok(())
}
pub async fn discover(timeout_secs: u64, _json: bool) -> anyhow::Result<()> {
    if let Ok(value) = crate::ipc::request(&crate::ipc::Request::Devices).await {
        println!("{value}");
        return Ok(());
    }
    let dir = data_dir()?;
    let own_id = load_or_create_device_id(&dir)?;
    let (udp, mdns) = tokio::join!(
        link_core::discovery::listen_for_peers(timeout_secs),
        link_core::discovery::browse_mdns_peers(timeout_secs, Some(own_id))
    );
    let mut all = std::collections::BTreeMap::new();
    for peer in udp
        .unwrap_or_default()
        .into_iter()
        .chain(mdns.unwrap_or_default())
    {
        if peer.info.id != own_id {
            all.entry(peer.info.id.to_string()).or_insert(peer);
        }
    }
    println!(
        "{}",
        serde_json::json!({"devices":all.into_values().collect::<Vec<_>>() })
    );
    Ok(())
}
pub async fn pair(peer: &str, _json: bool) -> anyhow::Result<()> {
    println!(
        "{}",
        crate::ipc::request(&crate::ipc::Request::Begin { peer: peer.into() }).await?
    );
    Ok(())
}
pub async fn sessions() -> anyhow::Result<()> {
    println!(
        "{}",
        crate::ipc::request(&crate::ipc::Request::Sessions).await?
    );
    Ok(())
}
pub async fn confirm(session: String, code: String, reject: bool) -> anyhow::Result<()> {
    println!(
        "{}",
        crate::ipc::request(&crate::ipc::Request::Confirm {
            session,
            code,
            accept: !reject
        })
        .await?
    );
    Ok(())
}

pub async fn ping(peer: &str, fingerprint: Option<&str>) -> anyhow::Result<()> {
    let dir = data_dir()?;
    let id = load_or_create_device_id(&dir)?;
    let fp: String = match fingerprint {
        Some(f) => f.to_string(),
        None => {
            let all = store(&dir).list()?;
            // Prefer the entry recorded for this peer address.
            if let Some(e) = all
                .iter()
                .find(|d| d.address.as_deref() == Some(peer) || d.name == peer)
            {
                e.cert_fingerprint.clone()
            } else if all.len() == 1 {
                all[0].cert_fingerprint.clone()
            } else {
                anyhow::bail!(
                    "no fingerprint: pass --fingerprint, or `pair --peer {peer}` first ({} stored pairing(s))",
                    all.len()
                );
            }
        }
    };
    let nonce = rand::thread_rng().next_u64();
    let pong =
        link_core::tls::tls_ping(peer, &link_core::codec::Ping { nonce, from: id }, &fp).await?;
    println!(
        "{}",
        serde_json::json!({ "peer": peer, "nonce": nonce, "pong_nonce": pong.nonce, "pong_from": pong.from.to_string(), "pinned": fp })
    );
    if pong.nonce != nonce {
        anyhow::bail!("nonce mismatch");
    }
    Ok(())
}

pub async fn pairs(_json: bool) -> anyhow::Result<()> {
    println!(
        "{}",
        crate::ipc::request(&crate::ipc::Request::Pairs).await?
    );
    Ok(())
}
pub async fn unpair(id: &str) -> anyhow::Result<()> {
    println!(
        "{}",
        crate::ipc::request(&crate::ipc::Request::Unpair { id: id.into() }).await?
    );
    Ok(())
}
