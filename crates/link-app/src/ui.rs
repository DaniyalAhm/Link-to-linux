use crate::daemon_client::DaemonClient;
use dioxus::prelude::*;
use link_core::discovery::DiscoveredDevice;
use link_core::pair_session::{PairSession, PairStatus};
use link_core::pairing::PairedDevice;
use link_core::DeviceInfo;
use std::collections::{HashMap, HashSet};

async fn read_linux_clipboard() -> Option<(String, String)> {
    use tokio::io::AsyncReadExt;
    let types = tokio::process::Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .await
        .ok()?;
    if !types.status.success() {
        return None;
    }
    let offered = String::from_utf8_lossy(&types.stdout);
    let images = ["image/png", "image/jpeg", "image/webp"];
    let mime = images
        .iter()
        .find(|mime| offered.lines().any(|line| line.trim() == **mime))
        .copied();
    let selected = if let Some(mime) = mime {
        (mime, mime.to_string(), 5 * 1024 * 1024)
    } else {
        let text_type = offered
            .lines()
            .map(str::trim)
            .find(|line| *line == "text/plain" || line.starts_with("text/plain;"))?;
        ("text/plain", text_type.to_string(), 32 * 1024)
    };
    let (wire_mime, offered_mime, max_bytes) = selected;
    let mut child = tokio::process::Command::new("wl-paste")
        .args(["--type", &offered_mime, "--no-newline"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let mut output = Vec::new();
    let stdout = child.stdout.take()?;
    let read = stdout
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut output)
        .await;
    if read.is_err() || output.len() > max_bytes {
        let _ = child.kill().await;
        return None;
    }
    if !child.wait().await.ok()?.success() {
        return None;
    }
    if wire_mime == "text/plain" && std::str::from_utf8(&output).is_err() {
        return None;
    }
    Some((
        wire_mime.into(),
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, output),
    ))
}

async fn set_linux_clipboard(mime: &str, data: &[u8]) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let write_result = if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(data).await
    } else {
        Err(std::io::Error::other("clipboard input unavailable"))
    };
    let exit = child.wait().await;
    match (write_result, exit) {
        (Ok(()), Ok(status)) if status.success() => Ok(()),
        (Err(error), _) => Err(error.to_string()),
        (_, Err(error)) => Err(error.to_string()),
        (_, Ok(status)) => Err(format!("wl-copy exited with {status}")),
    }
}

pub fn launch() {
    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            dioxus::desktop::Config::new().with_window(
                dioxus::desktop::WindowBuilder::new()
                    .with_title("Link to Linux")
                    .with_decorations(false),
            ),
        )
        .launch(App);
}

fn appearance_file() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("link-to-linux").join("appearance.json")
}

fn load_dark_mode() -> bool {
    std::fs::read(appearance_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("dark_mode").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn save_dark_mode(enabled: bool) {
    let path = appearance_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&serde_json::json!({ "dark_mode": enabled })) {
        let _ = std::fs::write(path, bytes);
    }
}

const STYLE: &str = r#"
:root{font-family:Inter,"Segoe UI",system-ui,sans-serif;color:#1c1c1c;background:#f5f5f7;font-synthesis:none;--text:#202124;--muted:#666b73;--line:#e4e5e8;--surface:#fff;--blue:#0067c0;--blue-light:#e8f2fc;--green:#16845b;--canvas:#f5f5f7}*{box-sizing:border-box}body{margin:0;background:var(--canvas);color:var(--text);font-size:14px}.window-chrome{height:40px;display:flex;align-items:center;background:#f5f5f7;border-bottom:1px solid #e4e5e8;user-select:none}.window-drag-region{display:flex;align-items:center;gap:9px;flex:1;height:100%;padding:0 14px;cursor:default}.window-app-mark{height:22px;width:22px;display:grid;place-items:center;border-radius:6px;background:#0878d1;color:#fff;font-size:13px;font-weight:700}.window-title{font-size:12px;color:#565a60;font-weight:600}.window-controls{display:flex;height:100%}.window-control{width:46px;height:100%;padding:0;border:0;border-radius:0;background:transparent;color:#4b4e53;font-size:14px;font-weight:400;box-shadow:none}.window-control:hover{background:#e7e7e9;box-shadow:none}.window-control.close:hover{background:#c42b1c;color:#fff}.app-shell{display:grid;grid-template-columns:244px minmax(0,1fr);min-height:calc(100vh - 40px)}.sidebar{position:sticky;top:0;height:calc(100vh - 40px);padding:24px 14px 18px;background:#f0f0f2;border-right:1px solid #e5e5e7;display:flex;flex-direction:column}.brand{display:flex;align-items:center;gap:11px;padding:3px 10px 24px}.brand-mark{height:34px;width:34px;display:grid;place-items:center;border-radius:10px;background:#0878d1;color:white;font-size:19px;font-weight:700;box-shadow:0 2px 5px #0067c022}.brand-name{font-size:16px;font-weight:650;letter-spacing:-.02em}.side-caption{padding:12px 12px 7px;color:#777b82;font-size:11px;text-transform:uppercase;letter-spacing:.09em;font-weight:700}.nav-list{display:grid;gap:4px}.nav-item{display:flex;align-items:center;gap:12px;width:100%;padding:10px 12px;border:0;border-radius:7px;text-align:left;background:transparent;color:#33363a;font-weight:500}.nav-item:hover{background:#e7e7e9;transform:none}.nav-item.selected{background:#fff;color:#005fb8;box-shadow:0 1px 3px #0000000b}.nav-icon{width:20px;text-align:center;font-size:16px}.sidebar-device{margin-top:auto;padding:14px 11px;border-top:1px solid #dedee1}.sidebar-device .device-title{font-size:12px;color:#6c7076;margin-bottom:8px}.device-indicator{display:flex;align-items:center;gap:8px;font-weight:600}.dot{width:8px;height:8px;border-radius:50%;background:#20a36b}.app-main{min-width:0;padding:0 38px 48px;max-width:1500px;width:100%;margin:0 auto}.topbar{height:76px;display:flex;align-items:center;justify-content:space-between;border-bottom:1px solid var(--line);margin-bottom:28px}.eyebrow{font-size:12px;color:#72767d;margin-bottom:4px}.topbar h1{font-size:23px;letter-spacing:-.025em;margin:0;font-weight:650}.service-badge{display:flex;align-items:center;gap:8px;border:1px solid #d8e9df;background:#f0faf4;color:#176943;border-radius:18px;padding:7px 11px;font-size:12px;white-space:nowrap}.content{max-width:1120px;margin:auto}.muted{color:var(--muted);font-size:13px;line-height:1.55}.section-title{font-size:18px;letter-spacing:-.015em;font-weight:650;margin:26px 0 12px}.subhead{font-size:14px;font-weight:650;margin:0 0 5px}.card{background:var(--surface);border:1px solid var(--line);border-radius:12px;padding:20px;margin:12px 0;box-shadow:0 2px 7px #20212408}.row{display:flex;justify-content:space-between;align-items:center;gap:16px;flex-wrap:wrap}.actions{display:flex;gap:9px;flex-wrap:wrap;align-items:center}button{background:var(--blue);color:white;border:1px solid var(--blue);border-radius:6px;padding:9px 15px;font-size:13px;font-weight:600;cursor:pointer;transition:background .15s,box-shadow .15s}button:hover{background:#005a9e;box-shadow:0 2px 5px #005a9e26;transform:none}button.secondary{background:white;color:#222;border-color:#d2d3d6}button.secondary:hover{background:#f7f7f7;border-color:#aeb2b7}button.danger{background:#fff;color:#b3261e;border-color:#e2b5b2}button.danger:hover{background:#fff5f4}button:disabled{opacity:.45;cursor:default}input{background:white;color:var(--text);border:1px solid #c7c9cc;border-radius:6px;padding:10px 12px;font-size:14px;min-width:min(320px,100%)}input:focus{outline:2px solid #70b8ef;border-color:var(--blue)}.code{font-size:38px;font-weight:700;letter-spacing:9px;margin:15px 0;color:#005fb8}.status{white-space:pre-wrap;padding:11px 13px;border:1px solid #d7e8fa;border-left:3px solid var(--blue);border-radius:7px;background:#f2f8fd}.name{font-weight:650;font-size:15px}.empty{padding:24px;border:1px dashed #c9cbd0;border-radius:10px;color:#6c7076;background:#fff;text-align:center}.hero{display:grid;grid-template-columns:minmax(0,1.2fr) minmax(230px,.8fr);gap:24px;align-items:center;padding:28px 30px;background:linear-gradient(120deg,#fff 0%,#f5f9ff 70%,#eaf4ff 100%);border:1px solid #dce8f3;border-radius:14px;min-height:210px}.hero h2{font-size:26px;letter-spacing:-.035em;margin:0 0 9px}.hero p{max-width:530px}.phone-art{height:160px;display:grid;place-items:center;color:#0572c2}.phone-outline{width:92px;height:150px;border:3px solid #6b9fc9;border-radius:19px;background:linear-gradient(150deg,#eaf6ff,#fff);box-shadow:15px 10px 0 -5px #cfe5f6;display:grid;place-items:center;font-size:29px}.stats{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px;margin:20px 0}.stat{background:#fff;border:1px solid var(--line);border-radius:10px;padding:17px}.stat-value{font-size:23px;font-weight:700;letter-spacing:-.03em}.stat-label{color:var(--muted);font-size:12px;margin-top:4px}.gallery-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(155px,1fr));gap:14px;margin:18px 0}.gallery-tile{overflow:hidden;background:#fff;border:1px solid var(--line);border-radius:9px;transition:box-shadow .15s,transform .15s}.gallery-tile:hover{box-shadow:0 4px 14px #172b4518;transform:translateY(-2px)}.gallery-tile img{width:100%;aspect-ratio:1;object-fit:cover;background:#f0f2f4}.tile-placeholder{aspect-ratio:1;display:grid;place-items:center;color:#90959b;background:#f0f2f4}.gallery-tile .tile-meta{padding:10px}.gallery-tile .tile-meta .name{display:block;font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.gallery-tile button{margin-top:8px;width:100%;padding:7px}.gallery-preview{display:block;max-width:100%;max-height:560px;object-fit:contain;margin:18px auto;border-radius:8px;background:#f0f2f4}.device-card{display:flex;align-items:center;gap:16px}.device-avatar{height:44px;width:44px;display:grid;place-items:center;border-radius:12px;background:#e8f2fc;color:#0067c0;font-size:20px}.details{margin-top:10px}summary{cursor:pointer;color:var(--muted);font-size:12px}code{overflow-wrap:anywhere;font-size:11px;color:#565d66}.home-columns{display:grid;grid-template-columns:1.1fr .9fr;gap:14px}.card-link{padding:0;border:0;background:transparent;color:#005fb8;font:inherit;font-weight:600;cursor:pointer}.card-link:hover{background:transparent;text-decoration:underline;box-shadow:none}.tab-content{animation:fadein .16s ease-out}@keyframes fadein{from{opacity:.7;transform:translateY(3px)}to{opacity:1;transform:none}}@media(max-width:850px){.app-shell{grid-template-columns:70px minmax(0,1fr)}.sidebar{padding:20px 8px}.brand{justify-content:center;padding:2px 0 22px}.brand-name,.side-caption,.nav-label,.sidebar-device{display:none}.nav-item{justify-content:center;padding:12px 4px}.app-main{padding:0 22px 36px}.hero{grid-template-columns:1fr}.phone-art{display:none}}@media(max-width:600px){.app-shell{display:block}.sidebar{position:fixed;z-index:3;top:auto;bottom:0;height:62px;width:100%;padding:6px 10px;border-right:0;border-top:1px solid var(--line)}.brand,.sidebar-device,.side-caption{display:none}.nav-list{grid-template-columns:repeat(5,1fr);gap:2px}.nav-item{display:grid;gap:2px;padding:5px 2px;font-size:10px}.nav-label{display:block;font-size:9px}.nav-icon{width:auto;font-size:16px}.app-main{padding:0 14px 82px}.topbar{height:66px;margin-bottom:18px}.topbar h1{font-size:20px}.stats{grid-template-columns:1fr 1fr}.hero{padding:22px}.hero h2{font-size:23px}.gallery-grid{grid-template-columns:repeat(auto-fill,minmax(125px,1fr));gap:9px}}
"#;

const DARK_STYLE: &str = r#"
.theme-dark{--text:#e5e7eb;--muted:#a1a7b0;--line:#242424;--surface:#050505;--blue:#1683d8;--blue-light:#101820;--green:#47c78a;--canvas:#000;background:#000;color:var(--text);min-height:100vh}
.theme-dark .window-chrome{background:#000;border-color:#242424}.theme-dark .window-title,.theme-dark .window-control{color:#d5d9df}.theme-dark .window-control:hover{background:#202020}.theme-dark .app-shell{background:#000}.theme-dark .sidebar{background:#050505;border-color:#242424}.theme-dark .side-caption,.theme-dark .sidebar-device .device-title,.theme-dark .eyebrow{color:#a1a7b0}.theme-dark .nav-item{color:#d5d9df}.theme-dark .nav-item:hover{background:#181818}.theme-dark .nav-item.selected{background:#202020;color:#8ccaff;box-shadow:none}.theme-dark .sidebar-device{border-color:#242424}.theme-dark .topbar{border-color:#242424}.theme-dark .service-badge{background:#102219;border-color:#21452f;color:#8ee0b2}.theme-dark .card,.theme-dark .stat,.theme-dark .gallery-tile{background:#050505;border-color:#242424;box-shadow:none}.theme-dark .hero{background:linear-gradient(120deg,#050505 0%,#080c10 72%,#0b141b 100%);border-color:#252a30}.theme-dark .phone-outline{background:linear-gradient(150deg,#0b141b,#050505);border-color:#426b8b;box-shadow:15px 10px 0 -5px #121b23}.theme-dark button.secondary{background:#111;color:#e5e7eb;border-color:#383838}.theme-dark button.secondary:hover{background:#1b1b1b;border-color:#626b77}.theme-dark button.danger{background:#1c1111;color:#ffaaa3;border-color:#55312f}.theme-dark input{background:#050505;color:#e5e7eb;border-color:#383838}.theme-dark .status{background:#0b141b;border-color:#263d50;color:#d6eaff}.theme-dark .empty{background:#050505;border-color:#383838;color:#a1a7b0}.theme-dark .tile-placeholder,.theme-dark .gallery-tile img,.theme-dark .gallery-preview{background:#111}.theme-dark .device-avatar{background:#101820;color:#8ccaff}.theme-dark code{color:#bdc5cf}.theme-dark .card-link{color:#8ccaff}.theme-dark .window-app-mark{background:#0878d1}
"#;

const DEVICE_MENU_STYLE: &str = r#"
.device-menu{position:relative;height:100%;display:flex;align-items:center;user-select:none}.device-menu-trigger{height:30px;min-width:0;padding:0 10px;border:1px solid #d4d6da;border-radius:7px;background:#fff;color:#333;font-size:12px;font-weight:600;box-shadow:none}.device-menu-trigger:hover{background:#f7f7f7;box-shadow:none}.device-menu-popup{position:absolute;z-index:10;right:0;top:36px;width:260px;padding:7px;background:#fff;border:1px solid #dedfe3;border-radius:10px;box-shadow:0 8px 28px #0002}.device-menu-heading{padding:8px 9px 6px;color:#73777e;font-size:10px;font-weight:700;letter-spacing:.08em;text-transform:uppercase}.device-menu-item{width:100%;display:flex;align-items:center;gap:9px;padding:9px;border:0;border-radius:6px;background:transparent;color:#24272b;text-align:left;font-size:12px;font-weight:500;box-shadow:none}.device-menu-item:hover,.device-menu-item.selected{background:#eef5fb;color:#005fb8;box-shadow:none}.device-menu-item .dot{flex:none}.device-menu-item .menu-device-name{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.device-menu-item .menu-device-os{color:#777;font-size:10px}.device-menu-divider{height:1px;background:#e5e5e8;margin:5px 2px}.device-menu-manage{color:#005fb8;font-weight:650}.theme-dark .device-menu-trigger{background:#080808;border-color:#383838;color:#e5e7eb}.theme-dark .device-menu-trigger:hover{background:#181818}.theme-dark .device-menu-popup{background:#080808;border-color:#303030;box-shadow:0 8px 28px #0009}.theme-dark .device-menu-heading{color:#a1a7b0}.theme-dark .device-menu-item{color:#e5e7eb}.theme-dark .device-menu-item:hover,.theme-dark .device-menu-item.selected{background:#171d23;color:#8ccaff}.theme-dark .device-menu-item .menu-device-os{color:#a1a7b0}.theme-dark .device-menu-divider{background:#292929}.theme-dark .device-menu-manage{color:#8ccaff}
"#;

const PHOTO_VIEWER_STYLE: &str = r#"
html,body,#main{margin:0;width:100%;height:100%;overflow:hidden;background:#f4f5f7;color:#202124;font-family:Inter,'Segoe UI',system-ui,sans-serif}.viewer{height:100vh;display:flex;flex-direction:column;background:#f4f5f7;color:#202124}.viewer.theme-dark{background:#000;color:#f3f4f6}.viewer-bar{height:54px;flex:none;display:flex;align-items:center;justify-content:space-between;padding:0 18px;background:#fff;border-bottom:1px solid #d7d9dd}.viewer.theme-dark .viewer-bar{background:#080808;border-color:#242424}.viewer-name{font-size:14px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.viewer-hint{font-size:12px;color:#686d75;margin-left:12px}.viewer.theme-dark .viewer-hint{color:#a1a7b0}.viewer-close{padding:7px 12px;border:1px solid #7775;border-radius:7px;background:transparent;color:inherit;cursor:pointer}.viewer-close:hover{background:#7772}.viewer-stage{min-height:0;flex:1;display:grid;place-items:center;padding:18px}.viewer-image{display:block;max-width:100%;max-height:100%;object-fit:contain}
"#;

const MESSAGES_STYLE: &str = r#"
.sms-contact{display:flex;flex-wrap:wrap;gap:8px;align-items:center;justify-content:space-between;text-align:left;width:100%;padding:12px;margin:3px 0}.sms-contact.selected{outline:1px solid #169bdf}.sms-contact span{font-size:12px;color:var(--muted)}.sms-contacts{display:block!important;min-height:160px}.sms-message span{white-space:pre-wrap;overflow-wrap:anywhere}
.sms-page{height:100%;min-height:480px;display:flex;flex-direction:column;gap:16px}.sms-heading{display:flex;justify-content:space-between;align-items:center}.sms-heading h2{margin:0;font-size:22px;font-weight:650;letter-spacing:-.02em}.sms-heading p{margin:5px 0 0;color:var(--muted);font-size:12px}.sms-layout{display:grid;grid-template-columns:minmax(245px,320px) minmax(0,1fr);gap:12px;flex:1;min-height:0;align-items:stretch}.sms-thread-list{min-width:0;overflow:auto;background:var(--surface);border:1px solid var(--line);border-radius:11px;padding:9px}.sms-device-card{padding:0;border:0;border-radius:0;background:transparent;box-shadow:none}.sms-device-card>.row{padding:10px 7px;border-bottom:1px solid var(--line)}.sms-device-card>.row .device-avatar{width:34px;height:34px;border-radius:50%;background:#e8f2fb;color:#1769aa}.sms-device-card .actions{margin-left:auto}.sms-thread-button{display:block;width:100%;margin:3px 0;padding:12px 10px;background:transparent;border:0;border-radius:8px;color:var(--text);text-align:left;box-shadow:none}.sms-thread-button:hover{background:var(--blue-light);box-shadow:none}.sms-thread-button.selected{background:#e8f2fb;color:#174e78;box-shadow:none}.sms-thread-address{display:block;font-size:13px;font-weight:650;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.sms-thread-snippet{display:block;margin-top:5px;color:var(--muted);font-size:12px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.sms-conversation{min-width:0;display:flex;flex-direction:column;min-height:420px;padding:0;overflow:hidden}.sms-chat-header{height:64px;flex:none;display:flex;align-items:center;gap:11px;padding:0 18px;border-bottom:1px solid var(--line)}.sms-chat-avatar{width:37px;height:37px;display:grid;place-items:center;border-radius:50%;background:#e8f2fb;color:#1769aa;font-weight:700}.sms-chat-header .name{font-size:14px}.sms-chat-header .muted{margin:3px 0 0;font-size:11px}.sms-message-list{display:flex;flex-direction:column;gap:10px;flex:1;min-height:250px;max-height:none;overflow:auto;padding:20px 22px;background:var(--canvas)}.sms-message{max-width:min(74%,560px);padding:10px 13px;border-radius:15px;background:#fff;border:1px solid var(--line);overflow-wrap:anywhere;white-space:pre-wrap;box-shadow:0 1px 2px #00008}.sms-message.inbound{align-self:flex-start;border-bottom-left-radius:5px}.sms-message.outbound{align-self:flex-end;background:#dff1ff;border-color:#d0e8fb;border-bottom-right-radius:5px}.sms-direction{margin:6px 0 0;color:var(--muted);font-size:10px}.sms-compose{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:10px;align-items:end;padding:14px 16px;background:var(--surface);border-top:1px solid var(--line)}.sms-compose textarea{min-height:44px;max-height:130px;resize:vertical;background:var(--canvas);color:var(--text);border:1px solid var(--line);border-radius:10px;padding:11px 13px;font:inherit;line-height:1.4}.sms-compose button{height:42px;padding:0 18px;border:0;border-radius:9px;background:#0878d1;color:white;font-weight:650;box-shadow:none}.sms-compose button:hover{background:#0668b6;box-shadow:none}.sms-empty{display:grid;place-items:center;text-align:center;color:var(--muted);min-height:300px;padding:24px}.sms-empty strong{display:block;color:var(--text);font-size:14px;margin-bottom:6px}.sms-empty span{font-size:12px}.theme-dark .sms-thread-list,.theme-dark .sms-conversation,.theme-dark .sms-compose{background:#080808}.theme-dark .sms-device-card>.row{border-color:#242424}.theme-dark .sms-thread-button{color:#e5e7eb}.theme-dark .sms-thread-button:hover{background:#141414}.theme-dark .sms-thread-button.selected{background:#142332;color:#b7ddfb}.theme-dark .sms-chat-avatar,.theme-dark .sms-device-card>.row .device-avatar{background:#13283a;color:#9dd2ff}.theme-dark .sms-message.inbound{background:#171717;border-color:#292929}.theme-dark .sms-message.outbound{background:#102337;border-color:#1a3a55}.theme-dark .sms-message-list,.theme-dark .sms-compose textarea{background:#030303}.theme-dark .sms-compose button{background:#147fcf}@media(max-width:850px){.sms-layout{grid-template-columns:minmax(190px,38%) minmax(0,1fr)}.sms-conversation{min-height:380px}.sms-message-list{padding:14px}.sms-message{max-width:88%}}
.sms-new-button{padding:9px 14px;border:0;border-radius:8px;background:#0878d1;color:#fff;font-size:12px;font-weight:650;box-shadow:none}.sms-new-button:hover{background:#0668b6;box-shadow:none}.sms-device-select{margin-left:auto;max-width:180px;padding:8px 10px;border:1px solid var(--line);border-radius:8px;background:var(--surface);color:var(--text);font:inherit}.sms-new-compose{grid-template-columns:minmax(0,1fr) auto}.sms-recipient{grid-column:1/-1;width:100%;height:42px;padding:0 12px;border:1px solid var(--line);border-radius:9px;background:var(--canvas);color:var(--text);font:inherit}.sms-new-empty{display:grid;place-items:center;color:var(--muted)}.theme-dark .sms-new-button{background:#147fcf}.theme-dark .sms-device-select,.theme-dark .sms-recipient{background:#030303;border-color:#333;color:#e5e7eb}
"#;

const MESSAGES_DETAIL_STYLE: &str = r#"
.sms-thread-title-row{display:flex;align-items:center;gap:8px}.sms-thread-title-row .sms-thread-address{flex:1}.sms-thread-time{flex:none;color:var(--muted);font-size:10px}.sms-thread-number{display:block;margin-top:4px;color:var(--muted);font-size:10px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.sms-back{display:none;width:34px;height:34px;padding:0;border:0;border-radius:50%;background:transparent;color:var(--blue);font-size:24px;line-height:1}.sms-empty-content{display:grid;justify-items:center;gap:9px;max-width:300px;text-align:center}.sms-empty-icon{width:58px;height:58px;display:grid;place-items:center;border-radius:20px;background:var(--blue-light);color:var(--blue);font-size:27px}.sms-empty-content strong{color:var(--text);font-size:15px}.sms-empty-content span:last-child{color:var(--muted);font-size:12px;line-height:1.5}.theme-dark .sms-thread-time,.theme-dark .sms-thread-number{color:#9ca3af}.theme-dark .sms-empty-icon{background:#142332;color:#8ccaff}@media(max-width:760px){.sms-layout{grid-template-columns:minmax(0,1fr)}.sms-layout:not(.conversation-open)>.sms-conversation{display:none}.sms-layout.conversation-open>.sms-thread-list{display:none}.sms-layout.conversation-open>.sms-conversation{min-height:calc(100vh - 240px)}.sms-back{display:grid;place-items:center}.sms-chat-header{padding:0 12px;gap:9px}.sms-device-select{max-width:135px}}

.app-main.messaging{height:calc(100vh - 40px);display:flex;flex-direction:column;padding-bottom:16px}.messaging .topbar{flex:none;height:64px;margin-bottom:12px}.messaging .content{display:flex;flex-direction:column;flex:1;min-height:0;width:100%;margin:0 auto}.messaging .sms-page{height:auto;flex:1;min-height:0;gap:10px}.messaging .sms-conversation{min-height:0;margin:0}.messaging .sms-message-list{min-height:0}.messaging .sms-layout{min-height:0}.messaging .status{flex:none;margin:0 0 10px;max-height:90px;overflow:auto}.sms-chat-header>div:not(.sms-chat-avatar){min-width:0}.sms-chat-header .name,.sms-chat-header .muted{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.sms-contact{background:var(--surface);color:var(--text);border-color:var(--line)}.sms-contact:hover{background:var(--blue-light)}.theme-dark .sms-contact:hover{background:#152535}@media(max-width:600px){.app-main.messaging{height:calc(100vh - 102px);padding-bottom:8px}.messaging .topbar{height:54px}.sms-heading p{font-size:10px}.sms-compose{padding:10px}}
"#;
#[component]
fn App() -> Element {
    let mut pairings = use_signal(Vec::<PairedDevice>::new);
    let mut devices = use_signal(Vec::<DiscoveredDevice>::new);
    let mut sessions = use_signal(Vec::<PairSession>::new);
    let mut identity = use_signal(|| None::<DeviceInfo>);
    let mut health = use_signal(|| "Starting link service…".to_string());
    let mut status = use_signal(String::new);
    let mut active_tab = use_signal(|| "home".to_string());
    let mut selected_device = use_signal(|| None::<String>);
    let mut device_menu_open = use_signal(|| false);
    let mut auto_gallery_key = use_signal(|| None::<String>);
    let mut dark_mode = use_signal(load_dark_mode);
    let busy = use_signal(|| false);
    let window = dioxus::desktop::use_window();
    let mut address = use_signal(String::new);
    let mut clip_enabled = use_signal(|| false);
    let mut clip_notice = use_signal(|| "Clipboard sync is off".to_string());
    let mut gallery_packets = use_signal(Vec::<link_core::pair_session::GalleryPacket>::new);
    let mut message_packets = use_signal(Vec::<link_core::pair_session::MessagesPacket>::new);
    let mut selected_message = use_signal(|| None::<(String, String, String)>);
    let mut message_draft = use_signal(String::new);
    let mut new_sms_address = use_signal(String::new);
    let mut contact_query = use_signal(String::new);
    let mut conversation_query = use_signal(String::new);
    let mut pending_sends = use_signal(HashMap::<String, (String, String, String)>::new);
    let mut drafts = use_signal(HashMap::<String, String>::new);
    let mut draft_context = use_signal(|| None::<String>);
    use_effect(move || {
        let key = selected_message.read().as_ref().map(|(peer, thread, _)| format!("{peer}:{thread}"));
        if *draft_context.read() != key {
            let previous = draft_context.read().clone();
            if let Some(previous) = previous { drafts.write().insert(previous, message_draft.read().clone()); }
            let next = key.as_ref().and_then(|k| drafts.read().get(k).cloned()).unwrap_or_default();
            draft_context.set(key);
            message_draft.set(next);
        }
    });
    let mut last_scroll = use_signal(|| None::<(String, String)>);
    use_effect(move || {
        if let Some((peer, thread, _)) = selected_message.read().clone().filter(|(_, thread, _)| thread != "new") {
            let newest = message_packets.read().iter().find(|p| p.from.to_string() == peer && p.op == "messages_thread_result" && p.thread_id.as_ref() == Some(&thread))
                .and_then(|p| p.messages.last().map(|m| m.id.clone()));
            if let Some(newest) = newest {
                let key = format!("{peer}:{thread}");
                let previous = last_scroll.read().clone();
                if previous.as_ref() != Some(&(key.clone(), newest.clone())) {
                    let force = previous.as_ref().is_none_or(|(old, _)| old != &key);
                    last_scroll.set(Some((key, newest)));
                    spawn(async move {
                        let _ = document::eval(&format!("requestAnimationFrame(() => {{ const e = document.querySelector('.sms-message-list'); if (e && ({} || e.scrollHeight-e.scrollTop-e.clientHeight < 180)) e.scrollTop=e.scrollHeight; }});", force)).await;
                    });
                }
            }
        }
    });
    let mut auto_messages_key = use_signal(|| None::<String>);
    let viewer_parent = window.clone();
    use_future(move || async move {
        let client = match DaemonClient::discover() {
            Ok(c) => c,
            Err(e) => {
                health.set(e.to_string());
                return;
            }
        };
        if let Err(e) = client.ensure_running().await {
            health.set(e.to_string());
            return;
        }
        loop {
            match client.snapshot().await {
                Ok(snapshot) => {
                    identity.set(Some(snapshot.identity));
                    devices.set(snapshot.devices);
                    sessions.set(snapshot.sessions);
                    pairings.set(snapshot.pairings);
                    health.set("Link service running".into());
                }
                Err(e) => health.set(format!("Link service unavailable: {e}")),
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
    use_future(move || async move {
        let client = match DaemonClient::discover() {
            Ok(c) => c,
            Err(_) => return,
        };
        if let Ok(value) = client.clipboard_status().await {
            clip_enabled.set(value["enabled"].as_bool().unwrap_or(false));
        }
        let mut last_clip: Option<(String, String)> = None;
        let mut sent_to = HashSet::<String>::new();
        let mut retry_after = HashMap::<String, tokio::time::Instant>::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if !*clip_enabled.read() {
                last_clip = None;
                sent_to.clear();
                retry_after.clear();
            } else {
                if let Some((mime, data_b64)) = read_linux_clipboard().await {
                    let current = (mime.clone(), data_b64.clone());
                    if last_clip.as_ref() != Some(&current) {
                        last_clip = Some(current);
                        sent_to.clear();
                        retry_after.clear();
                    }
                    let peers = pairings.read().clone();
                    let active_peers = peers
                        .iter()
                        .map(|peer| peer.id.to_string())
                        .collect::<HashSet<_>>();
                    sent_to.retain(|id| active_peers.contains(id));
                    retry_after.retain(|id, _| active_peers.contains(id));
                    let now = tokio::time::Instant::now();
                    for peer in peers {
                        let peer_id = peer.id.to_string();
                        let peer_name =
                            phone_display_name(&peer, &devices.read(), &message_packets.read());
                        if sent_to.contains(&peer_id)
                            || retry_after.get(&peer_id).is_some_and(|time| *time > now)
                        {
                            continue;
                        }
                        match client
                            .clipboard_send(&peer_id, mime.clone(), data_b64.clone())
                            .await
                        {
                            Ok(()) => {
                                sent_to.insert(peer_id.clone());
                                retry_after.remove(&peer_id);
                                clip_notice.set(format!("Clipboard sent to {peer_name}"));
                            }
                            Err(_) => {
                                retry_after
                                    .insert(peer_id, now + std::time::Duration::from_secs(5));
                                clip_notice.set(format!(
                                    "Clipboard delivery to {} failed; retrying",
                                    peer_name
                                ));
                            }
                        }
                    }
                } else {
                    last_clip = None;
                    sent_to.clear();
                    retry_after.clear();
                }
                if let Ok(value) = client.clipboard_take().await {
                    let pending = value
                        .get("clipboard")
                        .and_then(|v| {
                            serde_json::from_value::<Option<(String, String, String)>>(v.clone())
                                .ok()
                        })
                        .flatten();
                    if let Some((source_id, mime, data_b64)) = pending {
                        let result = if let Ok(data) = base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            &data_b64,
                        ) {
                            set_linux_clipboard(&mime, &data).await
                        } else {
                            Err("received clipboard data was invalid".into())
                        };
                        match result {
                            Ok(()) => {
                                let notice = format!("Clipboard received · {mime}");
                                last_clip = Some((mime, data_b64));
                                // This desktop is the hub for paired devices:
                                // relay the clip to everyone except its source.
                                sent_to = HashSet::from([source_id]);
                                retry_after.clear();
                                clip_notice.set(notice);
                            }
                            Err(error) => {
                                clip_notice.set(format!("Could not set clipboard: {error}"))
                            }
                        }
                    }
                }
            }
        }
    });
    use_future(move || {
        let viewer_parent = viewer_parent.clone();
        async move {
            let Ok(client) = DaemonClient::discover() else {
                return;
            };
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Ok(value) = client.gallery_take().await {
                    if let Some(packet) =
                        value.get("gallery").filter(|v| !v.is_null()).and_then(|v| {
                            serde_json::from_value::<link_core::pair_session::GalleryPacket>(
                                v.clone(),
                            )
                            .ok()
                        })
                    {
                        if packet.op == "gallery_image" {
                            if let (Some(mime), Some(data)) = (&packet.mime, &packet.data_b64) {
                                let photo_name = packet
                                    .name
                                    .clone()
                                    .or_else(|| packet.photo_id.clone())
                                    .unwrap_or_else(|| "Shared photo".into());
                                let title = format!("Photo Viewer · {photo_name}");
                                let props = PhotoViewerProps {
                                    image_src: format!("data:{mime};base64,{data}"),
                                    photo_name: photo_name.clone(),
                                    dark_mode: *dark_mode.read(),
                                };
                                let dom = dioxus::dioxus_core::VirtualDom::new_with_props(
                                    PhotoViewer,
                                    props,
                                );
                                viewer_parent.new_window(
                                    dom,
                                    dioxus::desktop::Config::new().with_window(
                                        dioxus::desktop::WindowBuilder::new()
                                            .with_title(title)
                                            .with_inner_size(dioxus::desktop::LogicalSize::new(
                                                1100.0, 820.0,
                                            )),
                                    ),
                                );
                                status.set(format!("Opened {photo_name} in the photo viewer"));
                            }
                            continue;
                        }
                        let mut packets = gallery_packets.read().clone();
                        if packet.op == "gallery_list_result" {
                            packets.retain(|p| {
                                !(p.from == packet.from && p.op == "gallery_list_result")
                            });
                        }
                        packets.push(packet);
                        if packets.len() > 24 {
                            packets.remove(0);
                        }
                        gallery_packets.set(packets);
                    }
                }
            }
        }
    });
    use_effect(move || {
        if active_tab.read().as_str() != "photos" {
            auto_gallery_key.set(None);
            return;
        }
        let selected = selected_device.read().clone();
        let peers = pairings
            .read()
            .iter()
            .filter(|peer| {
                selected
                    .as_ref()
                    .map_or(true, |id| id == &peer.id.to_string())
            })
            .cloned()
            .collect::<Vec<_>>();
        if peers.is_empty() {
            auto_gallery_key.set(None);
            return;
        }
        let key = format!(
            "{}:{}",
            if selected.is_some() {
                "selected"
            } else {
                "all"
            },
            peers
                .iter()
                .map(|peer| peer.id.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let already_requested = auto_gallery_key.read().as_ref() == Some(&key);
        if already_requested {
            return;
        }
        auto_gallery_key.set(Some(key));
        status.set("Loading shared galleries…".into());
        for peer in peers {
            let peer_id = peer.id.to_string();
            let request_id = link_core::DeviceId::generate().to_string();
            spawn(async move {
                if let Ok(client) = DaemonClient::discover() {
                    if let Err(error) = client.gallery_list(&peer_id, request_id, 0).await {
                        status.set(format!("Gallery request failed: {error}"));
                    }
                }
            });
        }
    });
    let mut selected_message_for_poll = selected_message;
    use_future(move || async move {
        let Ok(client) = DaemonClient::discover() else {
            return;
        };
        let mut next_status_probe = tokio::time::Instant::now();
        let mut last_peer_key = String::new();
        let mut last_contact_key = String::new();
        let mut next_contacts_probe = tokio::time::Instant::now();
        let mut next_messages_probe = tokio::time::Instant::now();
        loop {
            let peers = pairings
                .read()
                .iter()
                .filter(|peer| peer.os.eq_ignore_ascii_case("android") || peer.os.trim().is_empty())
                .map(|peer| peer.id.to_string())
                .collect::<Vec<_>>();
            let peer_key = peers.join(",");
            if !peers.is_empty()
                && (peer_key != last_peer_key || tokio::time::Instant::now() >= next_status_probe)
            {
                last_peer_key = peer_key;
                next_status_probe =
                    tokio::time::Instant::now() + std::time::Duration::from_secs(10);
                for peer_id in peers {
                    let status_client = client.clone();
                    tokio::spawn(async move {
                        let _ = status_client.messages_status(&peer_id).await;
                    });
                }
            }
            if active_tab.read().as_str() == "messages" {
                let selection = selected_message_for_poll.read().clone();
                if let Some((peer_id, thread_id, _)) =
                    selection.as_ref().filter(|(_, t, _)| t == "new")
                {
                    let query = contact_query.read().clone();
                    let key = format!("{peer_id}:{query}");
                    if last_contact_key != key || tokio::time::Instant::now() >= next_contacts_probe
                    {
                        next_contacts_probe =
                            tokio::time::Instant::now() + std::time::Duration::from_secs(10);
                        last_contact_key = key;
                        let contact_client = client.clone();
                        let peer_id = peer_id.clone();
                        spawn(async move {
                            if let Err(error) =
                                contact_client.messages_contacts(&peer_id, query, 0).await
                            {
                                status.set(format!("Could not load contacts: {error}"));
                            }
                        });
                    }
                    let _ = thread_id;
                } else {
                    last_contact_key.clear();
                }
                if tokio::time::Instant::now() >= next_messages_probe {
                    next_messages_probe =
                        tokio::time::Instant::now() + std::time::Duration::from_secs(5);
                    let enabled = message_packets
                        .read()
                        .iter()
                        .filter(|p| p.op == "messages_status_result" && p.enabled == Some(true))
                        .map(|p| p.from.to_string())
                        .collect::<Vec<_>>();
                    for peer_id in enabled {
                        let refresh_client = client.clone();
                        let thread = selection
                            .as_ref()
                            .filter(|(id, t, _)| id == &peer_id && t != "new")
                            .map(|(_, t, _)| t.clone());
                        spawn(async move {
                            if let Err(error) = refresh_client.messages_list(&peer_id).await {
                                status.set(format!("Messages unavailable: {error}"));
                            }
                            if let Some(thread) = thread {
                                let _ = refresh_client.messages_thread(&peer_id, thread).await;
                            }
                        });
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let Ok(value) = client.messages_take().await else {
                continue;
            };
            let Some(mut packet) = value
                .get("messages")
                .filter(|v| !v.is_null())
                .and_then(|v| {
                    serde_json::from_value::<link_core::pair_session::MessagesPacket>(v.clone())
                        .ok()
                })
            else {
                continue;
            };
            let request_peer = packet.from.to_string();
            if packet.op == "messages_send_result" {
                status.set("Sending through your phone…".into());
                if let Some((peer, recipient, body)) = pending_sends.read().get(&packet.request_id) {
                    let matches = selected_message_for_poll.read().as_ref().is_some_and(|(id, thread, address)| id == peer && (address == recipient || (thread == "new" && new_sms_address.read().trim() == recipient)));
                    if matches && message_draft.read().as_str() == body {
                        message_draft.set(String::new());
                    }
                }
                if let (Some(thread_id), Some(address)) = (packet.thread_id.clone(), packet.address.clone()) {
                    let follow = selected_message_for_poll.read().as_ref().is_some_and(|(peer, thread, recipient)|
                        peer == &request_peer && (thread == &thread_id || (thread == "new" && new_sms_address.read().trim() == address) || recipient == &address));
                    if follow { selected_message_for_poll.set(Some((request_peer.clone(), thread_id, address))); }
                }
                if let Some((peer_id, thread_id, _)) = selected_message_for_poll
                    .read()
                    .clone()
                    .filter(|(peer_id, _, _)| peer_id == &request_peer)
                {
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    let _ = client.messages_list(&peer_id).await;
                    let _ = client.messages_thread(&peer_id, thread_id).await;
                }
            } else if packet.op == "messages_sent" {
                pending_sends.write().remove(&packet.request_id);
                status.set("Message sent by your phone".into());
            } else if packet.op == "messages_error" {
                let failed = pending_sends.write().remove(&packet.request_id);
                if let Some((peer, recipient, body)) = failed {
                    if selected_message_for_poll
                        .read()
                        .as_ref()
                        .is_some_and(|(id, thread, address)| id == &peer && (address == &recipient || (thread == "new" && new_sms_address.read().trim() == recipient)))
                        && message_draft.read().is_empty()
                    {
                        message_draft.set(body);
                    }
                }
                status.set(
                    packet
                        .error
                        .clone()
                        .unwrap_or_else(|| "SMS request failed".into()),
                );
            }
            let mut packets = message_packets.read().clone();
            if packet.op == "messages_error" && packet.query.is_some() {
                packets.retain(|old| {
                    !(old.from == packet.from && old.op == "messages_contacts_result")
                });
            }
            if packet.op == "messages_status_result" && packet.enabled == Some(false) {
                packets.retain(|old| old.from != packet.from);
                if selected_message_for_poll
                    .read()
                    .as_ref()
                    .is_some_and(|(peer, _, _)| peer == &request_peer)
                {
                    selected_message_for_poll.set(None);
                }
            }
            if let Some(index) = packets.iter().position(|old| {
                old.from == packet.from
                    && old.op == packet.op
                    && (packet.op != "messages_thread_result" || old.thread_id == packet.thread_id)
            }) {
                let previous = packets.remove(index);
                merge_message_page(&previous, &mut packet);
            }
            packets.push(packet);
            if packets.len() > 32 {
                packets.remove(0);
            }
            message_packets.set(packets);
        }
    });
    use_effect(move || {
        if active_tab.read().as_str() != "messages" {
            auto_messages_key.set(None);
            return;
        }
        let peers = pairings
            .read()
            .iter()
            .filter(|peer| {
                message_packets.read().iter().any(|packet| {
                    packet.from == peer.id
                        && packet.op == "messages_status_result"
                        && packet.enabled == Some(true)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if peers.is_empty() {
            auto_messages_key.set(None);
            return;
        }
        let key = peers
            .iter()
            .map(|peer| peer.id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let already_requested = auto_messages_key.read().as_ref() == Some(&key);
        if already_requested {
            return;
        }
        auto_messages_key.set(Some(key));
        for peer in peers {
            let id = peer.id.to_string();
            let peer_name = phone_display_name(&peer, &devices.read(), &message_packets.read());
            spawn(async move {
                if let Ok(client) = DaemonClient::discover() {
                    if let Err(error) = client.messages_list(&id).await {
                        status.set(format!(
                            "Could not load messages from {}: {error}",
                            peer_name
                        ));
                    }
                }
            });
        }
    });
    let section_title = match active_tab.read().as_str() {
        "photos" => "Photos",
        "messages" => "Messages",
        "clipboard" => "Clipboard",
        "notifications" => "Notifications",
        "devices" => "Devices",
        _ => "Home",
    };
    let paired_count = pairings.read().len();
    let enabled_message_pairings = pairings
        .read()
        .iter()
        .filter(|peer| {
            message_packets.read().iter().any(|packet| {
                packet.from == peer.id
                    && packet.op == "messages_status_result"
                    && packet.enabled == Some(true)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let status_resolved = message_packets
        .read()
        .iter()
        .any(|packet| packet.op == "messages_status_result");
    let has_enabled_messages = !enabled_message_pairings.is_empty();
    let current_message_title = selected_message
        .read()
        .as_ref()
        .map(|(peer_id, thread_id, address)| {
            message_packets
                .read()
                .iter()
                .find(|packet| {
                    packet.from.to_string() == *peer_id
                        && packet.op == "messages_thread_result"
                        && packet.thread_id.as_deref() == Some(thread_id.as_str())
                })
                .and_then(|packet| packet.display_name.clone())
                .or_else(|| {
                    message_packets
                        .read()
                        .iter()
                        .find(|packet| {
                            packet.from.to_string() == *peer_id
                                && packet.op == "messages_list_result"
                        })
                        .and_then(|packet| {
                            packet.threads.iter().find(|thread| thread.id == *thread_id)
                        })
                        .and_then(|thread| thread.display_name.clone())
                })
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| address.clone())
        })
        .unwrap_or_default();
    let first_enabled_message_peer = enabled_message_pairings
        .first()
        .map(|peer| peer.id.to_string());
    use_effect(move || {
        if active_tab.read().as_str() == "messages" && status_resolved && !has_enabled_messages {
            active_tab.set("home".into());
        }
    });
    let nearby_devices = devices
        .read()
        .iter()
        .map(|device| {
            (
                device.info.id.to_string(),
                device.info.name.clone(),
                device.info.os.clone(),
                device.addresses.join(", "),
                device.addresses.first().cloned(),
            )
        })
        .collect::<Vec<_>>();
    let selected_gallery_device = selected_device.read().clone();
    let drag_window = window.clone();
    let minimize_window = window.clone();
    let maximize_window = window.clone();
    rsx! {
        style { "{STYLE}" }
        if *dark_mode.read() { style { "{DARK_STYLE}" } }
        style { "{DEVICE_MENU_STYLE}" }
        style { "{MESSAGES_STYLE}" }
        style { "{MESSAGES_DETAIL_STYLE}" }
        div { class:if *dark_mode.read() {"window-frame theme-dark"} else {"window-frame"},
            div { class:"window-chrome",
                div { class:"window-drag-region", onmousedown:move |_| drag_window.drag(),
                    div {class:"window-app-mark","L"}
                    span {class:"window-title","Link to Linux"}
                }
                div {class:"window-controls",
                    div {class:"device-menu",
                        button {class:"device-menu-trigger",aria_expanded:"{device_menu_open}",onclick:move |_| {let is_open = *device_menu_open.read(); device_menu_open.set(!is_open);},"Devices · {paired_count}  ⌄"}
                        if *device_menu_open.read() {
                        div {class:"device-menu-popup",
                            div {class:"device-menu-heading","PHOTO GALLERIES"}
                            button {class:if selected_device.read().is_none() {"device-menu-item selected"} else {"device-menu-item"},onclick:move |_| {selected_device.set(None);active_tab.set("photos".into());device_menu_open.set(false);},span {class:"dot"}span {class:"menu-device-name","All devices"}span {class:"menu-device-os","All galleries"}}
                            for paired in pairings.read().iter() {
                                {let device_id = paired.id.to_string(); let is_selected = selected_device.read().as_deref() == Some(device_id.as_str());
                                rsx! {button {class:if is_selected {"device-menu-item selected"} else {"device-menu-item"},key:"device-menu-{device_id}",onclick:move |_| {selected_device.set(Some(device_id.clone()));active_tab.set("photos".into());device_menu_open.set(false);},span {class:"dot"}span {class:"menu-device-name","{phone_display_name(paired, &devices.read(), &message_packets.read())}"}span {class:"menu-device-os","Android"}}}
                                }
                            }
                            if pairings.read().is_empty() {div {class:"device-menu-heading","No paired devices"}}
                            div {class:"device-menu-divider"}
                            button {class:"device-menu-item device-menu-manage",onclick:move |_| {active_tab.set("devices".into());device_menu_open.set(false);},"＋  Add or manage devices"}
                        }
                        }
                    }
                    button {class:"window-control appearance-toggle", title:if *dark_mode.read() {"Switch to light mode"} else {"Switch to dark mode"}, aria_label:if *dark_mode.read() {"Switch to light mode"} else {"Switch to dark mode"}, onclick:move |_| {let enabled = !*dark_mode.read(); dark_mode.set(enabled); save_dark_mode(enabled);}, if *dark_mode.read() {"☀"} else {"☾"}}
                    button {class:"window-control", title:"Minimize", aria_label:"Minimize window", onclick:move |_| minimize_window.set_minimized(true), "−"}
                    button {class:"window-control", title:"Maximize", aria_label:"Maximize window", onclick:move |_| maximize_window.toggle_maximized(), "□"}
                    button {class:"window-control close", title:"Close", aria_label:"Close window", onclick:move |_| window.close(), "×"}
                }
            }
            div { class:"app-shell",
            aside { class:"sidebar",
                div { class:"brand",
                    div { class:"brand-mark", "L" }
                    span { class:"brand-name", "Link to Linux" }
                }
                div { class:"side-caption", "Your connection" }
                div { class:"nav-list",
                    button { class:if *active_tab.read()=="home" {"nav-item selected"} else {"nav-item"}, onclick:move |_| active_tab.set("home".into()), span {class:"nav-icon","⌂"} span {class:"nav-label","Home"} }
                    button { class:if *active_tab.read()=="photos" {"nav-item selected"} else {"nav-item"}, onclick:move |_| active_tab.set("photos".into()), span {class:"nav-icon","▧"} span {class:"nav-label","Photos"} }
                    if !enabled_message_pairings.is_empty() {button { class:if *active_tab.read()=="messages" {"nav-item selected"} else {"nav-item"}, onclick:move |_| active_tab.set("messages".into()), span {class:"nav-icon","✉"} span {class:"nav-label","Messages"} }}
                    button { class:if *active_tab.read()=="clipboard" {"nav-item selected"} else {"nav-item"}, onclick:move |_| active_tab.set("clipboard".into()), span {class:"nav-icon","▤"} span {class:"nav-label","Clipboard"} }
                    button { class:if *active_tab.read()=="notifications" {"nav-item selected"} else {"nav-item"}, onclick:move |_| active_tab.set("notifications".into()), span {class:"nav-icon","◉"} span {class:"nav-label","Notifications"} }
                }
                div { class:"sidebar-device",
                    div {class:"device-title","PAIRED DEVICES"}
                    div {class:"device-indicator",span {class:"dot"},"{paired_count} connected"}
                }
            }
            main { class:if *active_tab.read()=="messages" {"app-main messaging"} else {"app-main"},
                header { class:"topbar",
                    div {div {class:"eyebrow","LINK TO LINUX · LOCAL CONNECTION"} h1 {"{section_title}"}}
                    span {class:"service-badge",span {class:"dot"},"{health}"}
                }
                div {class:"content tab-content",
                    if !status.read().is_empty() {p {class:"status","{status}"}}

                    if *active_tab.read()=="home" {
                        div {class:"hero",
                            div {
                                div {class:"eyebrow","YOUR PHONE, CONNECTED"}
                                h2 {"Everything in sync."}
                                p {class:"muted","Keep your Android photos, clipboard, and notifications close while your devices stay paired over your local network."}
                                div {class:"actions",
                                    button {onclick:move |_| active_tab.set("photos".into()),"Open photos"}
                                    button {class:"secondary",onclick:move |_| active_tab.set("devices".into()),"Manage devices"}
                                }
                            }
                            div {class:"phone-art",div {class:"phone-outline","↗"}}
                        }
                        div {class:"stats",
                            div {class:"stat",div {class:"stat-value","{paired_count}"},div {class:"stat-label","Paired devices"}}
                            div {class:"stat",div {class:"stat-value",if *clip_enabled.read() {"On"} else {"Off"}},div {class:"stat-label","Clipboard sync"}}
                            div {class:"stat",div {class:"stat-value","Ready"},div {class:"stat-label","Secure local link"}}
                        }
                        div {class:"home-columns",
                            div {class:"card",
                                div {class:"row",div {h2 {class:"subhead","Shared photos"}p {class:"muted","Browse your Android gallery from Linux."}},button {class:"card-link",onclick:move |_| active_tab.set("photos".into()),"View all"}}
                                if pairings.read().is_empty() {p {class:"muted","Pair an Android device to see its gallery here."}}
                                else {for paired in pairings.read().iter() {div {class:"row",key:"home-gallery-{paired.id}",div {class:"device-indicator",span {class:"device-avatar","▧"},"{phone_display_name(paired, &devices.read(), &message_packets.read())}"},GalleryButton {id:paired.id.to_string(),name:phone_display_name(paired, &devices.read(), &message_packets.read()),offset:0,label:"Open gallery",status}}}}
                            }
                            div {class:"card",
                                h2 {class:"subhead","Clipboard"}
                                p {class:"muted","{clip_notice}"}
                                button {class:"secondary",onclick:move |_| active_tab.set("clipboard".into()),"Clipboard settings"}
                            }
                        }
                    }

                    if *active_tab.read()=="photos" {
                        div {class:"row",div {h2 {class:"section-title","Your shared galleries"}p {class:"muted","Galleries load automatically when you open Photos. Thumbnails load in pages; full-size photos open on request."}}}
                        if pairings.read().is_empty() {div {class:"empty","Pair an Android device to share its gallery with Linux."}}
                        for paired in pairings.read().iter().filter(|paired| selected_gallery_device.as_ref().map_or(true, |selected| selected == &paired.id.to_string())) {
                            div {class:"card",key:"gallery-{paired.id}",
                                div {class:"row",div {class:"device-indicator",span {class:"device-avatar","▧"},div {span {class:"name","{phone_display_name(paired, &devices.read(), &message_packets.read())}"}p {class:"muted","Android photo gallery"}}},GalleryButton {id:paired.id.to_string(),name:phone_display_name(paired, &devices.read(), &message_packets.read()),offset:0,label:"Refresh",status}}
                                for packet in gallery_packets.read().iter().filter(|p| p.from == paired.id) {
                                    if let Some(message)=packet.error.as_ref().filter(|_|packet.op=="gallery_error") {p {class:"status","{message}"}}
                                    if packet.op=="gallery_list_result" {
                                        div {class:"gallery-grid",
                                            for item in packet.items.iter() {
                                                div {class:"gallery-tile",key:"photo-{paired.id}-{item.id}",
                                                    if let (Some(mime),Some(data))=(&item.thumbnail_mime,&item.thumbnail_b64) {img {src:"data:{mime};base64,{data}",alt:"{item.name}"}}
                                                    else {div {class:"tile-placeholder","Photo"}}
                                                    div {class:"tile-meta",span {class:"name","{item.name}"}p {class:"muted","{item.size} bytes"}GalleryFetchButton {id:paired.id.to_string(),photo_id:item.id.clone(),photo_name:item.name.clone(),status}}
                                                }
                                            }
                                        }
                                        if packet.items.is_empty() {p {class:"muted","No photos in this gallery."}}
                                        div {class:"actions",
                                            if let Some(offset)=packet.offset.filter(|offset|*offset>=40) {GalleryButton {id:paired.id.to_string(),name:phone_display_name(paired, &devices.read(), &message_packets.read()),offset:offset-40,label:"Previous page",status}}
                                            if let Some(offset)=packet.next_offset {GalleryButton {id:paired.id.to_string(),name:phone_display_name(paired, &devices.read(), &message_packets.read()),offset,label:"More photos",status}}
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if *active_tab.read()=="messages" {
                        div {class:"sms-page",
                        div {class:"sms-heading",div {h2 {"Messages"}p {"SMS and MMS from your Android devices"}}
                            button {class:"sms-new-button",onclick:move |_| {if let Some(peer_id)=first_enabled_message_peer.clone() {selected_message.set(Some((peer_id,"new".into(),String::new())));new_sms_address.set(String::new());contact_query.set(String::new());}},"＋  New message"}
                        }
                        div {class:if selected_message.read().is_some() {"sms-layout conversation-open"} else {"sms-layout"},
                            div {class:"sms-thread-list",
                                input {class:"sms-recipient",value:"{conversation_query}",placeholder:"Search loaded conversations",oninput:move |event|conversation_query.set(event.value())}
                                for paired in enabled_message_pairings.iter() {
                                    div {class:"card sms-device-card",key:"sms-device-{paired.id}",
                                        div {class:"row",div {class:"device-indicator",span {class:"device-avatar","✉"},div {span {class:"name","{phone_display_name(paired, &devices.read(), &message_packets.read())}"}p {class:"muted","Conversations"}}},MessagesRefreshButton {id:paired.id.to_string(),status}}
                                        if let Some(packet)=message_packets.read().iter().find(|packet| packet.from==paired.id && packet.op=="messages_list_result") {
                                            if packet.threads.is_empty() {p {class:"muted","No SMS or MMS conversations found."}}
                                            for thread in packet.threads.iter().filter(|t| format!("{} {} {}", t.display_name.as_deref().unwrap_or(""), t.address, t.snippet).to_lowercase().contains(&conversation_query.read().to_lowercase())) {
                                                MessagesThreadButton {id:paired.id.to_string(),thread_id:thread.id.clone(),address:thread.address.clone(),title:thread.display_name.as_deref().filter(|name|!name.trim().is_empty()).unwrap_or(&thread.address).to_string(),snippet:thread.snippet.clone(),date:thread.date,selected:selected_message.read().as_ref().is_some_and(|(peer,thread_id,_)|peer==&paired.id.to_string()&&thread_id==&thread.id),selected_message,status}
                                            }
                                            if let Some(offset)=packet.next_offset {MessagePageButton {id:paired.id.to_string(),thread_id:None,offset,status,label:"More conversations"}}
                                        } else if let Some(packet)=message_packets.read().iter().find(|packet| packet.from==paired.id && packet.op=="messages_error") {
                                            if let Some(error)=packet.error.as_ref() {p {class:"status","{error}"}}
                                            else {p {class:"status","SMS sharing is unavailable. Enable it on Android."}}
                                        } else {p {class:"muted","Loading conversations…"}}
                                    }
                                }
                                if enabled_message_pairings.is_empty() {div {class:"sms-empty",strong {"No enabled Android devices"}span {"Turn on Messages (SMS) and grant access in Link on your phone."}}}
                            }
                            if let Some((peer_id,thread_id,address))=selected_message.read().clone() {
                                if thread_id=="new" {
                                    div {class:"card sms-conversation",
                                        div {class:"sms-chat-header",button {class:"sms-back",title:"Back to conversations",onclick:move |_| selected_message.set(None),"‹"},div {class:"sms-chat-avatar","＋"},div {span {class:"name","New message"}p {class:"muted","Choose which Android phone to send from"}}
                                                select {class:"sms-device-select",value:"{peer_id}",onchange:move |event|selected_message.set(Some((event.value(),"new".into(),String::new()))),
                                                for peer in enabled_message_pairings.iter() {option {value:"{peer.id}","{phone_display_name(peer, &devices.read(), &message_packets.read())}"}}
                                            }
                                        }
                                        input {class:"sms-recipient",value:"{contact_query}",placeholder:"Search phone contacts by name or number",oninput:move |event|contact_query.set(event.value())}
                                        div {class:"sms-message-list sms-contacts",
                                            if let Some(packet)=message_packets.read().iter().find(|p| p.from.to_string()==peer_id && p.op=="messages_contacts_result" && p.query.as_deref()==Some(contact_query.read().as_str())) {
                                                if packet.contacts.is_empty() {p {class:"muted","No matching contacts. You can also enter a phone number below."}}
                                                for contact in packet.contacts.iter() {
                                                    ContactRecipientButton {name:contact.name.clone(),address:contact.address.clone(),recipients:new_sms_address}
                                                }
                                                if let Some(offset)=packet.next_offset {ContactPageButton {id:peer_id.clone(),query:contact_query.read().clone(),offset,status}}
                                            } else {p {class:"muted","Loading contacts… Contact access must be allowed on your phone."}}
                                        }
                                        div {class:"sms-compose sms-new-compose",
                                            input {class:"sms-recipient",value:"{new_sms_address}",placeholder:"To: phone numbers separated by semicolons",oninput:move |event|new_sms_address.set(event.value())}
                                            textarea {value:"{message_draft}",placeholder:"Write a message…",rows:"3",oninput:move |event|message_draft.set(event.value())}
                                            SendSmsButton {id:peer_id.clone(),address:new_sms_address.read().trim().to_string(),body:message_draft.read().clone(),draft:message_draft,pending:pending_sends,status}
                                        }
                                    }
                                } else {
                                div {class:"card sms-conversation",
                                    div {class:"sms-chat-header",button {class:"sms-back",title:"Back to conversations",onclick:move |_| selected_message.set(None),"‹"},div {class:"sms-chat-avatar","✉"},div {span {class:"name","{current_message_title}"}p {class:"muted","{address}"}}}
                                    div {class:"sms-message-list",
                                        if let Some(packet)=message_packets.read().iter().find(|packet|packet.from.to_string()==peer_id&&packet.op=="messages_thread_result"&&packet.thread_id.as_deref()==Some(thread_id.as_str())) {
                                            if let Some(offset)=packet.next_offset {MessagePageButton {id:peer_id.clone(),thread_id:Some(thread_id.clone()),offset,status,label:"Load older messages"}}
                                            if packet.messages.is_empty() {p {class:"muted","No SMS/MMS history in this conversation."}}
                                            for message in packet.messages.iter() {
                                                div {class:if message.direction=="outbound" {"sms-message outbound"} else {"sms-message inbound"},key:"sms-{message.id}",if (address.contains(';') || address.contains(',')) && message.direction=="inbound" {strong {"{message.sender_name.as_deref().unwrap_or(&message.address)}"}}span {"{message.body}"}p {class:"sms-direction",{format!("{} · {}", message.state.as_deref().unwrap_or(if message.direction=="outbound" {"Sent"} else {"Received"}), format_message_time(message.date))}}}
                                            }
                                        } else {p {class:"muted","Loading conversation…"}}
                                    }
                                    div {class:"sms-compose",textarea {value:"{message_draft}",placeholder:"Write a message…",rows:"3",oninput:move |event|message_draft.set(event.value())}SendSmsButton {id:peer_id,address,body:message_draft.read().clone(),draft:message_draft,pending:pending_sends,status}}
                                }
                                }
                            } else {div {class:"card sms-conversation sms-empty",div {class:"sms-empty-content",span {class:"sms-empty-icon","▤"},strong {"Your messages"},span {"Choose a conversation to read and reply. New messages will appear here, organized by phone and conversation."}}}
                            }
                        }
                        }
                    }

                    if *active_tab.read()=="clipboard" {
                        h2 {class:"section-title","Clipboard sync"}
                        div {class:"card row",
                            div {span {class:"name","{clip_notice}"}p {class:"muted","Text and PNG/JPEG/WebP images sync with trusted devices while this app is open. Text is limited to 32 KiB and images to 5 MiB; clipboard contents are never saved."}}
                            button {class:"secondary",onclick:move |_| {let enabled=!*clip_enabled.read();clip_enabled.set(enabled);clip_notice.set(if enabled {"Clipboard sync enabled"} else {"Clipboard sync is off"}.into());spawn(async move {if let Ok(client)=DaemonClient::discover(){let _=client.clipboard_enabled(enabled).await;}});},if *clip_enabled.read(){"Turn off"}else{"Turn on"}}
                        }
                        div {class:"card",h2 {class:"subhead","About Android clipboard access"}p {class:"muted","Android limits when apps can read copied content in the background. Use the Send clipboard action in the Android running notification when needed."}}
                    }

                    if *active_tab.read()=="notifications" {
                        h2 {class:"section-title","Phone notifications"}
                        div {class:"card",div {class:"device-indicator",span {class:"device-avatar","◉"},div {span {class:"name","Android to Linux"}p {class:"muted","Forward selected phone alerts to this computer."}}}
                            p {class:"muted","Notification forwarding is managed on Android. Turn it on in the Link app, grant notification access in Android Settings, and choose which apps may send alerts. Ongoing notifications are skipped."}
                        }
                    }

                    if *active_tab.read()=="devices" {
                        h2 {class:"section-title","Pairing requests"}
                        if sessions.read().is_empty() {p {class:"muted","Choose a nearby device or enter its address to start pairing."}}
                        for request in sessions.read().iter() {
                            div {class:"card",key:"{request.session}",div {class:"row",div {class:"name","{request.peer.name}"}span {class:"muted",if request.incoming {"Incoming request"} else {"Outgoing request"}}}
                                if request.status.pending() {div {class:"code","{request.code}"}p {class:"muted","Compare this code on both devices. Confirm on each device only if they match. Expires in {request.expires_in_secs}s."}
                                    if request.status==PairStatus::Awaiting {div {class:"actions",DecisionButton {session:request.session.clone(),code:request.code.clone(),accept:true,status,busy}DecisionButton {session:request.session.clone(),code:request.code.clone(),accept:false,status,busy}}}
                                    else {p {class:"muted","Confirmed here. Waiting for the other device…"}}
                                } else {p {match request.status {PairStatus::Paired=>"Paired successfully",PairStatus::Expired=>"Code expired. Start a new request.",PairStatus::Rejected=>"Pairing rejected",_=>"Pairing failed"}}if let Some(error)=&request.error {p {class:"muted","{error}"}}}
                            }
                        }
                        h2 {class:"section-title","Nearby devices"}
                        p {class:"muted","Devices on this local network appear here automatically."}
                        if devices.read().is_empty() {div {class:"empty","No nearby devices yet. Check that both devices are on the same network and Link is running."}}
                        for (device_id,device_name,device_os,device_addresses,device_address) in nearby_devices.iter() {
                            div {class:"card",key:"{device_id}",
                                div {class:"row",
                                    div {class:"device-card",
                                        span {class:"device-avatar","▣"}
                                        div {span {class:"name","{device_name}"}p {class:"muted","{device_os} · {device_addresses}"}}
                                    }
                                    if let Some(peer)=device_address {BeginButton {address:peer.clone(),status,busy}}
                                    else {span {class:"muted","No address resolved"}}
                                }
                            }
                        }
                        h2 {class:"section-title","Connect by address"}
                        div {class:"card",div {class:"actions",input {value:"{address}",placeholder:"192.168.1.20:53536",oninput:move |event|address.set(event.value())}BeginButton {address:address.read().trim().to_string(),status,busy}},p {class:"muted","For IPv6 use [address]:port. Emulator testing uses 127.0.0.1:53537 with ADB port forwarding."}}
                        h2 {class:"section-title","Trusted devices"}
                        if pairings.read().is_empty() {div {class:"empty","No trusted devices yet."}}
                        for paired in pairings.read().iter() {
                            div {class:"card row",key:"trusted-{paired.id}",
                                div {class:"device-card",span {class:"device-avatar","▣"},div {span {class:"name","{phone_display_name(paired, &devices.read(), &message_packets.read())}"}details {class:"details",summary {"Certificate details"}code {"{paired.cert_fingerprint}"}}}}
                                div {class:"actions",PingButton {address:devices.read().iter().find(|d|d.info.id==paired.id).and_then(|d|d.addresses.first()).cloned().or(paired.address.clone()).unwrap_or_else(||paired.name.clone()),fingerprint:paired.cert_fingerprint.clone(),status}UnpairButton {id:paired.id.to_string(),status}}
                            }
                        }
                    }
                }
            }
        }
        }
    }
}
#[component]
fn MessagesRefreshButton(id: String, mut status: Signal<String>) -> Element {
    rsx! {
        button {
            class: "secondary",
            onclick: move |_| {
                let id = id.clone();
                spawn(async move {
                    let result = async { DaemonClient::discover()?.messages_list(&id).await }.await;
                    status.set(match result {
                        Ok(()) => "Refreshing SMS conversations…".into(),
                        Err(error) => format!("Could not refresh messages: {error}"),
                    });
                });
            },
            "Refresh"
        }
    }
}

fn phone_display_name(
    paired: &PairedDevice,
    devices: &[DiscoveredDevice],
    packets: &[link_core::pair_session::MessagesPacket],
) -> String {
    // Prefer the name Android reports from its system settings. Discovery may
    // still carry an older model-derived identity for an already paired peer.
    let status_name = packets
        .iter()
        .rev()
        .find(|packet| packet.from == paired.id && packet.device_name.is_some())
        .and_then(|packet| packet.device_name.as_deref());
    let discovered_name = devices
        .iter()
        .find(|device| device.info.id == paired.id)
        .map(|device| device.info.name.as_str());

    [status_name, discovered_name, Some(paired.name.as_str())]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|name| !name.is_empty() && !looks_like_model_code(name))
        .unwrap_or("Android phone")
        .to_owned()
}

fn looks_like_model_code(name: &str) -> bool {
    name.split_whitespace()
        .any(|part| part.to_ascii_uppercase().starts_with("SM-"))
        || name.to_ascii_lowercase().contains("sdk_gphone")
}

#[component]
fn MessagesThreadButton(
    id: String,
    thread_id: String,
    address: String,
    title: String,
    snippet: String,
    date: i64,
    selected: bool,
    mut selected_message: Signal<Option<(String, String, String)>>,
    mut status: Signal<String>,
) -> Element {
    rsx! {
        button {
            class: if selected { "sms-thread-button selected" } else { "sms-thread-button" },
            onclick: move |_| {
                let (id, thread_id, address) = (id.clone(), thread_id.clone(), address.clone());
                selected_message.set(Some((id.clone(), thread_id.clone(), address)));
                status.set("Loading SMS conversation…".into());
                spawn(async move {
                    if let Ok(client) = DaemonClient::discover() {
                        if let Err(error) = client.messages_thread(&id, thread_id).await {
                            status.set(format!("Could not load conversation: {error}"));
                        }
                    }
                });
            },
            div {class:"sms-thread-title-row",span { class: "sms-thread-address", "{title}" }span {class:"sms-thread-time","{format_message_time(date)}"}}
            span { class: "sms-thread-snippet", "{snippet}" }
            span {class:"sms-thread-number","{address}"}
        }
    }
}

fn format_message_time(timestamp_ms: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(timestamp_ms);
    let elapsed = now_ms.saturating_sub(timestamp_ms).max(0);
    if elapsed < 60_000 {
        "now".into()
    } else if elapsed < 3_600_000 {
        format!("{}m ago", elapsed / 60_000)
    } else if elapsed < 86_400_000 {
        format!("{}h ago", elapsed / 3_600_000)
    } else if elapsed < 172_800_000 {
        "Yesterday".into()
    } else {
        format!("{}d ago", elapsed / 86_400_000)
    }
}

#[component]
fn SendSmsButton(
    id: String,
    address: String,
    body: String,
    draft: Signal<String>,
    mut pending: Signal<HashMap<String, (String, String, String)>>,
    mut status: Signal<String>,
) -> Element {
    let mut sending = use_signal(|| false);
    let disabled = *sending.read()
        || address.trim().is_empty()
        || body.trim().is_empty()
        || body.len() > 4096
        || pending
            .read()
            .values()
            .any(|(peer, recipient, text)| peer == &id && recipient == &address && text == &body);
    rsx! {
        button { disabled, onclick: move |_| {
            let (id, address, body) = (id.clone(), address.clone(), body.clone());
            sending.set(true);
            status.set("Sending request to phone…".into());
            spawn(async move {
                let result = async { DaemonClient::discover()?.messages_send(&id, address.clone(), body.clone()).await }.await;
                match result {
                    Ok(request_id) => { pending.write().insert(request_id, (id, address, body)); }
                    Err(error) => status.set(format!("Could not send message: {error}")),
                }
                sending.set(false);
            });
        }, if *sending.read() {"Sending…"} else if address.contains(';') || address.contains(',') {"Send group MMS"} else {"Send"} }
    }
}

#[component]
fn ContactRecipientButton(
    name: String,
    address: String,
    mut recipients: Signal<String>,
) -> Element {
    let selected = recipients
        .read()
        .split(';')
        .any(|value| value.trim() == address.trim());
    rsx! { button { class:if selected {"sms-contact selected"} else {"sms-contact"},
        onclick: move |_| {
            let mut values = recipients.read().split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect::<Vec<_>>();
            if values.iter().any(|v| v == &address) { values.retain(|v| v != &address); }
            else { values.push(address.clone()); }
            recipients.set(values.join("; "));
        }, strong {"{name}"} span {"{address}"} if selected {span {"Selected ✓"}} }
    }
}

#[component]
fn ContactPageButton(
    id: String,
    query: String,
    offset: u32,
    mut status: Signal<String>,
) -> Element {
    rsx! { button {onclick:move |_| {let (id, query)=(id.clone(),query.clone());spawn(async move {
        if let Ok(client)=DaemonClient::discover() {if let Err(e)=client.messages_contacts(&id,query,offset).await {status.set(e.to_string());}}
    });},"More contacts"} }
}

#[component]
fn MessagePageButton(
    id: String,
    thread_id: Option<String>,
    offset: u32,
    label: String,
    mut status: Signal<String>,
) -> Element {
    rsx! { button {class:"secondary",onclick:move |_| {let (id, thread_id)=(id.clone(),thread_id.clone());spawn(async move {
        if let Ok(client)=DaemonClient::discover() {if let Err(e)=client.messages_page(&id,thread_id,offset).await {status.set(e.to_string());}}
    });},"{label}"} }
}

/// Join pages by provider IDs, retaining older loaded history during live refreshes.
fn merge_message_page(
    old: &link_core::pair_session::MessagesPacket,
    new: &mut link_core::pair_session::MessagesPacket,
) {
    let append = new.offset.unwrap_or(0) > 0;
    if new.op == "messages_thread_result" && !new.messages.is_empty() {
        let oldest = new.messages.iter().map(|m| m.date).min().unwrap_or(0);
        let ids = new
            .messages
            .iter()
            .map(|m| m.id.clone())
            .collect::<HashSet<_>>();
        new.messages.extend(
            old.messages
                .iter()
                .filter(|m| !ids.contains(&m.id) && (append || m.date <= oldest))
                .cloned(),
        );
        new.messages
            .sort_by(|a, b| (a.date, &a.id).cmp(&(b.date, &b.id)));
        if !append && old.messages.len() > 100 {
            new.next_offset = old.next_offset;
        }
    } else if new.op == "messages_list_result" && !new.threads.is_empty() {
        let oldest = new.threads.iter().map(|m| m.date).min().unwrap_or(0);
        let ids = new
            .threads
            .iter()
            .map(|m| m.id.clone())
            .collect::<HashSet<_>>();
        new.threads.extend(
            old.threads
                .iter()
                .filter(|m| !ids.contains(&m.id) && (append || m.date <= oldest))
                .cloned(),
        );
        new.threads
            .sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.id.cmp(&b.id)));
        if !append && old.threads.len() > 100 {
            new.next_offset = old.next_offset;
        }
    } else if new.op == "messages_contacts_result" && append && old.query == new.query {
        let mut contacts = old.contacts.clone();
        let ids = contacts
            .iter()
            .map(|c| c.id.clone())
            .collect::<HashSet<_>>();
        contacts.extend(
            new.contacts
                .iter()
                .filter(|c| !ids.contains(&c.id))
                .cloned(),
        );
        new.contacts = contacts;
    }
}

#[component]
fn PhotoViewer(image_src: String, photo_name: String, dark_mode: bool) -> Element {
    let window = dioxus::desktop::use_window();
    rsx! {
        style { "{PHOTO_VIEWER_STYLE}" }
        div {class:if dark_mode {"viewer theme-dark"} else {"viewer"},
            header {class:"viewer-bar",
                div {class:"viewer-name","{photo_name}" span {class:"viewer-hint","Shared from Android"}}
                button {class:"viewer-close",title:"Close photo viewer",onclick:move |_| window.close(),"Close"}
            }
            main {class:"viewer-stage",img {class:"viewer-image",src:"{image_src}",alt:"{photo_name}"}}
        }
    }
}
#[component]
fn BeginButton(address: String, mut status: Signal<String>, mut busy: Signal<bool>) -> Element {
    let disabled = *busy.read() || address.is_empty();
    rsx! {button {disabled,onclick:move |_| {
        let peer=address.clone();busy.set(true);status.set("Requesting pairing…".into());
        spawn(async move {
            let result=async {DaemonClient::discover()?.begin_pair(&peer).await}.await;
            status.set(match result {Ok(session)=>format!("Compare code {} on both devices.",session.code),Err(e)=>format!("Pairing failed: {e}")});busy.set(false);
        });
    },"Pair device"}}
}
#[component]
fn GalleryButton(
    id: String,
    name: String,
    offset: u32,
    label: String,
    mut status: Signal<String>,
) -> Element {
    rsx! {button {class:"secondary",onclick:move |_| {let id=id.clone();let name=name.clone();let request_id=link_core::DeviceId::generate().to_string();spawn(async move {let result=async {DaemonClient::discover()?.gallery_list(&id,request_id,offset).await}.await;status.set(match result {Ok(())=>format!("Loading {name} gallery…"),Err(e)=>format!("Gallery request failed: {e}")});});},"{label}"}}
}
#[component]
fn GalleryFetchButton(
    id: String,
    photo_id: String,
    photo_name: String,
    mut status: Signal<String>,
) -> Element {
    rsx! {button {class:"secondary",onclick:move |_| {let id=id.clone();let photo_id=photo_id.clone();let photo_name=photo_name.clone();let request_id=link_core::DeviceId::generate().to_string();status.set(format!("Opening {photo_name}…"));spawn(async move {let result=async {DaemonClient::discover()?.gallery_fetch(&id,request_id,photo_id).await}.await;if let Err(e)=result {status.set(format!("Could not open photo: {e}"));}});},"View photo"}}
}
#[component]
fn DecisionButton(
    session: String,
    code: String,
    accept: bool,
    mut status: Signal<String>,
    mut busy: Signal<bool>,
) -> Element {
    rsx! {button {class:if accept {""} else {"danger"},disabled:*busy.read(),onclick:move |_| {
        let (session,code)=(session.clone(),code.clone());busy.set(true);
        spawn(async move {
            let result=async {DaemonClient::discover()?.confirm_pair(&session,&code,accept).await}.await;
            status.set(match result {Ok(()) if accept=>"Confirmed here. Confirm the matching code on the other device.".into(),Ok(())=>"Pairing rejected".into(),Err(e)=>format!("Confirmation failed: {e}")});busy.set(false);
        });
    },if accept {"Codes match — confirm"} else {"Reject"}}}
}
#[component]
fn PingButton(address: String, fingerprint: String, mut status: Signal<String>) -> Element {
    let mut busy = use_signal(|| false);
    rsx! {button {class:"secondary",disabled:*busy.read(),onclick:move |_| {
        let (peer,fp)=(address.clone(),fingerprint.clone());busy.set(true);
        spawn(async move {
            let result=async {DaemonClient::discover()?.ping(&peer,&fp).await}.await;
            status.set(match result {Ok(reply) if reply.nonce_ok=>format!("Connected to {} over TLS ({}ms)",reply.pong_from,reply.elapsed_ms),Ok(_)=>"Ping nonce mismatch".into(),Err(e)=>format!("Connection failed: {e}")});busy.set(false);
        });
    },"Ping"}}
}
#[component]
fn UnpairButton(id: String, mut status: Signal<String>) -> Element {
    let mut busy = use_signal(|| false);
    rsx! {button {class:"danger",disabled:*busy.read(),onclick:move |_| {
        let id=id.clone();busy.set(true);
        spawn(async move {
            let result=async {DaemonClient::discover()?.unpair(&id).await}.await;
            status.set(match result {Ok(())=>"Device unpaired".into(),Err(e)=>format!("Could not unpair: {e}")});busy.set(false);
        });
    },"Unpair"}}
}

#[cfg(test)]
mod messaging_tests {
    use super::merge_message_page;
    use link_core::pair_session::MessagesPacket;
    fn page(offset: u32, start: i64, end: i64, next: Option<u32>) -> MessagesPacket {
        serde_json::from_value(serde_json::json!({
            "op":"messages_thread_result", "from":link_core::DeviceId::generate(),
            "request_id":"test", "thread_id":"1", "offset":offset,"next_offset":next,
            "messages":(start..end).map(|n|serde_json::json!({"id":format!("sms:{n}"),
                "address":"5554","body":"fixture","date":n,"direction":"inbound"})).collect::<Vec<_>>()
        })).unwrap()
    }
    #[test]
    fn history_pages_and_live_refresh_keep_older_messages_without_duplicates() {
        let recent = page(0, 50, 150, Some(100));
        let mut older = page(100, 0, 55, None); // Page overlaps after five new arrivals.
        merge_message_page(&recent, &mut older);
        assert_eq!(older.messages.len(), 150);
        assert_eq!(older.messages.first().unwrap().date, 0);
        let mut refresh = page(0, 55, 155, Some(100));
        merge_message_page(&older, &mut refresh);
        assert_eq!(refresh.messages.len(), 155);
        assert_eq!(refresh.next_offset, None);
        assert!(refresh.messages.windows(2).all(|p| p[0].date < p[1].date));
    }
    #[test]
    fn empty_refresh_clears_deleted_conversation() {
        let old = page(0, 0, 100, None);
        let mut empty = page(0, 0, 0, None);
        merge_message_page(&old, &mut empty);
        assert!(empty.messages.is_empty());
    }
}
