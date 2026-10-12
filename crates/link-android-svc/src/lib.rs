//! Android glue: Kotlin app loads `liblink_android_svc.so` via JNI and
//! calls into `link-core` for identity / pairing / journaling.
//!
//! Design: Gradle owns the APK (activities, services, permissions).
//! Rust compiles to a `cdylib` via `cargo-ndk` and is loaded with
//! `System.loadLibrary("link_android_svc")`. No `NativeActivity` —
//! standard `AppCompatActivity` + `ForegroundService` in Kotlin.

pub mod bridge;
pub mod journal;
pub mod server;

use link_core::device::Capability;
use link_core::{DeviceId, DeviceInfo, PROTOCOL_VERSION, QUIC_PORT};

/// Build our own identity packet once Android gives us a device name.
/// Real cert fingerprint comes from the keystore in Phase 3.
pub fn local_identity(device_name: &str) -> anyhow::Result<DeviceInfo> {
    let info = DeviceInfo {
        id: DeviceId::generate(),
        name: device_name.to_string(),
        os: "android".to_string(),
        protocol_version: PROTOCOL_VERSION,
        quic_port: QUIC_PORT,
        pubkey_fingerprint: "sha256:phase0-placeholder".to_string(),
        capabilities: vec![
            Capability::Pair,
            Capability::Notify,
            Capability::File,
            Capability::Clip,
        ],
    };
    info.validate()?;
    Ok(info)
}

/// Blocking ping helper used by the JNI bridge (spawns a current-thread runtime).
/// Returns a JSON string: success or `{"ok":false,"error":"..."}`.
pub fn ping_blocking(peer: &str, device_name: &str) -> String {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return format!(r#"{{"ok":false,"error":"rt: {e}"}}"#),
    };
    let peer = peer.to_string();
    let device_name = device_name.to_string();
    rt.block_on(async move {
        let from = match local_identity(&device_name) {
            Ok(info) => info.id,
            Err(e) => return format!(r#"{{"ok":false,"error":"identity: {e:#}"}}"#),
        };
        // Stable nonce per call is fine for Phase 1a proof.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        match link_core::transport::tcp_ping(&peer, &link_core::codec::Ping { nonce, from }).await {
            Ok(pong) => format!(
                r#"{{"ok":true,"peer":"{peer}","nonce":{nonce},"pong_nonce":{},"pong_from":"{}"}}"#,
                pong.nonce, pong.from
            ),
            Err(e) => format!(r#"{{"ok":false,"peer":"{peer}","error":"{e:#}"}}"#),
        }
    })
}

/// TLS ping with pinned fingerprint (Phase 1b). Fails closed on mismatch.
pub fn tls_ping_blocking(peer: &str, expected_fp: &str, files_dir: &str) -> String {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return format!(r#"{{"ok":false,"error":"rt: {e}"}}"#),
    };
    let peer = peer.to_string();
    let expected_fp = expected_fp.to_string();
    let files_dir = files_dir.to_string();
    rt.block_on(async move {
        let from = match server::local_identity(&files_dir, QUIC_PORT, "android") {
            Ok(info) => info.id,
            Err(e) => {
                return serde_json::json!({"ok": false, "error": format!("identity: {e:#}")})
                    .to_string()
            }
        };
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        match link_core::tls::tls_ping(&peer, &link_core::codec::Ping { nonce, from }, &expected_fp)
            .await
        {
            Ok(pong) => serde_json::json!({
                "ok": true, "peer": peer, "nonce": nonce,
                "pong_nonce": pong.nonce, "pong_from": pong.from.to_string(),
            })
            .to_string(),
            Err(e) => {
                serde_json::json!({"ok": false, "peer": peer, "error": e.to_string()}).to_string()
            }
        }
    })
}

/// Pairing preview: fetch peer fingerprint without trusting. Returns fp or error JSON.
pub fn pair_preview_blocking(peer: &str) -> String {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return format!(r#"{{"ok":false,"error":"rt: {e}"}}"#),
    };
    rt.block_on(async move {
        match link_core::tls::fetch_fingerprint(peer).await {
            Ok(fp) => serde_json::json!({"ok": true, "peer": peer, "fingerprint": fp}).to_string(),
            Err(e) => {
                serde_json::json!({"ok": false, "peer": peer, "error": e.to_string()}).to_string()
            }
        }
    })
}
/// Plain-C init hook (useful for `adb shell` smoke tests, not used by Kotlin).
#[cfg_attr(target_os = "android", no_mangle)]
pub extern "C" fn link_android_init() -> i32 {
    log::info!("link-android-svc init ok");
    0
}
