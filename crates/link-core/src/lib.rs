//! `link-core`: OS-agnostic protocol, discovery, transport, pairing.
//!
//! Phase 0: types + codec + trait skeletons. No real I/O yet.
//! Single source of truth for the wire format lives in `proto/link.cddl`.

pub mod codec;
pub mod device;
pub mod discovery;
pub mod error;
pub mod identity;
pub mod ipc_wire;
pub mod pair_session;
pub mod pairing;
pub mod platform;
pub mod presence;
pub mod tls;
pub mod transport;

pub use device::{Capability, DeviceId, DeviceInfo};
pub use error::CoreError;
pub use identity::IdentityPacket;

pub const PROTOCOL_VERSION: u16 = 1;
/// mDNS service type for discovery (Phase 0).
pub const MDNS_SERVICE_TYPE: &str = "_linktolinux._udp.local.";
/// UDP port for broadcast fallback discovery.
pub const DISCOVERY_PORT: u16 = 53535;
/// UDP port for QUIC transport.
pub const QUIC_PORT: u16 = 53536;
