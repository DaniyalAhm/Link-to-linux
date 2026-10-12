//! Private Unix-socket API. Pairing connections remain owned by the daemon
//! while either UI waits for the user's decision.
use link_core::pair_session::PairManager;
use link_core::presence::PresenceTracker;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

pub use link_core::ipc_wire::Request;
pub fn socket_path() -> PathBuf {
    link_core::platform::linux_socket_path()
}
pub async fn request(request: &Request) -> anyhow::Result<Value> {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let mut socket = tokio::net::UnixStream::connect(socket_path())
            .await
            .map_err(|e| anyhow::anyhow!("cannot reach daemon; start `link-daemon serve`: {e}"))?;
        link_core::transport::write_frame(&mut socket, &serde_json::to_vec(request)?).await?;
        let bytes = link_core::transport::read_frame(&mut socket).await?;
        let result: Value = serde_json::from_slice(&bytes)?;
        if result["ok"] != true {
            anyhow::bail!(
                "{}",
                result["error"].as_str().unwrap_or("daemon request failed")
            );
        }
        Ok(result)
    })
    .await
    .map_err(|_| anyhow::anyhow!("daemon request timed out"))?
}
pub async fn status() -> anyhow::Result<()> {
    println!("{}", request(&Request::Status).await?);
    Ok(())
}
struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
pub async fn bind() -> anyhow::Result<tokio::net::UnixListener> {
    let path = socket_path();
    if tokio::net::UnixStream::connect(&path).await.is_ok() {
        anyhow::bail!("another daemon already serves {}", path.display());
    }
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}
pub async fn serve(
    listener: tokio::net::UnixListener,
    manager: Arc<PairManager>,
    tracker: Arc<PresenceTracker>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let _guard = SocketGuard(socket_path());
    loop {
        tokio::select! {
            _ = stop.wait_for(|s| *s) => return Ok(()),
            accepted = listener.accept() => {
                let (mut socket,_) = accepted?;
                let manager = manager.clone();
                let tracker = tracker.clone();
                tokio::spawn(async move {
                    let result = tokio::time::timeout(std::time::Duration::from_secs(15),async {
                        let bytes = link_core::transport::read_frame(&mut socket).await?;
                        if bytes.len()>8 * 1024 * 1024 {anyhow::bail!("IPC request too large");}
                        let request: Request = serde_json::from_slice(&bytes)?;
                        let result = match request {
                            Request::Status => json!({"ok":true,"identity":manager.local}),
                            Request::Devices => {tracker.prune()?;json!({"ok":true,"devices":tracker.snapshot()?})},
                            Request::Begin {peer} => json!({"ok":true,"pairing":manager.begin(peer).await?}),
                            Request::Sessions => json!({"ok":true,"sessions":manager.sessions()?}),
                            Request::Confirm {session,code,accept} => json!({"ok":true,"pairing":manager.confirm(&session,&code,accept)?}),
                            Request::Pairs => json!({"ok":true,"pairings":manager.pairs()?}),
                            Request::Unpair {id} => {manager.unpair(&id.parse()?)?;json!({"ok":true})},
                            Request::ClipboardEnabled {enabled} => {manager.set_clipboard_enabled(enabled);json!({"ok":true,"enabled":enabled})},
                            Request::ClipboardStatus => json!({"ok":true,"enabled":manager.clipboard_enabled()}),
                            Request::ClipboardTake => json!({"ok":true,"enabled":manager.clipboard_enabled(),"clipboard":manager.take_clipboard()?}),
                            Request::ClipboardSend {id,mime,data_b64} => {manager.send_clipboard(&id.parse()?,mime,data_b64).await?;json!({"ok":true})},
                            Request::GalleryList {id,request_id,offset} => {let id: link_core::DeviceId=id.parse()?; let peer=manager.paired_device(&id)?.ok_or_else(|| anyhow::anyhow!("device is not paired"))?; let address=peer.address.ok_or_else(|| anyhow::anyhow!("paired device has no saved address"))?; manager.send_gallery_at(&id,address,link_core::pair_session::GalleryPacket{op:"gallery_list".into(),from:manager.local.id,request_id,offset:Some(offset),items:vec![],next_offset:None,photo_id:None,name:None,mime:None,data_b64:None,error:None}).await?;json!({"ok":true})},
                            Request::GalleryFetch {id,request_id,photo_id} => {let id: link_core::DeviceId=id.parse()?; let peer=manager.paired_device(&id)?.ok_or_else(|| anyhow::anyhow!("device is not paired"))?; let address=peer.address.ok_or_else(|| anyhow::anyhow!("paired device has no saved address"))?; manager.send_gallery_at(&id,address,link_core::pair_session::GalleryPacket{op:"gallery_fetch".into(),from:manager.local.id,request_id,offset:None,items:vec![],next_offset:None,photo_id:Some(photo_id),name:None,mime:None,data_b64:None,error:None}).await?;json!({"ok":true})},
                            Request::GalleryTake => json!({"ok":true,"gallery":manager.take_gallery()?}),
                            Request::MessagesList {id,offset} => {let id: link_core::DeviceId=id.parse()?; let peer=manager.paired_device(&id)?.ok_or_else(|| anyhow::anyhow!("device is not paired"))?; let address=peer.address.ok_or_else(|| anyhow::anyhow!("paired device has no saved address"))?; manager.send_messages_at(&id,address,link_core::pair_session::MessagesPacket{op:"messages_list".into(),from:manager.local.id,request_id:link_core::DeviceId::generate().to_string(),threads:vec![],messages:vec![],thread_id:None,address:None,body:None,error:None,enabled:None,display_name:None,device_name:None,contacts:vec![],offset:Some(offset),next_offset:None,query:None}).await?;json!({"ok":true})},
                            Request::MessagesContacts {id,query,offset} => {let id: link_core::DeviceId=id.parse()?; let peer=manager.paired_device(&id)?.ok_or_else(|| anyhow::anyhow!("device is not paired"))?; let address=peer.address.ok_or_else(|| anyhow::anyhow!("paired device has no saved address"))?; manager.send_messages_at(&id,address,link_core::pair_session::MessagesPacket{op:"messages_contacts".into(),from:manager.local.id,request_id:link_core::DeviceId::generate().to_string(),threads:vec![],messages:vec![],thread_id:None,address:None,body:None,error:None,enabled:None,display_name:None,device_name:None,contacts:vec![],offset:Some(offset),next_offset:None,query:Some(query)}).await?;json!({"ok":true})},
                            Request::MessagesStatus {id} => {let id: link_core::DeviceId=id.parse()?; let peer=manager.paired_device(&id)?.ok_or_else(|| anyhow::anyhow!("device is not paired"))?; let address=peer.address.ok_or_else(|| anyhow::anyhow!("paired device has no saved address"))?; manager.send_messages_at(&id,address,link_core::pair_session::MessagesPacket{op:"messages_status".into(),from:manager.local.id,request_id:link_core::DeviceId::generate().to_string(),threads:vec![],messages:vec![],thread_id:None,address:None,body:None,error:None,enabled:None,display_name:None,device_name:None,contacts:vec![],offset:None,next_offset:None,query:None}).await?;json!({"ok":true})},
                            Request::MessagesThread {id,thread_id,offset} => {let id: link_core::DeviceId=id.parse()?; let peer=manager.paired_device(&id)?.ok_or_else(|| anyhow::anyhow!("device is not paired"))?; let address=peer.address.ok_or_else(|| anyhow::anyhow!("paired device has no saved address"))?; manager.send_messages_at(&id,address,link_core::pair_session::MessagesPacket{op:"messages_thread".into(),from:manager.local.id,request_id:link_core::DeviceId::generate().to_string(),threads:vec![],messages:vec![],thread_id:Some(thread_id),address:None,body:None,error:None,enabled:None,display_name:None,device_name:None,contacts:vec![],offset:Some(offset),next_offset:None,query:None}).await?;json!({"ok":true})},
                            Request::MessagesSend {id,address,body} => {if address.trim().is_empty() || address.len()>2048 || body.trim().is_empty() || body.len()>4096 {anyhow::bail!("SMS address or message is invalid");} let id: link_core::DeviceId=id.parse()?; let peer=manager.paired_device(&id)?.ok_or_else(|| anyhow::anyhow!("device is not paired"))?; let transport=peer.address.ok_or_else(|| anyhow::anyhow!("paired device has no saved address"))?; let request_id=link_core::DeviceId::generate().to_string(); manager.send_messages_at(&id,transport,link_core::pair_session::MessagesPacket{op:"messages_send".into(),from:manager.local.id,request_id:request_id.clone(),threads:vec![],messages:vec![],thread_id:None,address:Some(address),body:Some(body),error:None,enabled:None,display_name:None,device_name:None,contacts:vec![],offset:None,next_offset:None,query:None}).await?;json!({"ok":true,"request_id":request_id})},
                            Request::MessagesTake => json!({"ok":true,"messages":manager.take_messages()?}),
                        };
                        Ok::<Value,anyhow::Error>(result)
                    }).await;
                    let reply = match result {Ok(Ok(v))=>v,Ok(Err(e))=>json!({"ok":false,"error":e.to_string()}),Err(_)=>json!({"ok":false,"error":"request timed out"})};
                    if let Ok(bytes) = serde_json::to_vec(&reply) {let _ = link_core::transport::write_frame(&mut socket,&bytes).await;}
                });
            }
        }
    }
}
