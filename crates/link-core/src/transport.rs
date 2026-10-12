use crate::codec::{self, Ping, Pong};
use crate::{CoreError, DeviceId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Length-prefixed framing: u32 big-endian length + CBOR bytes.
/// Shared by daemon + Android so both sides interoperate.
pub async fn write_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    bytes: &[u8],
) -> Result<(), CoreError> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(CoreError::InvalidData("frame too large".into()));
    }
    w.write_u32(bytes.len() as u32)
        .await
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    w.write_all(bytes)
        .await
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    Ok(())
}

pub async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<Vec<u8>, CoreError> {
    let len = r
        .read_u32()
        .await
        .map_err(|e| CoreError::Transport(e.to_string()))? as usize;
    if len > 8 * 1024 * 1024 {
        return Err(CoreError::InvalidData("frame too large".into()));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .await
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    Ok(buf)
}

/// QUIC/TLS transport skeleton (Phase 1b). Phase 1a uses plaintext TCP below.
pub trait Transport: Send + Sync {
    fn connect(
        &self,
        addr: &str,
    ) -> impl std::future::Future<Output = Result<(), CoreError>> + Send;
    fn listen(&self, port: u16) -> impl std::future::Future<Output = Result<(), CoreError>> + Send;
}

pub struct QuinnTransport {
    pub port: u16,
}

impl QuinnTransport {
    pub fn new(port: u16) -> Self {
        Self { port }
    }
}

impl Transport for QuinnTransport {
    async fn connect(&self, _addr: &str) -> Result<(), CoreError> {
        Err(CoreError::Transport(
            "QUIC lands in Phase 1b; use tcp_ping for Phase 1a".into(),
        ))
    }

    async fn listen(&self, _port: u16) -> Result<(), CoreError> {
        Err(CoreError::Transport(
            "QUIC lands in Phase 1b; use serve_tcp for Phase 1a".into(),
        ))
    }
}

/// Phase 1a: TCP ping client. Connects, sends `Ping`, awaits `Pong` (5s timeout).
/// `peer` is `ip:port`, e.g. `127.0.0.1:53536` or `10.0.2.2:53536` from emulator.
pub async fn tcp_ping(peer: &str, ping: &Ping) -> Result<Pong, CoreError> {
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::TcpStream::connect(peer),
    )
    .await
    .map_err(|_| CoreError::Transport(format!("connect timeout: {peer}")))
    .and_then(|r| r.map_err(|e| CoreError::Transport(e.to_string())))?;
    let bytes = codec::encode(ping)?;
    write_frame(&mut stream, &bytes).await?;
    let resp = tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut stream))
        .await
        .map_err(|_| CoreError::Transport("pong timeout".into()))??;
    let pong: Pong = codec::decode(&resp)?;
    if pong.nonce != ping.nonce {
        return Err(CoreError::InvalidData(format!(
            "nonce mismatch: sent {} got {}",
            ping.nonce, pong.nonce
        )));
    }
    Ok(pong)
}

/// Phase 1a: TCP ping server. Binds `0.0.0.0:port`, replies `Pong` with same nonce.
/// Runs until Ctrl-C.
pub async fn serve_tcp(port: u16, device_id: DeviceId) -> Result<(), CoreError> {
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    serve_tcp_until(port, device_id, listener, shutdown_on_signal()).await
}

async fn shutdown_on_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Test-friendly variant: caller provides the bound listener + shutdown future.
pub async fn serve_tcp_until(
    _port: u16,
    device_id: DeviceId,
    listener: tokio::net::TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), CoreError> {
    use tokio::sync::oneshot;
    let (_done_tx, mut done_rx) = oneshot::channel::<()>();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                return Ok(());
            }
            _ = &mut done_rx => {
                return Ok(());
            }
            accepted = listener.accept() => {
                let (mut stream, _addr) = accepted.map_err(|e| CoreError::Transport(e.to_string()))?;
                tokio::spawn(async move {
                    let frame = match read_frame(&mut stream).await {
                        Ok(f) => f,
                        Err(_) => return,
                    };
                    let ping: Ping = match codec::decode(&frame) {
                        Ok(p) => p,
                        Err(_) => return,
                    };
                    let pong = Pong { nonce: ping.nonce, from: device_id };
                    if let Ok(bytes) = codec::encode(&pong) {
                        let _ = write_frame(&mut stream, &bytes).await;
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tcp_ping_pong_loopback() {
        let device_id = DeviceId::generate();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            serve_tcp_until(port, device_id, listener, async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
        });
        let ping = Ping {
            nonce: 1234,
            from: DeviceId::generate(),
        };
        let pong = tcp_ping(&format!("127.0.0.1:{port}"), &ping).await.unwrap();
        assert_eq!(pong.nonce, 1234);
        assert_eq!(pong.from, device_id);
        let _ = tx.send(());
        server.await.unwrap();
    }
}
