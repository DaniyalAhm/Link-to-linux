//! Typed local daemon requests; JSON inside length-prefixed IPC frames.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    Status,
    Devices,
    Begin {
        peer: String,
    },
    Sessions,
    Confirm {
        session: String,
        code: String,
        accept: bool,
    },
    Pairs,
    Unpair {
        id: String,
    },
    ClipboardEnabled {
        enabled: bool,
    },
    ClipboardStatus,
    ClipboardTake,
    ClipboardSend {
        id: String,
        mime: String,
        data_b64: String,
    },
    GalleryList {
        id: String,
        request_id: String,
        offset: u32,
    },
    GalleryFetch {
        id: String,
        request_id: String,
        photo_id: String,
    },
    GalleryTake,
    MessagesList {
        id: String,
        #[serde(default)]
        offset: u32,
    },
    MessagesContacts {
        id: String,
        query: String,
        offset: u32,
    },
    MessagesStatus {
        id: String,
    },
    MessagesThread {
        id: String,
        #[serde(default)]
        offset: u32,
        thread_id: String,
    },
    MessagesSend {
        id: String,
        address: String,
        body: String,
    },
    MessagesTake,
}
