use crate::CoreError;

/// MVP capability set. Extensible without breaking pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Pair,
    Notify,
    File,
    Clip,
}

/// Stable device identifier (UUIDv7, generated once per install).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DeviceId(pub uuid::Uuid);

impl DeviceId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for DeviceId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse()?))
    }
}

/// Static device descriptor exchanged during discovery.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    pub os: String,
    pub protocol_version: u16,
    pub quic_port: u16,
    pub pubkey_fingerprint: String,
    pub capabilities: Vec<Capability>,
}

impl DeviceInfo {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.name.is_empty() {
            return Err(CoreError::InvalidData("device name is empty".into()));
        }
        if self.protocol_version != crate::PROTOCOL_VERSION {
            return Err(CoreError::VersionMismatch {
                expected: crate::PROTOCOL_VERSION,
                got: self.protocol_version,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_validates() {
        let info = DeviceInfo {
            id: DeviceId::generate(),
            name: "test-laptop".into(),
            os: "linux".into(),
            protocol_version: crate::PROTOCOL_VERSION,
            quic_port: crate::QUIC_PORT,
            pubkey_fingerprint: "sha256:abc".into(),
            capabilities: vec![Capability::Pair, Capability::Notify],
        };
        assert!(info.validate().is_ok());
    }
}
