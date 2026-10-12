//! OS abstraction so `link-core` stays portable and
//! `link-daemon` / Android glue provide platform impls.

use crate::CoreError;

/// Desktop/mobile capabilities needed by the core state machine.
pub trait OsPlatform: Send + Sync {
    fn os_name(&self) -> &'static str;
    fn data_dir(&self) -> Result<std::path::PathBuf, CoreError>;
    fn notify(&self, title: &str, body: &str) -> Result<(), CoreError>;
}

/// Linux implementation skeleton (Phase 1 wires `notify-rust`/`zbus`,
/// XDG dirs, `keyring`, portals).
pub struct LinuxPlatform;

impl OsPlatform for LinuxPlatform {
    fn os_name(&self) -> &'static str {
        "linux"
    }

    fn data_dir(&self) -> Result<std::path::PathBuf, CoreError> {
        let base = std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".local/share")
            });
        Ok(base.join("link-to-linux"))
    }

    fn notify(&self, _title: &str, _body: &str) -> Result<(), CoreError> {
        // Phase 1: notify-rust via D-Bus. Stub ok for Phase 0.
        Ok(())
    }
}

/// Android implementation is provided by `link-android-svc` via JNI in Phase 3.
pub struct AndroidPlatform;

impl OsPlatform for AndroidPlatform {
    fn os_name(&self) -> &'static str {
        "android"
    }

    fn data_dir(&self) -> Result<std::path::PathBuf, CoreError> {
        Err(CoreError::Storage(
            "Android data dir must come from JNI context (Phase 3)".into(),
        ))
    }

    fn notify(&self, _title: &str, _body: &str) -> Result<(), CoreError> {
        Err(CoreError::Storage(
            "Android notify goes through FGS (Phase 3)".into(),
        ))
    }
}

/// Private runtime directory shared by the Linux daemon and desktop client.
pub fn linux_socket_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            LinuxPlatform
                .data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/link-to-linux"))
        });
    base.join("link-to-linux-runtime").join("daemon.sock")
}
