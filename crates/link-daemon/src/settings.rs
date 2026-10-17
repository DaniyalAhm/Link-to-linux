//! Preferences belong to the daemon so they survive both UI and service restarts.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Preferences {
    clipboard_sync: bool,
}

pub struct Settings {
    path: PathBuf,
    preferences: Mutex<Preferences>,
}

impl Settings {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("settings.json");
        let preferences = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Preferences::default(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            preferences: Mutex::new(preferences),
        })
    }

    pub fn restore(&self, manager: &link_core::pair_session::PairManager) -> anyhow::Result<()> {
        let preferences = self
            .preferences
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        manager.set_clipboard_enabled(preferences.clipboard_sync);
        Ok(())
    }

    pub fn set_clipboard(
        &self,
        manager: &link_core::pair_session::PairManager,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let mut preferences = self
            .preferences
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let next = Preferences {
            clipboard_sync: enabled,
        };
        std::fs::create_dir_all(self.path.parent().unwrap())?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&next)?)?;
        std::fs::rename(&temporary, &self.path)?;
        *preferences = next;
        manager.set_clipboard_enabled(enabled);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_choice_survives_service_recreation() {
        let dir = std::env::temp_dir().join(format!("link-settings-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let manager = || {
            let id = link_core::DeviceId::generate();
            let local = link_core::DeviceInfo {
                id,
                name: "test".into(),
                os: "linux".into(),
                protocol_version: link_core::PROTOCOL_VERSION,
                quic_port: link_core::QUIC_PORT,
                pubkey_fingerprint: "test".into(),
                capabilities: vec![],
            };
            link_core::pair_session::PairManager::new(
                local,
                link_core::tls::load_or_create_identity(&dir).unwrap(),
                std::sync::Arc::new(link_core::pairing::JsonFileStore::new(
                    dir.join("paired.json"),
                )),
            )
        };
        let first = manager();
        let settings = Settings::load(&dir).unwrap();
        settings.restore(&first).unwrap();
        assert!(!first.clipboard_enabled());
        settings.set_clipboard(&first, true).unwrap();
        drop(settings);
        drop(first);
        let restarted = manager();
        let settings = Settings::load(&dir).unwrap();
        settings.restore(&restarted).unwrap();
        assert!(restarted.clipboard_enabled());
        settings.set_clipboard(&restarted, false).unwrap();
        let restarted_again = manager();
        Settings::load(&dir)
            .unwrap()
            .restore(&restarted_again)
            .unwrap();
        assert!(!restarted_again.clipboard_enabled());
        // A failed save must not apply an unsaved runtime preference.
        std::fs::create_dir(dir.join("settings.json.tmp")).unwrap();
        assert!(settings.set_clipboard(&restarted, true).is_err());
        assert!(!restarted.clipboard_enabled());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
