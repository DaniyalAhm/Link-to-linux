use crate::{CoreError, DeviceId};
use std::collections::HashMap;
use std::sync::Mutex;

/// Persisted trust record for a paired peer (cert pinning in Phase 1).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PairedDevice {
    pub id: DeviceId,
    pub name: String,
    #[serde(default)]
    pub os: String,
    pub cert_fingerprint: String,
    #[serde(default)]
    pub address: Option<String>,
}

pub trait PairingStore: Send + Sync {
    fn insert(&self, device: PairedDevice) -> Result<(), CoreError>;
    fn remove(&self, id: &DeviceId) -> Result<(), CoreError>;
    fn get(&self, id: &DeviceId) -> Result<Option<PairedDevice>, CoreError>;
    fn list(&self) -> Result<Vec<PairedDevice>, CoreError>;
}

/// In-memory store for tests + Phase 0. SQLite impl lands in Phase 1.
#[derive(Debug, Default)]
pub struct MemoryPairingStore {
    inner: Mutex<HashMap<String, PairedDevice>>,
}

impl MemoryPairingStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PairingStore for MemoryPairingStore {
    fn insert(&self, device: PairedDevice) -> Result<(), CoreError> {
        self.inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .insert(device.id.to_string(), device);
        Ok(())
    }

    fn remove(&self, id: &DeviceId) -> Result<(), CoreError> {
        self.inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .remove(&id.to_string());
        Ok(())
    }

    fn get(&self, id: &DeviceId) -> Result<Option<PairedDevice>, CoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .get(&id.to_string())
            .cloned())
    }

    fn list(&self) -> Result<Vec<PairedDevice>, CoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .values()
            .cloned()
            .collect())
    }
}

/// JSON-file store for daemon + Android journal dir. Atomic-ish: write tmp + rename.
#[derive(Debug)]
pub struct JsonFileStore {
    path: std::path::PathBuf,
}

impl JsonFileStore {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    fn load(&self) -> Result<Vec<PairedDevice>, CoreError> {
        match std::fs::read(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(CoreError::Storage(e.to_string())),
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| CoreError::Storage(format!("paired.json corrupt: {e}"))),
        }
    }
    fn save(&self, all: &[PairedDevice]) -> Result<(), CoreError> {
        let bytes =
            serde_json::to_vec_pretty(all).map_err(|e| CoreError::Storage(e.to_string()))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::Storage(e.to_string()))?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| CoreError::Storage(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(())
    }
}

impl PairingStore for JsonFileStore {
    fn insert(&self, device: PairedDevice) -> Result<(), CoreError> {
        let mut all = self.load()?;
        all.retain(|d| d.id != device.id);
        all.push(device);
        self.save(&all)
    }
    fn remove(&self, id: &DeviceId) -> Result<(), CoreError> {
        let mut all = self.load()?;
        all.retain(|d| &d.id != id);
        self.save(&all)
    }

    fn get(&self, id: &DeviceId) -> Result<Option<PairedDevice>, CoreError> {
        Ok(self.load()?.into_iter().find(|d| &d.id == id))
    }

    fn list(&self) -> Result<Vec<PairedDevice>, CoreError> {
        self.load()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceId;

    #[test]
    fn file_store_survives_reopen_and_atomic_unpair() {
        let dir = std::env::temp_dir().join(format!("link-store-{}", DeviceId::generate()));
        let path = dir.join("paired.json");
        let record = PairedDevice {
            id: DeviceId::generate(),
            name: "phone".into(),
            os: "android".into(),
            cert_fingerprint: "sha256:test".into(),
            address: Some("192.168.1.2:53536".into()),
        };
        JsonFileStore::new(path.clone())
            .insert(record.clone())
            .unwrap();
        let reopened = JsonFileStore::new(path);
        assert_eq!(reopened.get(&record.id).unwrap(), Some(record.clone()));
        reopened.remove(&record.id).unwrap();
        assert!(reopened.list().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn memory_store_roundtrip() {
        let store = MemoryPairingStore::new();
        let id = DeviceId::generate();
        store
            .insert(PairedDevice {
                id,
                name: "phone".into(),
                os: "android".into(),
                cert_fingerprint: "sha256:x".into(),
                address: None,
            })
            .unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(store.get(&id).unwrap().is_some());
        store.remove(&id).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
