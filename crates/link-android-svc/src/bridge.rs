//! JNI bridge called from Kotlin (`MainActivity`, `LinkService`).
//!
//! Kotlin declarations must match:
//! ```kotlin
//! // MainActivity:
//! external fun nativeInit(): Int
//! external fun nativeLocalIdentityJson(deviceName: String, filesDir: String): String
//! external fun nativePingJson(peer: String, deviceName: String): String  // plaintext (Phase 1a)
//! external fun nativeTlsPingJson(peer: String, fingerprint: String, filesDir: String): String
//! external fun nativePairPreview(peer: String): String
//! // LinkService:
//! external fun nativeStartTlsServer(filesDir: String, port: Int, deviceName: String): String
//! external fun nativeStopTlsServer(): Int
//! external fun nativeOnNotification(filesDir: String, id: String, title: String, body: String): Int
//! ```
//! JSON-returning fns answer `{"ok":true,...}` or `{"ok":false,"error":"..."}`.

use jni::objects::{JClass, JString};
use jni::sys::{jint, jstring};
use jni::JNIEnv;

/// `MainActivity.nativeInit()` — verifies the `.so` loaded. Returns 0 on success.
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativeInit(
    mut env: JNIEnv,
    _cls: JClass,
) -> jint {
    log::info!("nativeInit called");
    let _ = &mut env;
    0
}

/// `MainActivity.nativeLocalIdentityJson(name, filesDir)` — persisted identity as JSON.
/// On error returns an empty string; real error channels land in Phase 1.
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativeLocalIdentityJson(
    mut env: JNIEnv,
    _cls: JClass,
    name: JString,
    files_dir: JString,
) -> jstring {
    let dir: String = env
        .get_string(&files_dir)
        .map(|s| s.into())
        .unwrap_or_default();
    let device_name: String = env
        .get_string(&name)
        .map(|s| s.into())
        .unwrap_or_else(|_| "android".to_string());

    let json = match crate::server::local_identity(&dir, link_core::QUIC_PORT, &device_name) {
        Ok(info) => serde_json::to_string(&info).unwrap_or_default(),
        Err(_) => String::new(),
    };
    env.new_string(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `MainActivity.nativePingJson(peer, name)` — blocking TCP ping, called from a
/// background thread in Kotlin. Never call on the UI thread (5s timeout).
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativePingJson(
    mut env: JNIEnv,
    _cls: JClass,
    peer: JString,
    name: JString,
) -> jstring {
    let peer: String = env.get_string(&peer).map(|s| s.into()).unwrap_or_default();
    let device_name: String = env
        .get_string(&name)
        .map(|s| s.into())
        .unwrap_or_else(|_| "android".to_string());
    let json = crate::ping_blocking(&peer, &device_name);
    env.new_string(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `MainActivity.nativeTlsPingJson(peer, fingerprint, filesDir)` — pinned TLS ping.
/// Call from a background thread (5s timeouts).
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativeTlsPingJson(
    mut env: JNIEnv,
    _cls: JClass,
    peer: JString,
    fingerprint: JString,
    files_dir: JString,
) -> jstring {
    let peer: String = env.get_string(&peer).map(|s| s.into()).unwrap_or_default();
    let fp: String = env
        .get_string(&fingerprint)
        .map(|s| s.into())
        .unwrap_or_default();
    let dir: String = env
        .get_string(&files_dir)
        .map(|s| s.into())
        .unwrap_or_default();
    let json = crate::tls_ping_blocking(&peer, &fp, &dir);
    env.new_string(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `MainActivity.nativePairPreview(peer)` — fetch peer fingerprint (untrusted).
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativePairPreview(
    mut env: JNIEnv,
    _cls: JClass,
    peer: JString,
) -> jstring {
    let peer: String = env.get_string(&peer).map(|s| s.into()).unwrap_or_default();
    let json = crate::pair_preview_blocking(&peer);
    env.new_string(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `LinkService.nativeStartTlsServer(filesDir, port, name)` — idempotent start.
/// Returns `{"ok":true,"fingerprint":"..."}` or `{"ok":false,"error":"..."}`.
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeStartTlsServer(
    mut env: JNIEnv,
    _cls: JClass,
    files_dir: JString,
    port: jint,
    name: JString,
) -> jstring {
    let dir: String = env
        .get_string(&files_dir)
        .map(|v| v.into())
        .unwrap_or_default();
    let device_name: String = env
        .get_string(&name)
        .map(|v| v.into())
        .unwrap_or_else(|_| "android".to_string());
    let json = match crate::server::start(&dir, port as u16, &device_name) {
        Ok(fp) => format!(r#"{{"ok":true,"fingerprint":"{fp}"}}"#),
        Err(e) => format!(r#"{{"ok":false,"error":"{e:#}"}}"#),
    };
    env.new_string(json)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `LinkService.nativeStopTlsServer()` — shutdown + join. Always returns 0.
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeStopTlsServer(
    _env: JNIEnv,
    _cls: JClass,
) -> jint {
    crate::server::stop();
    0
}

/// `LinkNotifications.nativeOnNotification(...)` — journal the notification to disk.
/// `filesDir` is passed from Kotlin so Rust never guesses Android paths.
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkNotifications_nativeOnNotification(
    mut env: JNIEnv,
    _cls: JClass,
    files_dir: JString,
    id: JString,
    title: JString,
    body: JString,
) -> jint {
    let dir: String = env
        .get_string(&files_dir)
        .map(|v| v.into())
        .unwrap_or_default();
    let id: String = env.get_string(&id).map(|v| v.into()).unwrap_or_default();
    let title: String = env.get_string(&title).map(|v| v.into()).unwrap_or_default();
    let body: String = env.get_string(&body).map(|v| v.into()).unwrap_or_default();
    let entry = format!("{id}|{title}|{body}");
    match crate::journal::append_line(&dir, "notifications.log", &entry) {
        Ok(()) => 0,
        Err(e) => {
            log::error!("journal append failed: {e:#}");
            -1
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkNotifications_nativeSendNotification(
    mut env: JNIEnv,
    _cls: JClass,
    peer_id: JString,
    address: JString,
    id: JString,
    app_name: JString,
    title: JString,
    body: JString,
) -> jstring {
    let get = |env: &mut JNIEnv, value: &JString| {
        env.get_string(value)
            .map(|value| value.into())
            .unwrap_or_default()
    };
    let peer_id = get(&mut env, &peer_id);
    let address = get(&mut env, &address);
    let id = get(&mut env, &id);
    let app_name = get(&mut env, &app_name);
    let title = get(&mut env, &title);
    let body = get(&mut env, &body);
    let result = crate::server::send_notification(peer_id, address, id, app_name, title, body)
        .map(|_| serde_json::json!({"ok":true}));
    json_result(env, result)
}

/// The foreground service advertises the same identity as MainActivity.
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeLocalIdentityJson(
    env: JNIEnv,
    cls: JClass,
    name: JString,
    files_dir: JString,
) -> jstring {
    Java_org_linktolinux_app_MainActivity_nativeLocalIdentityJson(env, cls, name, files_dir)
}

fn json_result(env: JNIEnv, result: anyhow::Result<serde_json::Value>) -> jstring {
    let value = result.unwrap_or_else(|e| serde_json::json!({"ok":false,"error":e.to_string()}));
    env.new_string(value.to_string())
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativePairBegin(
    mut env: JNIEnv,
    _cls: JClass,
    peer: JString,
) -> jstring {
    let peer: String = env.get_string(&peer).map(|s| s.into()).unwrap_or_default();
    let result = crate::server::begin_pair(peer)
        .map(|pairing| serde_json::json!({"ok":true,"pairing":pairing}));
    json_result(env, result)
}
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativePairSessions(
    env: JNIEnv,
    _cls: JClass,
) -> jstring {
    let result =
        (|| Ok(serde_json::json!({"ok":true,"sessions":crate::server::manager()?.sessions()?})))();
    json_result(env, result)
}
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativePairConfirm(
    mut env: JNIEnv,
    _cls: JClass,
    session: JString,
    code: JString,
    accept: jni::sys::jboolean,
) -> jstring {
    let session: String = env
        .get_string(&session)
        .map(|s| s.into())
        .unwrap_or_default();
    let code: String = env.get_string(&code).map(|s| s.into()).unwrap_or_default();
    let result = (|| {
        Ok(
            serde_json::json!({"ok":true,"pairing":crate::server::manager()?.confirm(&session,&code,accept!=0)?}),
        )
    })();
    json_result(env, result)
}
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativePairsJson(
    env: JNIEnv,
    _cls: JClass,
) -> jstring {
    let result =
        (|| Ok(serde_json::json!({"ok":true,"pairings":crate::server::manager()?.pairs()?})))();
    json_result(env, result)
}
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativeUnpair(
    mut env: JNIEnv,
    _cls: JClass,
    id: JString,
) -> jstring {
    let id: String = env.get_string(&id).map(|s| s.into()).unwrap_or_default();
    let result = (|| {
        crate::server::manager()?.unpair(&id.parse()?)?;
        Ok(serde_json::json!({"ok":true}))
    })();
    json_result(env, result)
}
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativeClipboardEnabled(
    _env: JNIEnv,
    _cls: JClass,
    enabled: jni::sys::jboolean,
) -> jint {
    match crate::server::set_clipboard_enabled(enabled != 0) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativeClipboardStatus(
    env: JNIEnv,
    _cls: JClass,
) -> jstring {
    json_result(
        env,
        crate::server::clipboard_enabled()
            .map(|enabled| serde_json::json!({"ok":true,"enabled":enabled})),
    )
}
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativeClipboardTake(
    env: JNIEnv,
    _cls: JClass,
) -> jstring {
    json_result(
        env,
        (|| Ok(serde_json::json!({"ok":true,"clipboard":crate::server::take_clipboard()?})))(),
    )
}

/// The foreground service shares the same Rust pair manager as MainActivity.
#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeClipboardStatus(
    env: JNIEnv,
    cls: JClass,
) -> jstring {
    Java_org_linktolinux_app_MainActivity_nativeClipboardStatus(env, cls)
}

#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeClipboardEnabled(
    env: JNIEnv,
    cls: JClass,
    enabled: jni::sys::jboolean,
) -> jint {
    Java_org_linktolinux_app_MainActivity_nativeClipboardEnabled(env, cls, enabled)
}

#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeClipboardTake(
    env: JNIEnv,
    cls: JClass,
) -> jstring {
    Java_org_linktolinux_app_MainActivity_nativeClipboardTake(env, cls)
}

#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeGalleryTake(
    env: JNIEnv,
    _cls: JClass,
) -> jstring {
    json_result(
        env,
        (|| Ok(serde_json::json!({"ok":true,"gallery":crate::server::take_gallery()?})))(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeGallerySend(
    mut env: JNIEnv,
    _cls: JClass,
    id: JString,
    address: JString,
    packet: JString,
) -> jstring {
    let id: String = env.get_string(&id).map(|v| v.into()).unwrap_or_default();
    let address: String = env
        .get_string(&address)
        .map(|v| v.into())
        .unwrap_or_default();
    let packet: String = env
        .get_string(&packet)
        .map(|v| v.into())
        .unwrap_or_default();
    json_result(
        env,
        crate::server::send_gallery(id, address, packet).map(|_| serde_json::json!({"ok":true})),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeMessagesTake(
    env: JNIEnv,
    _cls: JClass,
) -> jstring {
    json_result(
        env,
        (|| Ok(serde_json::json!({"ok":true,"messages":crate::server::take_messages()?})))(),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_LinkService_nativeMessagesSend(
    mut env: JNIEnv,
    _cls: JClass,
    id: JString,
    address: JString,
    packet: JString,
) -> jstring {
    let id: String = env.get_string(&id).map(|v| v.into()).unwrap_or_default();
    let address: String = env
        .get_string(&address)
        .map(|v| v.into())
        .unwrap_or_default();
    let packet: String = env
        .get_string(&packet)
        .map(|v| v.into())
        .unwrap_or_default();
    json_result(
        env,
        crate::server::send_messages(id, address, packet).map(|_| serde_json::json!({"ok":true})),
    )
}

#[no_mangle]
pub extern "system" fn Java_org_linktolinux_app_MainActivity_nativeClipboardSend(
    mut env: JNIEnv,
    _cls: JClass,
    id: JString,
    address: JString,
    mime: JString,
    data_b64: JString,
) -> jstring {
    let id: String = env.get_string(&id).map(|v| v.into()).unwrap_or_default();
    let address: String = env
        .get_string(&address)
        .map(|v| v.into())
        .unwrap_or_default();
    let mime: String = env.get_string(&mime).map(|v| v.into()).unwrap_or_default();
    let data_b64: String = env
        .get_string(&data_b64)
        .map(|v| v.into())
        .unwrap_or_default();
    let result = crate::server::send_clipboard(id, address, mime, data_b64)
        .map(|_| serde_json::json!({"ok":true}));
    json_result(env, result)
}
