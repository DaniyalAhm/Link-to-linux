//! Continuous presence tracking (transport-agnostic).
//!
//! Both discovery transports (UDP broadcast + mDNS) feed sightings into a
//! shared [`PresenceTracker`]; it expires peers not seen for [`EXPIRY_SECS`]
//! and renders [`DiscoveredDevice`] snapshots with real `last_seen_secs`.

use crate::discovery::DiscoveredDevice;
use crate::{CoreError, DeviceId, DeviceInfo};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Re-announce interval (mirrors the 5s cadence of KDE Connect-style discovery).
pub const ANNOUNCE_INTERVAL_SECS: u64 = 5;
/// Drop peers silent this long. mDNS `ServiceRemoved` drops immediately.
pub const EXPIRY_SECS: u64 = 30;

#[derive(Debug)]
struct Entry {
    info: DeviceInfo,
    last_seen: Instant,
    addresses: Vec<String>,
}

/// Thread-safe sighting store shared by listener tasks and query paths.
#[derive(Debug)]
pub struct PresenceTracker {
    inner: Mutex<HashMap<String, Entry>>,
    own_id: Option<DeviceId>,
    expiry: Duration,
}

impl PresenceTracker {
    pub fn new(own_id: Option<DeviceId>) -> Self {
        Self::with_expiry(own_id, Duration::from_secs(EXPIRY_SECS))
    }

    /// Test-friendly constructor (e.g. 200ms expiry for fast tests).
    pub fn with_expiry(own_id: Option<DeviceId>, expiry: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            own_id,
            expiry,
        }
    }

    /// Record a sighting. Returns `true` if membership changed (new peer or
    /// changed identity payload), `false` for a plain refresh. Invalid peers
    /// (bad version, empty name) and our own id are ignored (`false`).
    pub fn update(&self, info: DeviceInfo) -> Result<bool, CoreError> {
        self.update_at(info, Vec::new())
    }

    pub fn update_at(
        &self,
        info: DeviceInfo,
        mut addresses: Vec<String>,
    ) -> Result<bool, CoreError> {
        if Some(info.id) == self.own_id {
            return Ok(false);
        }
        info.validate()?;
        let mut map = self
            .inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let key = info.id.to_string();
        if addresses.is_empty() {
            if let Some(previous) = map.get(&key) {
                addresses = previous.addresses.clone();
            }
        }
        addresses.sort();
        addresses.dedup();
        let changed = match map.get(&key) {
            Some(e) => e.info != info || e.addresses != addresses,
            None => true,
        };
        map.insert(
            key,
            Entry {
                info,
                last_seen: Instant::now(),
                addresses,
            },
        );
        Ok(changed)
    }

    /// Drop a peer immediately (mDNS `ServiceRemoved`). Returns `true` if present.
    pub fn remove(&self, id: &DeviceId) -> Result<bool, CoreError> {
        let mut map = self
            .inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(map.remove(&id.to_string()).is_some())
    }

    /// Drop entries silent longer than the expiry. Returns lost ids.
    pub fn prune(&self) -> Result<Vec<DeviceId>, CoreError> {
        let mut map = self
            .inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let mut lost = Vec::new();
        map.retain(|_, e| {
            if e.last_seen.elapsed() > self.expiry {
                lost.push(e.info.id);
                false
            } else {
                true
            }
        });
        Ok(lost)
    }

    /// Current peers, oldest-seen first, with real `last_seen_secs`.
    pub fn snapshot(&self) -> Result<Vec<DiscoveredDevice>, CoreError> {
        let map = self
            .inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let mut out: Vec<DiscoveredDevice> = map
            .values()
            .map(|e| DiscoveredDevice {
                info: e.info.clone(),
                addresses: e.addresses.clone(),
                last_seen_secs: e.last_seen.elapsed().as_secs(),
            })
            .collect();
        out.sort_by_key(|d| std::cmp::Reverse(d.last_seen_secs));
        Ok(out)
    }

    pub fn len(&self) -> Result<usize, CoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .len())
    }

    pub fn is_empty(&self) -> Result<bool, CoreError> {
        Ok(self.len()? == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capability, PROTOCOL_VERSION, QUIC_PORT};

    fn info(name: &str) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::generate(),
            name: name.into(),
            os: "linux".into(),
            protocol_version: PROTOCOL_VERSION,
            quic_port: QUIC_PORT,
            pubkey_fingerprint: "sha256:test".into(),
            capabilities: vec![Capability::Pair],
        }
    }

    #[test]
    fn discovery_keeps_addresses_and_refreshes_changed_endpoints() {
        let tracker = PresenceTracker::new(None);
        let peer = info("phone");
        tracker
            .update_at(peer.clone(), vec!["192.168.1.5:53536".into()])
            .unwrap();
        tracker.update(peer.clone()).unwrap();
        assert_eq!(
            tracker.snapshot().unwrap()[0].addresses,
            vec!["192.168.1.5:53536"]
        );
        assert!(tracker
            .update_at(peer, vec!["192.168.1.6:53536".into()])
            .unwrap());
        assert_eq!(
            tracker.snapshot().unwrap()[0].addresses,
            vec!["192.168.1.6:53536"]
        );
    }

    #[test]
    fn update_refresh_prune() {
        let t = PresenceTracker::with_expiry(None, Duration::from_millis(120));
        let a = info("a");
        assert!(t.update(a.clone()).unwrap()); // new peer
        assert!(!t.update(a.clone()).unwrap()); // plain refresh
        assert_eq!(t.len().unwrap(), 1);
        std::thread::sleep(Duration::from_millis(160));
        let lost = t.prune().unwrap();
        assert_eq!(lost, vec![a.id]);
        assert_eq!(t.len().unwrap(), 0);
    }

    #[test]
    fn payload_change_reports_membership_change() {
        let t = PresenceTracker::new(None);
        let mut a = info("a");
        assert!(t.update(a.clone()).unwrap());
        a.name = "a-renamed".into();
        assert!(t.update(a).unwrap());
    }

    #[test]
    fn rejects_own_id_and_bad_peers() {
        let me = DeviceId::generate();
        let t = PresenceTracker::new(Some(me));
        let mut mine = info("me");
        mine.id = me;
        assert!(!t.update(mine).unwrap());
        let mut bad = info("bad");
        bad.protocol_version = 999;
        assert!(bad.validate().is_err());
        // update() surfaces the validation error, stores nothing.
        assert!(t.update(bad).is_err());
        assert_eq!(t.len().unwrap(), 0);
    }

    #[test]
    fn dedups_across_transports() {
        // Same id sighted via mDNS then UDP refreshes one entry.
        let t = PresenceTracker::new(None);
        let a = info("a");
        assert!(t.update(a.clone()).unwrap());
        assert!(!t.update(a.clone()).unwrap());
        assert_eq!(t.len().unwrap(), 1);
        let snap = t.snapshot().unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].info.id, a.id);
    }

    #[test]
    fn remove_drops_immediately() {
        let t = PresenceTracker::new(None);
        let a = info("a");
        t.update(a.clone()).unwrap();
        assert!(t.remove(&a.id).unwrap());
        assert!(!t.remove(&a.id).unwrap());
        assert_eq!(t.len().unwrap(), 0);
    }
}
