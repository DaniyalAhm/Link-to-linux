//! Two-sided pairing. The display code is bound to this mutually authenticated
//! TLS connection using an exporter; it is never transmitted over the network.
use crate::pairing::{PairedDevice, PairingStore};
use crate::{CoreError, DeviceInfo};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

pub const PAIR_TIMEOUT_SECS: u64 = 120;
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 32 * 1024;
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub fn supported_clipboard_mime(mime: &str) -> bool {
    mime == "text/plain" || matches!(mime, "image/png" | "image/jpeg" | "image/webp")
}
fn valid_clipboard_data(mime: &str, data: &[u8]) -> bool {
    match mime {
        "text/plain" => std::str::from_utf8(data).is_ok() && data.len() <= MAX_CLIPBOARD_TEXT_BYTES,
        "image/png" => {
            data.starts_with(b"\x89PNG\r\n\x1a\n") && data.len() <= MAX_CLIPBOARD_IMAGE_BYTES
        }
        "image/jpeg" => {
            data.starts_with(&[0xff, 0xd8, 0xff]) && data.len() <= MAX_CLIPBOARD_IMAGE_BYTES
        }
        "image/webp" => {
            data.len() >= 12
                && &data[..4] == b"RIFF"
                && &data[8..12] == b"WEBP"
                && data.len() <= MAX_CLIPBOARD_IMAGE_BYTES
        }
        _ => false,
    }
}
const MAX_PENDING: usize = 8;
const MAX_PENDING_CLIPBOARD: usize = 8;
const MAX_PENDING_NOTIFICATIONS: usize = 32;
const MAX_PENDING_GALLERY: usize = 16;
const MAX_PENDING_MESSAGES: usize = 16;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PairHello {
    pub op: String,
    pub session: String,
    pub info: DeviceInfo,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Decision {
    op: String,
    session: String,
    accept: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairStatus {
    Awaiting,
    Waiting,
    Paired,
    Rejected,
    Expired,
    Failed,
}
impl PairStatus {
    pub fn pending(self) -> bool {
        matches!(self, Self::Awaiting | Self::Waiting)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PairSession {
    pub session: String,
    pub code: String,
    pub peer: PairedDevice,
    pub incoming: bool,
    pub status: PairStatus,
    pub expires_in_secs: u64,
    pub error: Option<String>,
}
struct Entry {
    view: PairSession,
    created: Instant,
    decision: watch::Sender<Option<bool>>,
}

pub struct PairManager {
    pub local: DeviceInfo,
    pub identity: crate::tls::Identity,
    store: Arc<dyn PairingStore>,
    entries: Mutex<HashMap<String, Entry>>,
    clipboard_enabled: std::sync::atomic::AtomicBool,
    clipboard_pending: Mutex<std::collections::VecDeque<(String, String, String)>>,
    notification_pending: Mutex<std::collections::VecDeque<NotificationPacket>>,
    gallery_pending: Mutex<std::collections::VecDeque<GalleryPacket>>,
    messages_pending: Mutex<std::collections::VecDeque<MessagesPacket>>,
}
impl PairManager {
    pub fn new(
        local: DeviceInfo,
        identity: crate::tls::Identity,
        store: Arc<dyn PairingStore>,
    ) -> Self {
        Self {
            local,
            identity,
            store,
            entries: Mutex::new(HashMap::new()),
            clipboard_enabled: std::sync::atomic::AtomicBool::new(false),
            clipboard_pending: Mutex::new(std::collections::VecDeque::new()),
            notification_pending: Mutex::new(std::collections::VecDeque::new()),
            gallery_pending: Mutex::new(std::collections::VecDeque::new()),
            messages_pending: Mutex::new(std::collections::VecDeque::new()),
        }
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Entry>>, CoreError> {
        self.entries
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))
    }
    pub fn sessions(&self) -> Result<Vec<PairSession>, CoreError> {
        let mut entries = self.lock()?;
        entries.retain(|_, e| e.created.elapsed() < Duration::from_secs(300));
        let mut result = Vec::new();
        for entry in entries.values_mut() {
            let remaining = Duration::from_secs(PAIR_TIMEOUT_SECS)
                .saturating_sub(entry.created.elapsed())
                .as_secs();
            if entry.created.elapsed() >= Duration::from_secs(PAIR_TIMEOUT_SECS)
                && entry.view.status.pending()
            {
                entry.view.status = PairStatus::Expired;
                entry.decision.send_replace(Some(false));
            }
            let mut view = entry.view.clone();
            view.expires_in_secs = remaining;
            result.push(view);
        }
        result.sort_by(|a, b| a.session.cmp(&b.session));
        Ok(result)
    }
    pub fn confirm(
        &self,
        session: &str,
        code: &str,
        accept: bool,
    ) -> Result<PairSession, CoreError> {
        let mut entries = self.lock()?;
        let entry = entries
            .get_mut(session)
            .ok_or_else(|| CoreError::InvalidData("pairing request no longer exists".into()))?;
        if entry.created.elapsed() >= Duration::from_secs(PAIR_TIMEOUT_SECS) {
            entry.view.status = PairStatus::Expired;
            entry.decision.send_replace(Some(false));
            return Err(CoreError::InvalidData("pairing code expired".into()));
        }
        if !entry.view.status.pending() {
            return Err(CoreError::InvalidData("pairing already finished".into()));
        }
        if accept && entry.view.code != code {
            return Err(CoreError::InvalidData(
                "confirmation code does not match".into(),
            ));
        }
        if entry.decision.borrow().is_some() {
            return Err(CoreError::InvalidData("this device already decided".into()));
        }
        entry.view.status = if accept {
            PairStatus::Waiting
        } else {
            PairStatus::Rejected
        };
        entry.decision.send_replace(Some(accept));
        Ok(entry.view.clone())
    }
    pub fn pairs(&self) -> Result<Vec<PairedDevice>, CoreError> {
        let _guard = self.lock()?;
        self.store.list()
    }
    pub fn clipboard_enabled(&self) -> bool {
        self.clipboard_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn set_clipboard_enabled(&self, enabled: bool) {
        self.clipboard_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn take_clipboard(&self) -> Result<Option<(String, String, String)>, CoreError> {
        Ok(self
            .clipboard_pending
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .pop_front())
    }
    pub fn take_notification(&self) -> Result<Option<NotificationPacket>, CoreError> {
        Ok(self
            .notification_pending
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .pop_front())
    }
    pub fn take_gallery(&self) -> Result<Option<GalleryPacket>, CoreError> {
        Ok(self
            .gallery_pending
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .pop_front())
    }
    pub fn take_messages(&self) -> Result<Option<MessagesPacket>, CoreError> {
        Ok(self
            .messages_pending
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .pop_front())
    }
    pub async fn send_messages_at(
        &self,
        id: &crate::DeviceId,
        address: String,
        packet: MessagesPacket,
    ) -> Result<(), CoreError> {
        let peer = self
            .store
            .get(id)?
            .ok_or_else(|| CoreError::InvalidData("device is not paired".into()))?;
        if packet.from != self.local.id {
            return Err(CoreError::InvalidData("invalid messages sender".into()));
        }
        crate::tls::tls_send_messages(&address, &self.identity, &peer.cert_fingerprint, &packet)
            .await
    }
    pub fn receive_messages(&self, packet: MessagesPacket, cert_fp: &str) -> Result<(), CoreError> {
        if packet.request_id.is_empty()
            || packet.request_id.len() > 128
            || packet.op.len() > 32
            || packet.threads.len() > 100
            || packet.messages.len() > 100
            || packet.address.as_ref().is_some_and(|v| v.len() > 2048)
            || packet.thread_id.as_ref().is_some_and(|v| v.len() > 128)
            || packet.body.as_ref().is_some_and(|v| v.len() > 4096)
            || packet.error.as_ref().is_some_and(|v| v.len() > 512)
            || packet.display_name.as_ref().is_some_and(|v| v.len() > 512)
            || packet.device_name.as_ref().is_some_and(|v| v.len() > 128)
            || packet.contacts.len() > 100
            || packet.offset.is_some_and(|v| v > 100000)
            || packet.query.as_ref().is_some_and(|v| v.len() > 128)
            || packet
                .contacts
                .iter()
                .any(|c| c.id.len() > 128 || c.name.len() > 256 || c.address.len() > 128)
            || packet.threads.iter().any(|t| {
                t.id.len() > 128
                    || t.address.len() > 2048
                    || t.display_name.as_ref().is_some_and(|v| v.len() > 512)
                    || t.snippet.len() > 512
            })
            || packet.messages.iter().any(|m| {
                m.id.len() > 128
                    || m.address.len() > 128
                    || m.body.len() > 32768
                    || m.state.as_ref().is_some_and(|v| v.len() > 32)
                    || m.sender_name.as_ref().is_some_and(|v| v.len() > 256)
                    || !matches!(m.direction.as_str(), "inbound" | "outbound")
            })
        {
            return Err(CoreError::InvalidData("invalid messages packet".into()));
        }
        if !matches!(
            packet.op.as_str(),
            "messages_list"
                | "messages_contacts"
                | "messages_contacts_result"
                | "messages_sent"
                | "messages_status"
                | "messages_thread"
                | "messages_send"
                | "messages_list_result"
                | "messages_status_result"
                | "messages_thread_result"
                | "messages_send_result"
                | "messages_error"
        ) {
            return Err(CoreError::InvalidData("unknown messages operation".into()));
        }
        let peer = self
            .store
            .get(&packet.from)?
            .ok_or_else(|| CoreError::InvalidData("messages sender is not paired".into()))?;
        if peer.cert_fingerprint != cert_fp {
            return Err(CoreError::InvalidData(
                "messages certificate is not paired".into(),
            ));
        }
        // Android reports its user-facing system device name with status
        // responses. Keep the trusted record in sync so the desktop can show
        // that name outside the Messages view too (and across restarts).
        if packet.op == "messages_status_result" {
            if let Some(name) = packet
                .device_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                if peer.name != name {
                    let mut updated = peer;
                    updated.name = name.to_owned();
                    self.store.insert(updated)?;
                }
            }
        }
        let mut queue = self
            .messages_pending
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        if queue.len() >= MAX_PENDING_MESSAGES {
            return Err(CoreError::InvalidData("messages queue is full".into()));
        }
        queue.push_back(packet);
        Ok(())
    }
    pub async fn send_gallery_at(
        &self,
        id: &crate::DeviceId,
        address: String,
        packet: GalleryPacket,
    ) -> Result<(), CoreError> {
        let peer = self
            .store
            .get(id)?
            .ok_or_else(|| CoreError::InvalidData("device is not paired".into()))?;
        if packet.from != self.local.id {
            return Err(CoreError::InvalidData("invalid gallery sender".into()));
        }
        crate::tls::tls_send_gallery(&address, &self.identity, &peer.cert_fingerprint, &packet)
            .await
    }
    pub fn receive_gallery(&self, packet: GalleryPacket, cert_fp: &str) -> Result<(), CoreError> {
        if packet.request_id.is_empty()
            || packet.request_id.len() > 128
            || packet.op.len() > 32
            || packet.items.len() > 40
            || packet.error.as_ref().is_some_and(|error| error.len() > 512)
        {
            return Err(CoreError::InvalidData("invalid gallery message".into()));
        }
        if let Some(data) = &packet.data_b64 {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| CoreError::InvalidData("invalid gallery image encoding".into()))?;
            let mime = packet.mime.as_deref().unwrap_or("");
            if bytes.len() > MAX_CLIPBOARD_IMAGE_BYTES
                || !matches!(mime, "image/png" | "image/jpeg" | "image/webp")
                || !valid_clipboard_data(mime, &bytes)
            {
                return Err(CoreError::InvalidData(
                    "gallery image is unsupported or too large".into(),
                ));
            }
        }
        if packet.items.iter().any(|item| {
            item.id.len() > 128
                || item.name.len() > 512
                || item.mime.len() > 128
                || !matches!(
                    item.mime.as_str(),
                    "image/png"
                        | "image/jpeg"
                        | "image/webp"
                        | "image/heic"
                        | "image/heif"
                        | "image/avif"
                )
        }) {
            return Err(CoreError::InvalidData(
                "invalid gallery item metadata".into(),
            ));
        }
        for item in &packet.items {
            if let (Some(mime), Some(encoded)) = (&item.thumbnail_mime, &item.thumbnail_b64) {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| {
                        CoreError::InvalidData("invalid gallery thumbnail encoding".into())
                    })?;
                if bytes.len() > 48 * 1024
                    || !matches!(mime.as_str(), "image/jpeg" | "image/webp" | "image/png")
                    || !valid_clipboard_data(mime, &bytes)
                {
                    return Err(CoreError::InvalidData(
                        "gallery thumbnail is invalid or too large".into(),
                    ));
                }
            } else if item.thumbnail_mime.is_some() || item.thumbnail_b64.is_some() {
                return Err(CoreError::InvalidData(
                    "incomplete gallery thumbnail".into(),
                ));
            }
        }
        if !matches!(
            packet.op.as_str(),
            "gallery_list"
                | "gallery_list_result"
                | "gallery_fetch"
                | "gallery_image"
                | "gallery_error"
        ) {
            return Err(CoreError::InvalidData("unknown gallery operation".into()));
        }
        let peer = self
            .store
            .get(&packet.from)?
            .ok_or_else(|| CoreError::InvalidData("gallery sender is not paired".into()))?;
        if peer.cert_fingerprint != cert_fp {
            return Err(CoreError::InvalidData(
                "gallery certificate is not paired".into(),
            ));
        }
        let mut queue = self
            .gallery_pending
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        if queue.len() >= MAX_PENDING_GALLERY {
            return Err(CoreError::InvalidData(
                "gallery request queue is full".into(),
            ));
        }
        queue.push_back(packet);
        Ok(())
    }
    pub async fn send_notification_at(
        &self,
        id: &crate::DeviceId,
        address: String,
        notification_key: String,
        app_name: String,
        title: String,
        body: String,
    ) -> Result<(), CoreError> {
        if notification_key.is_empty()
            || notification_key.len() > 512
            || app_name.len() > 256
            || title.len() > 512
            || body.len() > 4096
        {
            return Err(CoreError::InvalidData("notification is too large".into()));
        }
        let peer = self
            .store
            .get(id)?
            .ok_or_else(|| CoreError::InvalidData("device is not paired".into()))?;
        crate::tls::tls_send_notification(
            &address,
            &self.identity,
            &peer.cert_fingerprint,
            &NotificationPacket {
                op: "notification".into(),
                id: notification_key,
                from: self.local.id,
                app_name,
                title,
                body,
            },
        )
        .await
    }
    pub fn receive_notification(
        &self,
        packet: NotificationPacket,
        cert_fp: &str,
    ) -> Result<(), CoreError> {
        if packet.op != "notification"
            || packet.id.is_empty()
            || packet.id.len() > 512
            || packet.app_name.len() > 256
            || packet.title.len() > 512
            || packet.body.len() > 4096
        {
            return Err(CoreError::InvalidData("invalid notification".into()));
        }
        let peer = self
            .store
            .get(&packet.from)?
            .ok_or_else(|| CoreError::InvalidData("notification sender is not paired".into()))?;
        if peer.cert_fingerprint != cert_fp {
            return Err(CoreError::InvalidData(
                "notification certificate is not paired".into(),
            ));
        }
        let mut pending = self
            .notification_pending
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        if let Some(existing) = pending.iter_mut().find(|entry| entry.id == packet.id) {
            *existing = packet;
        } else {
            if pending.len() >= MAX_PENDING_NOTIFICATIONS {
                pending.pop_front();
            }
            pending.push_back(packet);
        }
        Ok(())
    }
    pub fn paired_device(&self, id: &crate::DeviceId) -> Result<Option<PairedDevice>, CoreError> {
        self.store.get(id)
    }
    pub async fn send_clipboard(
        &self,
        id: &crate::DeviceId,
        mime: String,
        data_b64: String,
    ) -> Result<(), CoreError> {
        let peer = self
            .store
            .get(id)?
            .ok_or_else(|| CoreError::InvalidData("device is not paired".into()))?;
        let address = peer
            .address
            .ok_or_else(|| CoreError::InvalidData("paired device has no saved address".into()))?;
        self.send_clipboard_at(id, address, mime, data_b64).await
    }
    pub async fn send_clipboard_at(
        &self,
        id: &crate::DeviceId,
        address: String,
        mime: String,
        data_b64: String,
    ) -> Result<(), CoreError> {
        if !self.clipboard_enabled() {
            return Err(CoreError::InvalidData("clipboard sync is disabled".into()));
        }
        let data = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &data_b64)
            .map_err(|_| CoreError::InvalidData("invalid clipboard data encoding".into()))?;
        if !supported_clipboard_mime(&mime) || !valid_clipboard_data(&mime, &data) {
            return Err(CoreError::InvalidData(
                "unsupported clipboard MIME type or payload too large".into(),
            ));
        }
        let peer = self
            .store
            .get(id)?
            .ok_or_else(|| CoreError::InvalidData("device is not paired".into()))?;
        crate::tls::tls_send_clipboard(
            &address,
            &self.identity,
            &peer.cert_fingerprint,
            &ClipboardPacket {
                op: "clip".into(),
                id: crate::DeviceId::generate().to_string(),
                from: self.local.id,
                mime,
                data_b64,
            },
        )
        .await
    }
    pub fn receive_clipboard(
        &self,
        packet: ClipboardPacket,
        cert_fp: &str,
    ) -> Result<(), CoreError> {
        if !self.clipboard_enabled() {
            return Err(CoreError::InvalidData("clipboard sync is disabled".into()));
        }
        if packet.op != "clip"
            || packet.id.parse::<crate::DeviceId>().is_err()
            || !supported_clipboard_mime(&packet.mime)
        {
            return Err(CoreError::InvalidData("invalid clipboard message".into()));
        }
        let data =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &packet.data_b64)
                .map_err(|_| CoreError::InvalidData("invalid clipboard data encoding".into()))?;
        if !valid_clipboard_data(&packet.mime, &data) {
            return Err(CoreError::InvalidData("clipboard payload too large".into()));
        }
        let peer = self
            .store
            .get(&packet.from)?
            .ok_or_else(|| CoreError::InvalidData("clipboard sender is not paired".into()))?;
        if peer.cert_fingerprint != cert_fp {
            return Err(CoreError::InvalidData(
                "clipboard certificate is not paired".into(),
            ));
        }
        let mut pending = self
            .clipboard_pending
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        // Keep clipboard updates ordered and never silently discard an older
        // item. A full queue rejects the newest packet so its sender can retry.
        if pending.len() >= MAX_PENDING_CLIPBOARD {
            return Err(CoreError::InvalidData(
                "clipboard queue is full; retry delivery".into(),
            ));
        }
        pending.push_back((packet.from.to_string(), packet.mime, packet.data_b64));
        Ok(())
    }
    pub fn unpair(&self, id: &crate::DeviceId) -> Result<(), CoreError> {
        let mut entries = self.lock()?;
        for entry in entries
            .values_mut()
            .filter(|e| e.view.peer.id == *id && e.view.status.pending())
        {
            entry.view.status = PairStatus::Rejected;
            entry.decision.send_replace(Some(false));
        }
        self.store.remove(id)
    }
    fn insert(
        &self,
        hello: &PairHello,
        cert_fp: &str,
        code: String,
        address: String,
        incoming: bool,
    ) -> Result<(PairSession, watch::Receiver<Option<bool>>), CoreError> {
        hello.info.validate()?;
        if hello.op != "pair"
            || hello.session.parse::<crate::DeviceId>().is_err()
            || hello.info.id == self.local.id
            || hello.info.pubkey_fingerprint != cert_fp
        {
            return Err(CoreError::InvalidData(
                "invalid pairing identity or certificate".into(),
            ));
        }
        let mut entries = self.lock()?;
        entries.retain(|_, e| e.created.elapsed() < Duration::from_secs(300));
        if entries
            .values()
            .filter(|e| {
                e.view.status.pending()
                    && e.created.elapsed() < Duration::from_secs(PAIR_TIMEOUT_SECS)
            })
            .count()
            >= MAX_PENDING
            || entries.contains_key(&hello.session)
        {
            return Err(CoreError::InvalidData(
                "too many pending requests or duplicate session".into(),
            ));
        }
        if let Some(old) = self.store.get(&hello.info.id)? {
            if old.cert_fingerprint != cert_fp {
                return Err(CoreError::InvalidData(
                    "trusted certificate changed; unpair first".into(),
                ));
            }
        }
        while entries.len() >= 32 {
            let oldest = entries
                .iter()
                .filter(|(_, e)| {
                    !e.view.status.pending()
                        || e.created.elapsed() >= Duration::from_secs(PAIR_TIMEOUT_SECS)
                })
                .min_by_key(|(_, e)| e.created)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                entries.remove(&id);
            } else {
                return Err(CoreError::InvalidData("too many pairing sessions".into()));
            }
        }
        let view = PairSession {
            session: hello.session.clone(),
            code,
            peer: PairedDevice {
                id: hello.info.id,
                name: hello.info.name.clone(),
                os: hello.info.os.clone(),
                cert_fingerprint: cert_fp.into(),
                address: Some(address),
            },
            incoming,
            status: PairStatus::Awaiting,
            expires_in_secs: PAIR_TIMEOUT_SECS,
            error: None,
        };
        let (decision, rx) = watch::channel(None);
        entries.insert(
            view.session.clone(),
            Entry {
                view: view.clone(),
                created: Instant::now(),
                decision,
            },
        );
        Ok((view, rx))
    }
    fn finish(&self, session: &str, result: Result<(), CoreError>) {
        if let Ok(mut entries) = self.lock() {
            if let Some(entry) = entries.get_mut(session) {
                // Expiry or local rejection cannot be undone by a late packet.
                if matches!(
                    entry.view.status,
                    PairStatus::Expired | PairStatus::Rejected
                ) {
                    return;
                }
                match result {
                    Ok(()) if entry.created.elapsed() < Duration::from_secs(PAIR_TIMEOUT_SECS) => {
                        match self.store.insert(entry.view.peer.clone()) {
                            Ok(()) => entry.view.status = PairStatus::Paired,
                            Err(e) => {
                                entry.view.status = PairStatus::Failed;
                                entry.view.error = Some(e.to_string());
                            }
                        }
                    }
                    Ok(()) => entry.view.status = PairStatus::Expired,
                    Err(e) => {
                        entry.view.status =
                            if entry.created.elapsed() >= Duration::from_secs(PAIR_TIMEOUT_SECS) {
                                PairStatus::Expired
                            } else {
                                PairStatus::Rejected
                            };
                        entry.view.error = Some(e.to_string());
                    }
                }
            }
        }
    }
    pub async fn begin(self: &Arc<Self>, address: String) -> Result<PairSession, CoreError> {
        let (mut stream, remote_fp) = crate::tls::connect_pairing(&address, &self.identity).await?;
        let session = crate::DeviceId::generate().to_string();
        let hello = PairHello {
            op: "pair".into(),
            session: session.clone(),
            info: self.local.clone(),
        };
        send(&mut stream, &hello).await?;
        let remote: PairHello = tokio::time::timeout(Duration::from_secs(5), receive(&mut stream))
            .await
            .map_err(|_| CoreError::Transport("pairing hello timeout".into()))??;
        if remote.session != session {
            return Err(CoreError::InvalidData("pairing session mismatch".into()));
        }
        let mut binding = [0u8; 32];
        stream
            .get_ref()
            .1
            .export_keying_material(
                &mut binding,
                b"EXPORTER-LinkToLinux-Pairing-v1",
                Some(session.as_bytes()),
            )
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        let (view, decision) = self.insert(&remote, &remote_fp, code(&binding), address, false)?;
        let manager = self.clone();
        tokio::spawn(async move {
            manager.exchange(stream, session, decision).await;
        });
        Ok(view)
    }
    pub async fn incoming(
        self: &Arc<Self>,
        mut stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        hello: PairHello,
        address: std::net::SocketAddr,
    ) -> Result<(), CoreError> {
        let cert = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .ok_or_else(|| {
                CoreError::InvalidData("pairing requires a client certificate".into())
            })?;
        let cert_fp = crate::tls::fingerprint(cert.as_ref());
        let mut binding = [0u8; 32];
        stream
            .get_ref()
            .1
            .export_keying_material(
                &mut binding,
                b"EXPORTER-LinkToLinux-Pairing-v1",
                Some(hello.session.as_bytes()),
            )
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        let peer_address =
            std::net::SocketAddr::new(address.ip(), hello.info.quic_port).to_string();
        let (_, decision) = self.insert(&hello, &cert_fp, code(&binding), peer_address, true)?;
        let session = hello.session;
        let reply = PairHello {
            op: "pair".into(),
            session: session.clone(),
            info: self.local.clone(),
        };
        if let Err(e) = send(&mut stream, &reply).await {
            self.finish(&session, Err(e));
            return Ok(());
        }
        self.exchange(stream, session, decision).await;
        Ok(())
    }
    async fn exchange<S>(
        &self,
        stream: S,
        session: String,
        mut decision: watch::Receiver<Option<bool>>,
    ) where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let approve = async {
            let accept = loop {
                if let Some(value) = *decision.borrow_and_update() {
                    break value;
                }
                decision
                    .changed()
                    .await
                    .map_err(|e| CoreError::Transport(e.to_string()))?;
            };
            send(
                &mut writer,
                &Decision {
                    op: "confirm".into(),
                    session: session.clone(),
                    accept,
                },
            )
            .await?;
            if !accept {
                return Err(CoreError::InvalidData("pairing rejected locally".into()));
            }
            Ok::<(), CoreError>(())
        };
        let remote = async {
            let reply: Decision = receive(&mut reader).await?;
            if reply.op != "confirm" || reply.session != session || !reply.accept {
                return Err(CoreError::InvalidData("pairing rejected by peer".into()));
            }
            Ok::<(), CoreError>(())
        };
        let result = tokio::time::timeout(Duration::from_secs(PAIR_TIMEOUT_SECS), async {
            tokio::try_join!(approve, remote).map(|_| ())
        })
        .await
        .unwrap_or_else(|_| Err(CoreError::InvalidData("pairing code expired".into())));
        self.finish(&session, result);
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClipboardPacket {
    pub op: String,
    pub id: String,
    pub from: crate::DeviceId,
    pub mime: String,
    pub data_b64: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NotificationPacket {
    pub op: String,
    /// Android's stable notification key, used by Linux to replace updates.
    pub id: String,
    pub from: crate::DeviceId,
    pub app_name: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GalleryItem {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub date_modified: i64,
    pub size: u64,
    #[serde(default)]
    pub thumbnail_mime: Option<String>,
    #[serde(default)]
    pub thumbnail_b64: Option<String>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GalleryPacket {
    pub op: String,
    pub from: crate::DeviceId,
    pub request_id: String,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub items: Vec<GalleryItem>,
    #[serde(default)]
    pub next_offset: Option<u32>,
    #[serde(default)]
    pub photo_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub data_b64: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MessageThread {
    pub id: String,
    pub address: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub snippet: String,
    pub date: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SmsMessage {
    pub id: String,
    pub address: String,
    pub body: String,
    pub date: i64,
    pub direction: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub sender_name: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MessageContact {
    pub id: String,
    pub name: String,
    pub address: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessagesPacket {
    pub op: String,
    pub from: crate::DeviceId,
    pub request_id: String,
    #[serde(default)]
    pub threads: Vec<MessageThread>,
    #[serde(default)]
    pub messages: Vec<SmsMessage>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub contacts: Vec<MessageContact>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub next_offset: Option<u32>,
    #[serde(default)]
    pub query: Option<String>,
}
fn code(binding: &[u8; 32]) -> String {
    format!(
        "{:06}",
        u32::from_be_bytes(binding[..4].try_into().unwrap()) % 1_000_000
    )
}
async fn send<W: AsyncWrite + Unpin, T: serde::Serialize>(
    stream: &mut W,
    value: &T,
) -> Result<(), CoreError> {
    crate::transport::write_frame(stream, &crate::codec::encode(value)?).await
}
async fn receive<R: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
    stream: &mut R,
) -> Result<T, CoreError> {
    let bytes = crate::transport::read_frame(stream).await?;
    if bytes.len() > 65536 {
        return Err(CoreError::InvalidData("pairing packet too large".into()));
    }
    crate::codec::decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::MemoryPairingStore;

    fn manager(name: &str) -> Arc<PairManager> {
        manager_with_store(name).0
    }
    fn manager_with_store(name: &str) -> (Arc<PairManager>, Arc<MemoryPairingStore>) {
        let key = rcgen::generate_simple_self_signed(vec!["link-to-linux".into()]).unwrap();
        let identity = crate::tls::Identity {
            cert_der: key.cert.der().to_vec(),
            key_der: key.key_pair.serialize_der(),
        };
        let local = DeviceInfo {
            id: crate::DeviceId::generate(),
            name: name.into(),
            os: "test".into(),
            protocol_version: crate::PROTOCOL_VERSION,
            quic_port: crate::QUIC_PORT,
            pubkey_fingerprint: identity.fingerprint(),
            capabilities: vec![crate::Capability::Pair],
        };
        let store = Arc::new(MemoryPairingStore::new());
        let manager = Arc::new(PairManager::new(local, identity, store.clone()));
        (manager, store)
    }

    #[test]
    fn messages_packet_shape_is_distinct_from_gallery_packets() {
        let from = crate::DeviceId::generate();
        let gallery = GalleryPacket {
            op: "gallery_list".into(),
            from,
            request_id: "request".into(),
            offset: Some(0),
            items: vec![],
            next_offset: None,
            photo_id: None,
            name: None,
            mime: None,
            data_b64: None,
            error: None,
        };
        assert!(
            serde_json::from_value::<MessagesPacket>(serde_json::to_value(gallery).unwrap())
                .is_err()
        );
        let messages = MessagesPacket {
            op: "messages_list".into(),
            from,
            request_id: "request".into(),
            threads: vec![],
            messages: vec![],
            thread_id: None,
            address: None,
            body: None,
            error: None,
            enabled: None,
            display_name: None,
            device_name: None,
            contacts: vec![],
            offset: None,
            next_offset: None,
            query: None,
        };
        assert!(
            serde_json::from_value::<GalleryPacket>(serde_json::to_value(messages).unwrap())
                .is_err()
        );
    }

    #[test]
    fn messages_responses_require_a_paired_certificate_and_are_queued() {
        let (manager, store) = manager_with_store("linux");
        let phone = crate::DeviceId::generate();
        let fingerprint = "sha256:phone";
        store
            .insert(PairedDevice {
                id: phone,
                name: "Android phone".into(),
                os: "android".into(),
                cert_fingerprint: fingerprint.into(),
                address: Some("192.0.2.10:53536".into()),
            })
            .unwrap();
        let packet = MessagesPacket {
            op: "messages_list_result".into(),
            from: phone,
            request_id: "request".into(),
            threads: vec![MessageThread {
                id: "42".into(),
                address: "+15551234567".into(),
                display_name: Some("Alice Example".into()),
                snippet: "hello".into(),
                date: 1,
            }],
            messages: vec![],
            thread_id: None,
            address: None,
            body: None,
            error: None,
            enabled: None,
            display_name: None,
            device_name: None,
            contacts: vec![],
            offset: None,
            next_offset: None,
            query: None,
        };
        assert!(manager
            .receive_messages(packet.clone(), "sha256:wrong")
            .is_err());
        manager.receive_messages(packet, fingerprint).unwrap();
        assert_eq!(
            manager.take_messages().unwrap().unwrap().threads[0].id,
            "42"
        );
        let status = MessagesPacket {
            op: "messages_status_result".into(),
            from: phone,
            request_id: "status".into(),
            threads: vec![],
            messages: vec![],
            thread_id: None,
            address: None,
            body: None,
            error: None,
            enabled: Some(true),
            display_name: None,
            device_name: None,
            contacts: vec![],
            offset: None,
            next_offset: None,
            query: None,
        };
        manager.receive_messages(status, fingerprint).unwrap();
        assert_eq!(
            manager.take_messages().unwrap().unwrap().enabled,
            Some(true)
        );
    }

    #[test]
    fn clipboard_queue_is_fifo_and_preserves_multiple_sources() {
        let (manager, store) = manager_with_store("local");
        manager.set_clipboard_enabled(true);
        let first = crate::DeviceId::generate();
        let second = crate::DeviceId::generate();
        for (id, name) in [(first, "first"), (second, "second")] {
            store
                .insert(PairedDevice {
                    id,
                    name: name.into(),
                    os: "android".into(),
                    cert_fingerprint: "pinned".into(),
                    address: None,
                })
                .unwrap();
        }
        for (index, from) in [first, second, first].into_iter().enumerate() {
            manager
                .receive_clipboard(
                    ClipboardPacket {
                        op: "clip".into(),
                        id: crate::DeviceId::generate().to_string(),
                        from,
                        mime: "text/plain".into(),
                        data_b64: base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            format!("clip-{index}"),
                        ),
                    },
                    "pinned",
                )
                .unwrap();
        }
        let queued = (0..3)
            .map(|_| manager.take_clipboard().unwrap().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(queued[0].0, first.to_string());
        assert_eq!(queued[1].0, second.to_string());
        assert_eq!(queued[2].0, first.to_string());
        assert_eq!(queued[0].2, "Y2xpcC0w");
        assert_eq!(queued[1].2, "Y2xpcC0x");
        assert_eq!(queued[2].2, "Y2xpcC0y");
    }

    async fn serve(
        manager: Arc<PairManager>,
    ) -> (
        String,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<Result<(), CoreError>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let config = crate::tls::pairing_server_config(&manager.identity).unwrap();
        let task = tokio::spawn(crate::tls::tls_serve_with_pairing_until(
            manager.local.id,
            listener,
            config,
            Some(manager),
            async move {
                let _ = rx.await;
            },
        ));
        (address, tx, task)
    }
    async fn finished(manager: &PairManager, session: &str) -> PairSession {
        for _ in 0..100 {
            let view = manager
                .sessions()
                .unwrap()
                .into_iter()
                .find(|v| v.session == session)
                .unwrap();
            if !view.status.pending() {
                return view;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("pairing did not finish");
    }

    #[tokio::test]
    async fn notification_is_forwarded_only_to_a_pinned_pairing() {
        let (phone, phone_store) = manager_with_store("phone");
        let (linux, linux_store) = manager_with_store("linux");
        let linux_id = linux.local.id;
        let phone_id = phone.local.id;
        phone_store
            .insert(PairedDevice {
                id: linux_id,
                name: "linux".into(),
                os: "linux".into(),
                cert_fingerprint: linux.identity.fingerprint(),
                address: None,
            })
            .unwrap();
        linux_store
            .insert(PairedDevice {
                id: phone_id,
                name: "phone".into(),
                os: "android".into(),
                cert_fingerprint: phone.identity.fingerprint(),
                address: None,
            })
            .unwrap();
        let (address, stop, server) = serve(linux.clone()).await;
        phone
            .send_notification_at(
                &linux_id,
                address,
                "android-key-1".into(),
                "Messages".into(),
                "New message".into(),
                "Test notification".into(),
            )
            .await
            .unwrap();
        let received = linux.take_notification().unwrap().unwrap();
        assert_eq!(received.from, phone_id);
        assert_eq!(received.title, "New message");
        assert_eq!(received.body, "Test notification");
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn both_codes_match_and_both_confirmations_are_required() {
        let a = manager("desktop");
        let b = manager("phone");
        let (address, stop, server) = serve(b.clone()).await;
        let request = a.begin(address.clone()).await.unwrap();
        let incoming = b
            .sessions()
            .unwrap()
            .into_iter()
            .find(|v| v.session == request.session)
            .unwrap();
        assert_eq!(request.code, incoming.code);
        assert_eq!(request.code.len(), 6);
        assert_eq!(incoming.peer.cert_fingerprint, a.identity.fingerprint());
        assert_eq!(request.peer.cert_fingerprint, b.identity.fingerprint());
        assert!(a.pairs().unwrap().is_empty());
        assert!(b.pairs().unwrap().is_empty());
        assert!(a.confirm(&request.session, "wrong", true).is_err());
        a.confirm(&request.session, &request.code, true).unwrap();
        assert!(a.confirm(&request.session, &request.code, false).is_err());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(a.pairs().unwrap().is_empty());
        assert!(b.pairs().unwrap().is_empty());
        b.confirm(&request.session, &incoming.code, true).unwrap();
        assert_eq!(
            finished(&a, &request.session).await.status,
            PairStatus::Paired
        );
        assert_eq!(
            finished(&b, &request.session).await.status,
            PairStatus::Paired
        );
        assert_eq!(a.pairs().unwrap()[0].id, b.local.id);
        assert_eq!(
            a.pairs().unwrap()[0].address.as_deref(),
            Some(address.as_str())
        );
        assert_eq!(b.pairs().unwrap()[0].id, a.local.id);
        assert!(a.confirm(&request.session, &request.code, true).is_err());
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn remote_rejection_never_saves_trust() {
        let a = manager("a");
        let b = manager("b");
        let (address, stop, server) = serve(b.clone()).await;
        let request = a.begin(address).await.unwrap();
        a.confirm(&request.session, &request.code, true).unwrap();
        b.confirm(&request.session, "", false).unwrap();
        assert_eq!(
            finished(&a, &request.session).await.status,
            PairStatus::Rejected
        );
        assert!(a.pairs().unwrap().is_empty());
        assert!(b.pairs().unwrap().is_empty());
        let _ = stop.send(());
        server.await.unwrap().unwrap();
    }
    #[test]
    fn expired_or_spoofed_requests_cannot_be_confirmed() {
        let a = manager("a");
        let b = manager("b");
        let hello = PairHello {
            op: "pair".into(),
            session: crate::DeviceId::generate().to_string(),
            info: b.local.clone(),
        };
        assert!(a
            .insert(
                &hello,
                "sha256:spoof",
                "123456".into(),
                "127.0.0.1:1".into(),
                true
            )
            .is_err());
        let (view, _) = a
            .insert(
                &hello,
                &b.identity.fingerprint(),
                "123456".into(),
                "127.0.0.1:1".into(),
                true,
            )
            .unwrap();
        a.lock().unwrap().get_mut(&view.session).unwrap().created =
            Instant::now() - Duration::from_secs(PAIR_TIMEOUT_SECS + 1);
        assert!(a.confirm(&view.session, &view.code, true).is_err());
        assert_eq!(a.sessions().unwrap()[0].status, PairStatus::Expired);
        assert!(a.pairs().unwrap().is_empty());
    }
    #[test]
    fn rejects_duplicate_sessions_and_changed_trusted_certificate() {
        let a = manager("a");
        let b = manager("b");
        let hello = PairHello {
            op: "pair".into(),
            session: crate::DeviceId::generate().to_string(),
            info: b.local.clone(),
        };
        a.insert(
            &hello,
            &b.identity.fingerprint(),
            "123456".into(),
            "127.0.0.1:1".into(),
            true,
        )
        .unwrap();
        assert!(a
            .insert(
                &hello,
                &b.identity.fingerprint(),
                "123456".into(),
                "127.0.0.1:1".into(),
                true
            )
            .is_err());
        let c = manager("c");
        c.store
            .insert(PairedDevice {
                id: b.local.id,
                name: "b".into(),
                os: "linux".into(),
                cert_fingerprint: "sha256:old".into(),
                address: None,
            })
            .unwrap();
        assert!(c
            .insert(
                &hello,
                &b.identity.fingerprint(),
                "123456".into(),
                "127.0.0.1:1".into(),
                true
            )
            .is_err());
    }
}
