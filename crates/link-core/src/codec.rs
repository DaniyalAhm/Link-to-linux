use crate::{CoreError, DeviceInfo};

/// CBOR codec (Phase 0). JSON is used only for local IPC / debugging.
pub fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, CoreError> {
    let mut buf = Vec::new();
    ciborium::ser::into_writer(value, &mut buf).map_err(|e| CoreError::Codec(e.to_string()))?;
    Ok(buf)
}

pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, CoreError> {
    ciborium::de::from_reader(bytes).map_err(|e| CoreError::Codec(e.to_string()))
}

/// Envelope for MVP plugin messages.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    pub from: crate::DeviceId,
    pub to: Option<crate::DeviceId>,
    pub kind: MessageKind,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Pair,
    Notify,
    File,
    Clip,
}

pub fn encode_identity(info: &DeviceInfo) -> Result<Vec<u8>, CoreError> {
    info.validate()?;
    encode(info)
}

/// Phase 1a handshake messages (plaintext TCP; replaced by QUIC+TLS in Phase 1b).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ping {
    pub nonce: u64,
    pub from: crate::DeviceId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pong {
    pub nonce: u64,
    pub from: crate::DeviceId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Capability, DeviceId};

    #[test]
    fn cbor_roundtrip_identity() {
        let info = DeviceInfo {
            id: DeviceId::generate(),
            name: "phone".into(),
            os: "android".into(),
            protocol_version: crate::PROTOCOL_VERSION,
            quic_port: crate::QUIC_PORT,
            pubkey_fingerprint: "sha256:test".into(),
            capabilities: vec![Capability::Pair, Capability::File],
        };
        let bytes = encode_identity(&info).unwrap();
        let back: DeviceInfo = decode(&bytes).unwrap();
        assert_eq!(info, back);
    }

    #[test]
    fn cbor_roundtrip_ping_pong() {
        let ping = Ping {
            nonce: 42,
            from: DeviceId::generate(),
        };
        let bytes = encode(&ping).unwrap();
        let back: Ping = decode(&bytes).unwrap();
        assert_eq!(ping, back);
    }
}
