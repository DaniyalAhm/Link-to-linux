//! Background TLS server for Android (Phase 1b).
//!
//! Kotlin's `LinkService` calls `nativeStartTlsServer` once on a background
//! thread; Rust owns a dedicated OS thread + current-thread runtime running
//! [`link_core::tls::tls_serve_until`]. `nativeStopTlsServer` fires the
//! shutdown channel. Identity (`cert.der`/`key.der`) + `device.json` live in
//! the app's `filesDir` passed from Kotlin — Rust never guesses paths.

use link_core::DeviceId;
use std::sync::{Arc, Mutex, OnceLock};

struct Running {
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: std::thread::JoinHandle<()>,
    runtime: tokio::runtime::Handle,
    manager: Arc<link_core::pair_session::PairManager>,
}

static SERVER: OnceLock<Mutex<Option<Running>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Running>> {
    SERVER.get_or_init(|| Mutex::new(None))
}

/// Start the TLS server on `port`. Returns our cert fingerprint on success.
/// A second start while running returns an error.
pub fn start(files_dir: &str, port: u16, device_name: &str) -> anyhow::Result<String> {
    let mut guard = slot().lock().unwrap();
    if let Some(running) = guard.as_ref() {
        return Ok(running.manager.identity.fingerprint());
    }
    let dir = std::path::PathBuf::from(files_dir);
    std::fs::create_dir_all(&dir)?;
    let info = local_identity(files_dir, port, device_name)?;
    let id = info.id;
    let identity = link_core::tls::load_or_create_identity(&dir)?;
    let fp = identity.fingerprint();
    let manager = Arc::new(link_core::pair_session::PairManager::new(
        info,
        identity.clone(),
        Arc::new(link_core::pairing::JsonFileStore::new(
            dir.join("paired.json"),
        )),
    ));
    let background_manager = manager.clone();
    let name = device_name.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let handle = std::thread::Builder::new()
        .name("link-tls-serve".into())
        .spawn(move || {
            log::info!("tls server starting on {port} as {name} id={id}");
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("server runtime failed: {e}")));
                    return;
                }
            };
            let rt_handle = rt.handle().clone();
            rt.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await
                {
                    Ok(l) => l,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("server bind failed: {e}")));
                        return;
                    }
                };
                let cfg = match link_core::tls::pairing_server_config(&identity) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("server config failed: {e:#}")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(rt_handle));
                if let Err(e) = link_core::tls::tls_serve_with_pairing_until(
                    id,
                    listener,
                    cfg,
                    Some(background_manager),
                    async move {
                        let _ = rx.await;
                    },
                )
                .await
                {
                    log::error!("server error: {e:#}");
                }
            });
        })
        .map_err(|e| anyhow::anyhow!("spawn server thread: {e}"))?;
    let runtime = match ready_rx.recv() {
        Ok(Ok(handle)) => handle,
        result => {
            let _ = handle.join();
            anyhow::bail!("TLS server failed to start: {result:?}");
        }
    };
    *guard = Some(Running {
        shutdown: tx,
        handle,
        runtime,
        manager,
    });
    Ok(fp)
}

/// Stop the server, waiting for the thread. No-op when not running.
pub fn stop() {
    let running = slot().lock().unwrap().take();
    if let Some(r) = running {
        let _ = r.shutdown.send(());
        let _ = r.handle.join();
    }
}

/// Discovery and the TLS listener use the same persisted identity.
pub fn local_identity(
    files_dir: &str,
    port: u16,
    name: &str,
) -> anyhow::Result<link_core::DeviceInfo> {
    let dir = std::path::Path::new(files_dir);
    std::fs::create_dir_all(dir)?;
    let path = dir.join("device.json");
    let id = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<DeviceId>(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = DeviceId::generate();
            std::fs::write(&path, serde_json::to_vec(&id)?)?;
            id
        }
        Err(e) => return Err(e.into()),
    };
    let identity = link_core::tls::load_or_create_identity(dir)?;
    let info = link_core::DeviceInfo {
        id,
        name: name.to_owned(),
        os: "android".into(),
        protocol_version: link_core::PROTOCOL_VERSION,
        quic_port: port,
        pubkey_fingerprint: identity.fingerprint(),
        capabilities: vec![link_core::Capability::Pair],
    };
    info.validate()?;
    Ok(info)
}

pub fn manager() -> anyhow::Result<Arc<link_core::pair_session::PairManager>> {
    slot()
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.manager.clone())
        .ok_or_else(|| anyhow::anyhow!("Link service is starting; try again in a moment"))
}
pub fn begin_pair(peer: String) -> anyhow::Result<link_core::pair_session::PairSession> {
    let (runtime, manager) = {
        let guard = slot().lock().unwrap();
        let running = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Start the link service first"))?;
        (running.runtime.clone(), running.manager.clone())
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    runtime.spawn(async move {
        let _ = tx.send(manager.begin(peer).await);
    });
    Ok(rx.recv_timeout(std::time::Duration::from_secs(16))??)
}

pub fn set_clipboard_enabled(enabled: bool) -> anyhow::Result<()> {
    manager()?.set_clipboard_enabled(enabled);
    Ok(())
}
pub fn clipboard_enabled() -> anyhow::Result<bool> {
    Ok(manager()?.clipboard_enabled())
}
pub fn take_clipboard() -> anyhow::Result<Option<(String, String, String)>> {
    manager()?.take_clipboard().map_err(Into::into)
}
pub fn take_gallery() -> anyhow::Result<Option<link_core::pair_session::GalleryPacket>> {
    manager()?.take_gallery().map_err(Into::into)
}
pub fn take_messages() -> anyhow::Result<Option<link_core::pair_session::MessagesPacket>> {
    manager()?.take_messages().map_err(Into::into)
}
pub fn send_gallery(id: String, address: String, packet_json: String) -> anyhow::Result<()> {
    let id: DeviceId = id.parse()?;
    let packet: link_core::pair_session::GalleryPacket = serde_json::from_str(&packet_json)?;
    let (runtime, manager) = {
        let guard = slot().lock().unwrap();
        let running = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Start the link service first"))?;
        (running.runtime.clone(), running.manager.clone())
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    runtime.spawn(async move {
        let _ = tx.send(manager.send_gallery_at(&id, address, packet).await);
    });
    rx.recv_timeout(std::time::Duration::from_secs(20))??;
    Ok(())
}
pub fn send_messages(id: String, address: String, packet_json: String) -> anyhow::Result<()> {
    let id: DeviceId = id.parse()?;
    let packet: link_core::pair_session::MessagesPacket = serde_json::from_str(&packet_json)?;
    let (runtime, manager) = {
        let guard = slot().lock().unwrap();
        let running = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Start the Link service first"))?;
        (running.runtime.clone(), running.manager.clone())
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    runtime.spawn(async move {
        let _ = tx.send(manager.send_messages_at(&id, address, packet).await);
    });
    rx.recv_timeout(std::time::Duration::from_secs(20))??;
    Ok(())
}
pub fn send_clipboard(
    id: String,
    address: String,
    mime: String,
    data_b64: String,
) -> anyhow::Result<()> {
    let id: DeviceId = id.parse()?;
    let (runtime, manager) = {
        let guard = slot().lock().unwrap();
        let running = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Start the link service first"))?;
        (running.runtime.clone(), running.manager.clone())
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    runtime.spawn(async move {
        let _ = tx.send(
            manager
                .send_clipboard_at(&id, address, mime, data_b64)
                .await,
        );
    });
    rx.recv_timeout(std::time::Duration::from_secs(12))??;
    Ok(())
}

pub fn send_notification(
    id: String,
    address: String,
    notification_key: String,
    app_name: String,
    title: String,
    body: String,
) -> anyhow::Result<()> {
    let id: DeviceId = id.parse()?;
    let (runtime, manager) = {
        let guard = slot().lock().unwrap();
        let running = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Link service is starting; try again in a moment"))?;
        (running.runtime.clone(), running.manager.clone())
    };
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    runtime.spawn(async move {
        let _ = tx.send(
            manager
                .send_notification_at(&id, address, notification_key, app_name, title, body)
                .await,
        );
    });
    rx.recv_timeout(std::time::Duration::from_secs(12))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_identity_matches_persisted_tls_identity() {
        let dir = std::env::temp_dir().join(format!("link-identity-{}", DeviceId::generate()));
        let path = dir.to_str().unwrap();
        let first = local_identity(path, 53536, "phone").unwrap();
        let second = local_identity(path, 53537, "renamed").unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.pubkey_fingerprint, second.pubkey_fingerprint);
        assert_eq!(second.quic_port, 53537);
        assert_eq!(
            first.pubkey_fingerprint,
            link_core::tls::load_or_create_identity(&dir)
                .unwrap()
                .fingerprint()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
