//! Desktop client for the daemon's private socket API. Only ping uses the CLI.
#![cfg_attr(not(feature = "ui"), allow(dead_code))]
use anyhow::{bail, Context, Result};
use link_core::discovery::DiscoveredDevice;
use link_core::ipc_wire::Request;
use link_core::pair_session::PairSession;
use link_core::pairing::PairedDevice;
use link_core::DeviceInfo;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct PingResult {
    pub pong_from: String,
    pub nonce_ok: bool,
    pub elapsed_ms: u128,
}
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub identity: DeviceInfo,
    pub devices: Vec<DiscoveredDevice>,
    pub sessions: Vec<PairSession>,
    pub pairings: Vec<PairedDevice>,
}
#[derive(Debug, Clone)]
pub struct DaemonClient {
    bin: PathBuf,
    socket: PathBuf,
}
impl DaemonClient {
    pub fn discover() -> Result<Self> {
        if let Some(path) = std::env::var_os("LINK_DAEMON_BIN") {
            let bin = PathBuf::from(path);
            if !bin.exists() {
                bail!("LINK_DAEMON_BIN={} does not exist", bin.display());
            }
            return Ok(Self {
                bin,
                socket: link_core::platform::linux_socket_path(),
            });
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let bin = parent.join("link-daemon");
                if bin.exists() {
                    return Ok(Self {
                        bin,
                        socket: link_core::platform::linux_socket_path(),
                    });
                }
            }
        }
        Ok(Self {
            bin: "link-daemon".into(),
            socket: link_core::platform::linux_socket_path(),
        })
    }
    async fn run(&self, args: &[&str]) -> Result<serde_json::Value> {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio::process::Command::new(&self.bin)
                .args(args)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("daemon command timed out")?
        .with_context(|| format!("could not run {}", self.bin.display()))?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        serde_json::from_slice(&output.stdout).context("invalid JSON from daemon")
    }
    async fn rpc(&self, request: &Request) -> Result<serde_json::Value> {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let mut stream = tokio::net::UnixStream::connect(&self.socket)
                .await
                .context("cannot reach link service")?;
            link_core::transport::write_frame(&mut stream, &serde_json::to_vec(request)?).await?;
            let bytes = link_core::transport::read_frame(&mut stream).await?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            if value["ok"] != true {
                bail!(
                    "{}",
                    value["error"].as_str().unwrap_or("daemon request failed")
                );
            }
            Ok(value)
        })
        .await
        .context("daemon request timed out")?
    }
    pub async fn ensure_running(&self) -> Result<()> {
        if self.rpc(&Request::Status).await.is_ok() {
            return Ok(());
        }
        let mut child = tokio::process::Command::new(&self.bin)
            .args(["serve", "--name", "Linux desktop"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("could not start daemon")?;
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if self.rpc(&Request::Status).await.is_ok() {
                return Ok(());
            }
            if let Some(status) = child.try_wait()? {
                bail!("daemon exited ({status}); check for an old daemon on port 53536 and restart it");
            }
        }
        let _ = child.kill().await;
        bail!("daemon did not become ready")
    }
    pub async fn pairs(&self) -> Result<Vec<PairedDevice>> {
        serde_json::from_value(self.rpc(&Request::Pairs).await?["pairings"].clone())
            .context("invalid pairings")
    }
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let (status, devices, sessions, pairings) = tokio::try_join!(
            self.rpc(&Request::Status),
            self.rpc(&Request::Devices),
            self.rpc(&Request::Sessions),
            self.pairs()
        )?;
        Ok(Snapshot {
            identity: serde_json::from_value(status["identity"].clone())?,
            devices: serde_json::from_value(devices["devices"].clone())?,
            sessions: serde_json::from_value(sessions["sessions"].clone())?,
            pairings,
        })
    }
    pub async fn begin_pair(&self, peer: &str) -> Result<PairSession> {
        serde_json::from_value(
            self.rpc(&Request::Begin { peer: peer.into() }).await?["pairing"].clone(),
        )
        .context("invalid pairing request")
    }
    pub async fn confirm_pair(&self, session: &str, code: &str, accept: bool) -> Result<()> {
        self.rpc(&Request::Confirm {
            session: session.into(),
            code: code.into(),
            accept,
        })
        .await?;
        Ok(())
    }
    pub async fn ping(&self, peer: &str, fingerprint: &str) -> Result<PingResult> {
        let start = Instant::now();
        let result = self
            .run(&["ping", "--peer", peer, "--fingerprint", fingerprint])
            .await?;
        Ok(PingResult {
            pong_from: result["pong_from"]
                .as_str()
                .context("missing peer ID")?
                .into(),
            nonce_ok: result["nonce"].is_u64() && result["nonce"] == result["pong_nonce"],
            elapsed_ms: start.elapsed().as_millis(),
        })
    }
    pub async fn unpair(&self, id: &str) -> Result<()> {
        self.rpc(&Request::Unpair { id: id.into() }).await?;
        Ok(())
    }
    pub async fn clipboard_enabled(&self, enabled: bool) -> Result<()> {
        self.rpc(&Request::ClipboardEnabled { enabled }).await?;
        Ok(())
    }
    pub async fn clipboard_take(&self) -> Result<serde_json::Value> {
        self.rpc(&Request::ClipboardTake).await
    }
    pub async fn clipboard_status(&self) -> Result<serde_json::Value> {
        self.rpc(&Request::ClipboardStatus).await
    }
    pub async fn clipboard_send(&self, id: &str, mime: String, data_b64: String) -> Result<()> {
        self.rpc(&Request::ClipboardSend {
            id: id.into(),
            mime,
            data_b64,
        })
        .await?;
        Ok(())
    }
    pub async fn gallery_list(&self, id: &str, request_id: String, offset: u32) -> Result<()> {
        self.rpc(&Request::GalleryList {
            id: id.into(),
            request_id,
            offset,
        })
        .await?;
        Ok(())
    }
    pub async fn gallery_fetch(
        &self,
        id: &str,
        request_id: String,
        photo_id: String,
    ) -> Result<()> {
        self.rpc(&Request::GalleryFetch {
            id: id.into(),
            request_id,
            photo_id,
        })
        .await?;
        Ok(())
    }
    pub async fn gallery_take(&self) -> Result<serde_json::Value> {
        self.rpc(&Request::GalleryTake).await
    }
    pub async fn messages_list(&self, id: &str) -> Result<()> {
        self.rpc(&Request::MessagesList {
            id: id.into(),
            offset: 0,
        })
        .await?;
        Ok(())
    }
    pub async fn messages_status(&self, id: &str) -> Result<()> {
        self.rpc(&Request::MessagesStatus { id: id.into() }).await?;
        Ok(())
    }
    pub async fn messages_thread(&self, id: &str, thread_id: String) -> Result<()> {
        self.rpc(&Request::MessagesThread {
            id: id.into(),
            thread_id,
            offset: 0,
        })
        .await?;
        Ok(())
    }
    pub async fn messages_send(&self, id: &str, address: String, body: String) -> Result<String> {
        let response = self
            .rpc(&Request::MessagesSend {
                id: id.into(),
                address,
                body,
            })
            .await?;
        response["request_id"]
            .as_str()
            .map(str::to_owned)
            .context("missing message request ID")
    }
    pub async fn messages_contacts(&self, id: &str, query: String, offset: u32) -> Result<()> {
        self.rpc(&Request::MessagesContacts {
            id: id.into(),
            query,
            offset,
        })
        .await?;
        Ok(())
    }
    pub async fn messages_page(
        &self,
        id: &str,
        thread_id: Option<String>,
        offset: u32,
    ) -> Result<()> {
        let request = match thread_id {
            Some(thread_id) => Request::MessagesThread {
                id: id.into(),
                thread_id,
                offset,
            },
            None => Request::MessagesList {
                id: id.into(),
                offset,
            },
        };
        self.rpc(&request).await?;
        Ok(())
    }
    pub async fn messages_take(&self) -> Result<serde_json::Value> {
        self.rpc(&Request::MessagesTake).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pairing_code_is_preserved_and_confirmation_errors_are_reported() {
        let dir =
            std::env::temp_dir().join(format!("link-ipc-test-{}", link_core::DeviceId::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("daemon.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let view = PairSession {
            session: link_core::DeviceId::generate().to_string(),
            code: "000042".into(),
            peer: PairedDevice {
                id: link_core::DeviceId::generate(),
                name: "phone".into(),
                os: "android".into(),
                cert_fingerprint: "sha256:test".into(),
                address: Some("192.168.1.2:53536".into()),
            },
            incoming: false,
            status: link_core::pair_session::PairStatus::Awaiting,
            expires_in_secs: 120,
            error: None,
        };
        let expected = view.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let bytes = link_core::transport::read_frame(&mut stream).await.unwrap();
            let request: Request = serde_json::from_slice(&bytes).unwrap();
            assert!(matches!(request,Request::Begin {peer} if peer=="192.168.1.2:53536"));
            let response =
                serde_json::to_vec(&serde_json::json!({"ok":true,"pairing":expected})).unwrap();
            link_core::transport::write_frame(&mut stream, &response)
                .await
                .unwrap();
            let (mut stream, _) = listener.accept().await.unwrap();
            let bytes = link_core::transport::read_frame(&mut stream).await.unwrap();
            let request: Request = serde_json::from_slice(&bytes).unwrap();
            assert!(
                matches!(request,Request::Confirm {session,code,accept} if session==expected.session && code=="000042" && accept)
            );
            link_core::transport::write_frame(
                &mut stream,
                br#"{"ok":false,"error":"code expired"}"#,
            )
            .await
            .unwrap();
        });
        let client = DaemonClient {
            bin: "/nonexistent-daemon".into(),
            socket,
        };
        let response = client.begin_pair("192.168.1.2:53536").await.unwrap();
        assert_eq!(response.code, "000042");
        let error = client
            .confirm_pair(&view.session, &response.code, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("code expired"));
        server.await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn errors_on_missing_daemon() {
        let client = DaemonClient {
            bin: PathBuf::from("/nonexistent-link-daemon-xyz"),
            socket: PathBuf::from("/nonexistent-link-daemon.sock"),
        };
        assert!(client.pairs().await.is_err());
    }
}
