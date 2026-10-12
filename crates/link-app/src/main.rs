//! Dioxus frontend entry point.
//!
//! Phase 0 builds WITHOUT the `ui` feature so `cargo check --workspace`
//! does not require WebKitGTK/system WebView. Enable the real UI with:
//!
//! ```sh
//! cargo run -p link-app --features ui
//! # or, once `dx` is installed:
//! dx serve --platform desktop --features ui
//! ```

mod daemon_client;

#[cfg(feature = "ui")]
mod ui;

#[cfg(feature = "ui")]
fn main() {
    ui::launch();
}

#[cfg(not(feature = "ui"))]
#[tokio::main]
async fn main() {
    // Headless mode: no WebView needed. Shows the trust store via the daemon.
    match daemon_client::DaemonClient::discover() {
        Err(e) => println!("daemon not found: {e:#}"),
        Ok(c) => match c.pairs().await {
            Err(e) => println!("pairs failed: {e:#}"),
            Ok(list) => {
                println!("{} trusted pairing(s)", list.len());
                for d in &list {
                    println!("{} {} {}", d.id, d.name, d.cert_fingerprint);
                }
                println!("Re-run with `--features ui` for the full GUI.");
            }
        },
    }
}
