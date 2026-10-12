use crate::{CoreError, DeviceInfo};

/// Identity packet broadcast during discovery (CBOR-encoded [`DeviceInfo`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityPacket(pub DeviceInfo);

impl IdentityPacket {
    pub fn new(info: DeviceInfo) -> Result<Self, CoreError> {
        info.validate()?;
        Ok(Self(info))
    }

    pub fn encode(&self) -> Result<Vec<u8>, CoreError> {
        crate::codec::encode_identity(&self.0)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        let info: DeviceInfo = crate::codec::decode(bytes)?;
        Self::new(info)
    }
}
