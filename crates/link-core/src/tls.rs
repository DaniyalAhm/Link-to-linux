//! TLS transport (Phase 1b): self-signed certs + SHA-256 fingerprint pinning.
//!
//! No CA, no hostname checks — trust comes from pairing (TOFU + stored
//! fingerprint in [`crate::pairing::PairingStore`]). Same length-prefixed
//! CBOR framing as plaintext, wrapped in rustls.

use crate::codec::{self, Ping, Pong};
use crate::transport::{read_frame, write_frame};
use crate::{CoreError, DeviceId};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;

/// Device TLS identity (persisted in data dir as `cert.der` / `key.der`).
#[derive(Debug, Clone)]
pub struct Identity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl Identity {
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.cert_der)
    }
}

/// `sha256:<hex>` over the DER cert.
pub fn fingerprint(cert_der: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(cert_der);
    format!("sha256:{}", hex::encode(h.finalize()))
}

fn ensure_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn load_or_create_identity(dir: &std::path::Path) -> Result<Identity, CoreError> {
    let cert_path = dir.join("cert.der");
    let key_path = dir.join("key.der");
    if let (Ok(cert_der), Ok(key_der)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| CoreError::Storage(e.to_string()))?;
        }
        return Ok(Identity { cert_der, key_der });
    }
    if cert_path.exists() || key_path.exists() {
        return Err(CoreError::Storage(
            "incomplete TLS identity; restore the matching cert.der/key.der pair".into(),
        ));
    }
    ensure_provider();
    let key = rcgen::generate_simple_self_signed(vec!["link-to-linux".to_string()])
        .map_err(|e| CoreError::Transport(format!("cert gen: {e}")))?;
    let cert_der = key.cert.der().to_vec();
    let key_der = key.key_pair.serialize_der();
    std::fs::create_dir_all(dir).map_err(|e| CoreError::Storage(e.to_string()))?;
    std::fs::write(&cert_path, &cert_der).map_err(|e| CoreError::Storage(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&key_path)
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        file.write_all(&key_der)
            .map_err(|e| CoreError::Storage(e.to_string()))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&key_path, &key_der).map_err(|e| CoreError::Storage(e.to_string()))?;
    Ok(Identity { cert_der, key_der })
}

#[derive(Debug)]
struct PinnedVerifier {
    /// `None` = capture-only mode (pairing preview).
    expected: Option<String>,
    captured: Mutex<Option<Vec<u8>>>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        *self.captured.lock().unwrap() = Some(end_entity.as_ref().to_vec());
        match &self.expected {
            Some(exp) if &fp == exp => Ok(ServerCertVerified::assertion()),
            Some(_) => Err(rustls::Error::General(format!(
                "fingerprint mismatch: got {fp}"
            ))),
            None => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn pinned_client_config(
    expected: Option<String>,
) -> Result<(Arc<rustls::ClientConfig>, Arc<PinnedVerifier>), CoreError> {
    ensure_provider();
    let verifier = Arc::new(PinnedVerifier {
        expected,
        captured: Mutex::new(None),
    });
    let cfg = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    Ok((Arc::new(cfg), verifier))
}

/// Pairing authenticates possession of both keys; trust is granted later by
/// comparing the exporter code. Ordinary ping clients may omit their cert.
#[derive(Debug)]
struct PairClientVerifier;
impl rustls::server::danger::ClientCertVerifier for PairClientVerifier {
    fn client_auth_mandatory(&self) -> bool {
        false
    }
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _cert: &CertificateDer<'_>,
        _chain: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        // Self-signed identities are expected; verify their handshake signature below.
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
pub fn pairing_server_config(identity: &Identity) -> Result<Arc<rustls::ServerConfig>, CoreError> {
    ensure_provider();
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(Arc::new(PairClientVerifier))
        .with_single_cert(vec![CertificateDer::from(identity.cert_der.clone())], key)
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    Ok(Arc::new(config))
}
pub(crate) async fn connect_pairing(
    peer: &str,
    identity: &Identity,
) -> Result<
    (
        tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
        String,
    ),
    CoreError,
> {
    ensure_provider();
    let verifier = Arc::new(PinnedVerifier {
        expected: None,
        captured: Mutex::new(None),
    });
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![CertificateDer::from(identity.cert_der.clone())], key)
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let stream = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let tcp = tokio::net::TcpStream::connect(peer)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(server_name(), tcp)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))
    })
    .await
    .map_err(|_| CoreError::Transport("pairing connection timed out".into()))??;
    let cert = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .ok_or_else(|| CoreError::Transport("no peer certificate".into()))?;
    let fp = fingerprint(cert.as_ref());
    Ok((stream, fp))
}

/// Send clipboard text over pinned TLS while proving this device's identity.
pub async fn tls_send_clipboard(
    peer: &str,
    identity: &Identity,
    expected_fp: &str,
    packet: &crate::pair_session::ClipboardPacket,
) -> Result<(), CoreError> {
    ensure_provider();
    let verifier = Arc::new(PinnedVerifier {
        expected: Some(expected_fp.into()),
        captured: Mutex::new(None),
    });
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![CertificateDer::from(identity.cert_der.clone())], key)
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let mut stream = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let tcp = tokio::net::TcpStream::connect(peer)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(server_name(), tcp)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))
    })
    .await
    .map_err(|_| CoreError::Transport("clipboard connection timed out".into()))??;
    write_frame(&mut stream, &codec::encode(packet)?).await?;
    #[derive(serde::Deserialize)]
    struct Ack {
        ok: bool,
        error: Option<String>,
    }
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut stream))
        .await
        .map_err(|_| CoreError::Transport("clipboard acknowledgement timed out".into()))??;
    let ack: Ack = codec::decode(&bytes)?;
    if !ack.ok {
        return Err(CoreError::InvalidData(
            ack.error.unwrap_or_else(|| "clipboard rejected".into()),
        ));
    }
    Ok(())
}

/// Send a phone notification over mutually authenticated, certificate-pinned TLS.
pub async fn tls_send_notification(
    peer: &str,
    identity: &Identity,
    expected_fp: &str,
    packet: &crate::pair_session::NotificationPacket,
) -> Result<(), CoreError> {
    ensure_provider();
    let verifier = Arc::new(PinnedVerifier {
        expected: Some(expected_fp.into()),
        captured: Mutex::new(None),
    });
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![CertificateDer::from(identity.cert_der.clone())], key)
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let mut stream = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let tcp = tokio::net::TcpStream::connect(peer)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(server_name(), tcp)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))
    })
    .await
    .map_err(|_| CoreError::Transport("notification connection timed out".into()))??;
    write_frame(&mut stream, &codec::encode(packet)?).await?;
    #[derive(serde::Deserialize)]
    struct Ack {
        ok: bool,
        error: Option<String>,
    }
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut stream))
        .await
        .map_err(|_| CoreError::Transport("notification acknowledgement timed out".into()))??;
    let ack: Ack = codec::decode(&bytes)?;
    if !ack.ok {
        return Err(CoreError::InvalidData(
            ack.error.unwrap_or_else(|| "notification rejected".into()),
        ));
    }
    Ok(())
}

/// Send an authenticated gallery request or response over the paired TLS connection.
pub async fn tls_send_gallery(
    peer: &str,
    identity: &Identity,
    expected_fp: &str,
    packet: &crate::pair_session::GalleryPacket,
) -> Result<(), CoreError> {
    ensure_provider();
    let verifier = Arc::new(PinnedVerifier {
        expected: Some(expected_fp.into()),
        captured: Mutex::new(None),
    });
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![CertificateDer::from(identity.cert_der.clone())], key)
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let mut stream = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let tcp = tokio::net::TcpStream::connect(peer)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(server_name(), tcp)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))
    })
    .await
    .map_err(|_| CoreError::Transport("gallery connection timed out".into()))??;
    write_frame(&mut stream, &codec::encode(packet)?).await?;
    #[derive(serde::Deserialize)]
    struct Ack {
        ok: bool,
        error: Option<String>,
    }
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(12), read_frame(&mut stream))
        .await
        .map_err(|_| CoreError::Transport("gallery acknowledgement timed out".into()))??;
    let ack: Ack = codec::decode(&bytes)?;
    if !ack.ok {
        return Err(CoreError::InvalidData(
            ack.error
                .unwrap_or_else(|| "gallery request rejected".into()),
        ));
    }
    Ok(())
}

/// Send an authenticated SMS request or response over a pinned paired TLS connection.
pub async fn tls_send_messages(
    peer: &str,
    identity: &Identity,
    expected_fp: &str,
    packet: &crate::pair_session::MessagesPacket,
) -> Result<(), CoreError> {
    ensure_provider();
    let verifier = Arc::new(PinnedVerifier {
        expected: Some(expected_fp.into()),
        captured: Mutex::new(None),
    });
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![CertificateDer::from(identity.cert_der.clone())], key)
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    let mut stream = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let tcp = tokio::net::TcpStream::connect(peer)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(server_name(), tcp)
            .await
            .map_err(|e| CoreError::Transport(e.to_string()))
    })
    .await
    .map_err(|_| CoreError::Transport("messages connection timed out".into()))??;
    write_frame(&mut stream, &codec::encode(packet)?).await?;
    #[derive(serde::Deserialize)]
    struct Ack {
        ok: bool,
        error: Option<String>,
    }
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(12), read_frame(&mut stream))
        .await
        .map_err(|_| CoreError::Transport("messages acknowledgement timed out".into()))??;
    let ack: Ack = codec::decode(&bytes)?;
    if !ack.ok {
        return Err(CoreError::InvalidData(
            ack.error
                .unwrap_or_else(|| "messages request rejected".into()),
        ));
    }
    Ok(())
}

/// Build a TLS server config from a device identity (also used by Android glue).
pub fn server_config(identity: &Identity) -> Result<Arc<rustls::ServerConfig>, CoreError> {
    ensure_provider();
    let cert = CertificateDer::from(identity.cert_der.clone());
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|e| CoreError::Transport(format!("bad key: {e}")))?;
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    Ok(Arc::new(cfg))
}

fn server_name() -> ServerName<'static> {
    // Ignored by PinnedVerifier; rustls just requires a syntactically valid name.
    "link.local".try_into().unwrap()
}

/// Pairing preview: fetch the peer's cert fingerprint without trusting anything.
pub async fn fetch_fingerprint(peer: &str) -> Result<String, CoreError> {
    let (cfg, verifier) = pinned_client_config(None)?;
    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::TcpStream::connect(peer),
    )
    .await
    .map_err(|_| CoreError::Transport(format!("connect timeout: {peer}")))
    .and_then(|r| r.map_err(|e| CoreError::Transport(e.to_string())))?;
    let conn = tokio_rustls::TlsConnector::from(cfg);
    let _stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        conn.connect(server_name(), tcp),
    )
    .await
    .map_err(|_| CoreError::Transport("tls handshake timeout".into()))?
    .map_err(|e| CoreError::Transport(format!("tls: {e}")))?;
    let guard = verifier.captured.lock().unwrap();
    let der = guard
        .as_ref()
        .ok_or_else(|| CoreError::Transport("no cert presented".into()))?;
    Ok(fingerprint(der))
}

/// TLS ping with pinned peer fingerprint. Fails closed on mismatch.
pub async fn tls_ping(peer: &str, ping: &Ping, expected_fp: &str) -> Result<Pong, CoreError> {
    let (cfg, _v) = pinned_client_config(Some(expected_fp.to_string()))?;
    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::TcpStream::connect(peer),
    )
    .await
    .map_err(|_| CoreError::Transport(format!("connect timeout: {peer}")))
    .and_then(|r| r.map_err(|e| CoreError::Transport(e.to_string())))?;
    let conn = tokio_rustls::TlsConnector::from(cfg);
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        conn.connect(server_name(), tcp),
    )
    .await
    .map_err(|_| CoreError::Transport("tls handshake timeout".into()))?
    .map_err(|e| CoreError::Transport(format!("tls: {e}")))?;
    write_frame(&mut stream, &codec::encode(ping)?).await?;
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

/// TLS ping server until Ctrl-C.
pub async fn tls_serve(
    port: u16,
    device_id: DeviceId,
    identity: &Identity,
) -> Result<(), CoreError> {
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .map_err(|e| CoreError::Transport(e.to_string()))?;
    tls_serve_until(port, device_id, listener, server_config(identity)?, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

/// Test-friendly variant with caller-provided listener + shutdown.
pub async fn tls_serve_until(
    _port: u16,
    device_id: DeviceId,
    listener: tokio::net::TcpListener,
    server_cfg: Arc<rustls::ServerConfig>,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), CoreError> {
    tls_serve_with_pairing_until(device_id, listener, server_cfg, None, shutdown).await
}

pub async fn tls_serve_with_pairing_until(
    device_id: DeviceId,
    listener: tokio::net::TcpListener,
    server_cfg: Arc<rustls::ServerConfig>,
    pairing: Option<Arc<crate::pair_session::PairManager>>,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), CoreError> {
    tokio::pin!(shutdown);
    let acceptor = tokio_rustls::TlsAcceptor::from(server_cfg);
    let limit = Arc::new(tokio::sync::Semaphore::new(64));
    loop {
        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            accepted = listener.accept() => {
                let (tcp,address) = accepted.map_err(|e| CoreError::Transport(e.to_string()))?;
                let permit = match limit.clone().try_acquire_owned() {Ok(p) => p, Err(_) => continue};
                let acceptor = acceptor.clone();
                let pairing = pairing.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let mut stream = match tokio::time::timeout(std::time::Duration::from_secs(5),acceptor.accept(tcp)).await {Ok(Ok(s))=>s,_=>return};
                    let frame = match tokio::time::timeout(std::time::Duration::from_secs(5),read_frame(&mut stream)).await {Ok(Ok(f)) if f.len() <= 8 * 1024 * 1024 => f,_=>return};
                    if let Ok(ping) = codec::decode::<Ping>(&frame) {
                        let pong = Pong { nonce: ping.nonce, from: device_id };
                        if let Ok(bytes) = codec::encode(&pong) {
                            let _ = tokio::time::timeout(std::time::Duration::from_secs(5),write_frame(&mut stream,&bytes)).await;
                        }
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5),stream.shutdown()).await;
                    } else if let (Some(manager),Ok(hello)) = (pairing.clone(),codec::decode::<crate::pair_session::PairHello>(&frame)) {
                        if let Err(e) = manager.incoming(stream,hello,address).await {tracing::debug!("pairing declined: {e}");}
                    } else if let (Some(manager),Ok(packet)) = (pairing.clone(),codec::decode::<crate::pair_session::ClipboardPacket>(&frame)) {
                        let cert_fp = stream.get_ref().1.peer_certificates().and_then(|certs| certs.first()).map(|cert| fingerprint(cert.as_ref()));
                        let result = match cert_fp { Some(fp) => manager.receive_clipboard(packet, &fp), None => Err(CoreError::InvalidData("clipboard requires an authenticated paired device".into())) };
                        let ack = match result { Ok(()) => serde_json::json!({"ok":true}), Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}) };
                        if let Ok(bytes) = codec::encode(&ack) { let _ = tokio::time::timeout(std::time::Duration::from_secs(5),write_frame(&mut stream,&bytes)).await; }
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5),stream.shutdown()).await;
                    } else if let (Some(manager),Ok(packet)) = (pairing.clone(),codec::decode::<crate::pair_session::MessagesPacket>(&frame)) {
                        let cert_fp = stream.get_ref().1.peer_certificates().and_then(|certs| certs.first()).map(|cert| fingerprint(cert.as_ref()));
                        let result = match cert_fp { Some(fp) => manager.receive_messages(packet, &fp), None => Err(CoreError::InvalidData("messages require an authenticated paired device".into())) };
                        let ack = match result { Ok(()) => serde_json::json!({"ok":true}), Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}) };
                        if let Ok(bytes) = codec::encode(&ack) { let _ = tokio::time::timeout(std::time::Duration::from_secs(5),write_frame(&mut stream,&bytes)).await; }
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5),stream.shutdown()).await;
                    } else if let (Some(manager),Ok(packet)) = (pairing.clone(),codec::decode::<crate::pair_session::GalleryPacket>(&frame)) {
                        let cert_fp = stream.get_ref().1.peer_certificates().and_then(|certs| certs.first()).map(|cert| fingerprint(cert.as_ref()));
                        let result = match cert_fp { Some(fp) => manager.receive_gallery(packet, &fp), None => Err(CoreError::InvalidData("gallery requires an authenticated paired device".into())) };
                        let ack = match result { Ok(()) => serde_json::json!({"ok":true}), Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}) };
                        if let Ok(bytes) = codec::encode(&ack) { let _ = tokio::time::timeout(std::time::Duration::from_secs(5),write_frame(&mut stream,&bytes)).await; }
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5),stream.shutdown()).await;
                    } else if let (Some(manager),Ok(packet)) = (pairing,codec::decode::<crate::pair_session::NotificationPacket>(&frame)) {
                        let cert_fp = stream.get_ref().1.peer_certificates().and_then(|certs| certs.first()).map(|cert| fingerprint(cert.as_ref()));
                        let result = match cert_fp { Some(fp) => manager.receive_notification(packet, &fp), None => Err(CoreError::InvalidData("notification requires an authenticated paired device".into())) };
                        let ack = match result { Ok(()) => serde_json::json!({"ok":true}), Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}) };
                        if let Ok(bytes) = codec::encode(&ack) { let _ = tokio::time::timeout(std::time::Duration::from_secs(5),write_frame(&mut stream,&bytes)).await; }
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(5),stream.shutdown()).await;
                    }
                });
            }
        }
    }
}

mod hex {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        let mut s = String::with_capacity(bytes.as_ref().len() * 2);
        for b in bytes.as_ref() {
            s.push(CHARS[(b >> 4) as usize] as char);
            s.push(CHARS[(b & 0xf) as usize] as char);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity() -> Identity {
        ensure_provider();
        let key = rcgen::generate_simple_self_signed(vec!["test".to_string()]).unwrap();
        Identity {
            cert_der: key.cert.der().to_vec(),
            key_der: key.key_pair.serialize_der(),
        }
    }

    /// A copied pinned certificate must not authenticate a different private key.
    #[tokio::test]
    async fn rejects_pinned_certificate_with_wrong_signing_key() {
        for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
            let identity = test_identity();
            let impostor = test_identity();
            let key = rustls::pki_types::PrivateKeyDer::try_from(impostor.key_der).unwrap();
            let signing_key = rustls::crypto::ring::sign::any_supported_type(&key).unwrap();
            let certified = rustls::sign::CertifiedKey::new(
                vec![CertificateDer::from(identity.cert_der.clone())],
                signing_key,
            );
            // A custom resolver intentionally bypasses the normal cert/key match check.
            let config = rustls::ServerConfig::builder_with_protocol_versions(&[version])
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(rustls::sign::SingleCertAndKey::from(certified)));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let peer = listener.local_addr().unwrap().to_string();
            let (tx, rx) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(tls_serve_until(
                0,
                DeviceId::generate(),
                listener,
                Arc::new(config),
                async move {
                    let _ = rx.await;
                },
            ));
            let result = tls_ping(
                &peer,
                &Ping {
                    nonce: 1,
                    from: DeviceId::generate(),
                },
                &identity.fingerprint(),
            )
            .await;
            assert!(
                result.is_err(),
                "copied cert authenticated without its private key"
            );
            let _ = tx.send(());
            server.await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn tls_ping_pong_pinned() {
        let id = test_identity();
        let fp = id.fingerprint();
        let device_id = DeviceId::generate();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let cfg = server_config(&id).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            tls_serve_until(port, device_id, listener, cfg, async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
        });
        // Pairing preview works without trust.
        let fetched = fetch_fingerprint(&format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        assert_eq!(fetched, fp);
        // Pinned ping succeeds.
        let ping = Ping {
            nonce: 7,
            from: DeviceId::generate(),
        };
        let pong = tls_ping(&format!("127.0.0.1:{port}"), &ping, &fp)
            .await
            .unwrap();
        assert_eq!(pong.nonce, 7);
        assert_eq!(pong.from, device_id);
        // Wrong pin fails closed.
        let bad = tls_ping(
            &format!("127.0.0.1:{port}"),
            &ping,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .await;
        assert!(bad.is_err());
        let _ = tx.send(());
        server.await.unwrap();
    }
}
