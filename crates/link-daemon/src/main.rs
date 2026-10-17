use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

mod ipc;
mod service;
mod settings;

#[derive(Debug, Parser)]
#[command(
    name = "link-daemon",
    about = "Link to Linux background service (Phase 1b: TLS + pinning)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the TLS ping server (foreground; systemd runs this).
    Serve {
        /// Friendly device name advertised in discovery.
        #[arg(long, default_value = "linux")]
        name: String,
        /// TCP port to listen on.
        #[arg(long, default_value_t = link_core::QUIC_PORT)]
        port: u16,
    },
    /// Ping the local daemon IPC socket.
    Status,
    /// Listen for UDP broadcast identities.
    Discover {
        /// Listen window in seconds.
        #[arg(long, default_value_t = 5)]
        timeout_secs: u64,
        /// Machine-readable output for the GUI (`{"devices":[...]}`).
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Instant nearby-device list from the serving daemon's presence cache.
    Devices {
        /// Machine-readable output for the GUI (`{"devices":[...]}`).
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Pair with a peer: show its cert fingerprint, verify, then trust.
    Pair {
        /// Peer address `ip:port`.
        #[arg(long)]
        peer: String,
        /// Machine-readable pairing session (no trust is saved yet).
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// List incoming/outgoing confirmation-code sessions.
    PairRequests {
        #[arg(long)]
        json: bool,
    },
    /// Confirm a matching code, or reject the request.
    PairConfirm {
        #[arg(long)]
        session: String,
        #[arg(long, default_value = "")]
        code: String,
        #[arg(long)]
        reject: bool,
        #[arg(long)]
        json: bool,
    },
    /// TLS ping a paired peer (fails closed without a pinned fingerprint).
    Ping {
        /// Peer address `ip:port`.
        #[arg(long)]
        peer: String,
        /// Expected `sha256:...` fingerprint. Defaults to the single stored
        /// pairing, or the stored entry for this peer address.
        #[arg(long)]
        fingerprint: Option<String>,
    },
    /// List trusted pairings.
    Pairs {
        /// Machine-readable output for the GUI (`{"pairings":[...]}`).
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Remove a trusted pairing by device id.
    Unpair {
        /// Device id (uuid).
        #[arg(long)]
        id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Serve { name, port } => service::serve(&name, port).await,
        Commands::Status => ipc::status().await,
        Commands::Discover { timeout_secs, json } => service::discover(timeout_secs, json).await,
        Commands::Devices { json } => service::devices(json).await,
        Commands::Pair { peer, json } => service::pair(&peer, json).await,
        Commands::PairRequests { .. } => service::sessions().await,
        Commands::PairConfirm {
            session,
            code,
            reject,
            ..
        } => service::confirm(session, code, reject).await,
        Commands::Ping { peer, fingerprint } => service::ping(&peer, fingerprint.as_deref()).await,
        Commands::Pairs { json } => service::pairs(json).await,
        Commands::Unpair { id } => service::unpair(&id).await,
    }
}
