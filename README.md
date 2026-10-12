# Link to Linux

LAN-only Android ↔ Linux link, with a shared Rust protocol, Linux daemon,
Dioxus desktop UI, and a Kotlin Android app connected through JNI.

Implemented: mDNS / UDP discovery, TLS ping/pong, stable identities, persisted
certificate pins, two-sided confirmation-code pairing, and opt-in text/image
clipboard sync. Android notifications can also be forwarded to Linux. Linux can
read SMS/MMS history and send messages through paired Android phones after the
user enables message sharing and grants Android's read/send permissions.

## Run the desktop app

### Build from source

Install stable Rust, `just`, and the development packages for WebKitGTK 4.1
and libxdo. For clipboard sync, install `wl-clipboard` and use a Wayland session.
Run these commands from the project root:

```sh
cargo build -p link-daemon
just ui
```

`just ui` supports a user-space libxdo installation in `~/.local/lib`.
On Wayland issues, try `GDK_BACKEND=x11 just ui`; clipboard sync still uses
Wayland tools. `LINK_DAEMON_BIN` overrides daemon lookup, otherwise the app
uses a sibling binary or PATH.

### Build a local Flatpak

The local Flatpak packages the desktop app, daemon, libxdo, and Wayland
clipboard tools. See the [Flatpak build guide](packaging/flatpak/README.md)
for prerequisites and commands to build and install `LinkToLinux.flatpak`.
After installation, launch it from your application menu or run:

```sh
flatpak run org.linktolinux.App
```

Stop the native daemon before launching the Flatpak, since both use the same
network ports. Flatpak app data is separate, so pair your devices again when
switching from a native installation. The package does not install a host
systemd service.

## Pair your devices

The UI starts the daemon when needed. Nearby devices and incoming pairing
requests update automatically. Choose **Pair device** on a discovered device,
or enter `host:port` under **Connect by address** (IPv6: `[address]:port`).
Open the Android app on the other device. Compare the six-digit code on both
screens and confirm on **both** within two minutes. Reject if the codes differ.
Neither device saves trust after just one approval. Trusted devices have Ping
and Unpair actions; incoming requests can also be approved from either UI.

The code is derived independently from the mutually key-authenticated TLS
connection using an exporter, rather than sent over the network. New requests
have new sessions; expiry, disconnect, and rejection do not grant trust.
Changed stored fingerprints require unpairing before a new pairing.

## Features and permissions

### Clipboard

Clipboard sync is off until enabled on each device. The desktop app uses
`wl-paste` and `wl-copy` on Wayland. Android applies incoming clips from its
foreground service while minimized. It only reads local clipboard data while
Link is focused, per Android's clipboard privacy restrictions; returning to
Link checks and sends the latest clip. The running Android notification has a
**Send clipboard** action that opens Link and sends the current clip. This is an
Android platform limitation, not a network or pairing failure. A possible
future workaround is an optional Link keyboard (IME): Android allows the active
keyboard to read clipboard data in the background, but users would have to
select it as their default keyboard and trust it with their typing. Sync accepts
plain text up to 32 KiB and
PNG/JPEG/WebP images up to 5 MiB; it does not save clipboard contents to disk.

Keep the desktop frontend running for clipboard sync.

### Photo gallery

The Linux app can open a paired Android device's gallery as a live, paged photo
collection with thumbnails. On Android, grant gallery access in Link settings;
Android 14+ can grant access to selected photos only. Full-size photos are read
over the paired TLS connection only when opened, with a 5 MiB transfer limit.
Larger photos are resized and converted to WebP on Android; the gallery is not
copied wholesale onto Linux storage.

### Messages and contacts

The Linux **Messages** view reads the phone's SMS/MMS conversations, including
carrier group threads, and sends individual SMS or group MMS over pinned TLS.
Enable **Messages (SMS/MMS)** on Android and grant read/send access. Tap
**Allow contact access** to browse/search phone contacts in Linux's **New message**
view; select multiple contacts for a group MMS. Conversations retain Android's
thread IDs, recipients, and newest-first ordering. History loads in pages of 100
with **Load older messages**; open conversations refresh every five seconds.
Android send callbacks report success or failure, and failed drafts can be retried.
RCS conversations are not exposed by the Android SMS/MMS provider. MMS attachments
currently appear as type labels (for example, `[Photo]`), without media downloads.
Group MMS needs carrier MMS support and mobile data; carrier charges may apply.
Message and contact content is kept in Linux process memory, not saved to disk.

### Notifications

Notification forwarding is off by default. Enable **Forwarding** in Android
Link and grant notification access in Android Settings. Android sends titles
and message text to trusted paired devices over pinned TLS. The Linux daemon
displays notifications even while the desktop window is closed. Ongoing
notifications and apps on your block list are skipped.

## Android

See [android/README.md](android/README.md) for APK builds and emulator testing.
The phone and computer normally use an existing LAN; router port forwarding
is unnecessary. Android starts its foreground link service on app launch so
it can advertise itself and receive pairing requests.

## Daemon CLI

```sh
cargo run -p link-daemon -- serve --name laptop
# In another terminal:
target/debug/link-daemon status
target/debug/link-daemon devices --json
target/debug/link-daemon pair --peer 192.168.1.20:53536 --json
target/debug/link-daemon pair-requests --json
# Use the session and code shown by the request, after comparing both screens:
target/debug/link-daemon pair-confirm --session <session> --code <six-digit-code>
# Or reject:
target/debug/link-daemon pair-confirm --session <session> --reject
target/debug/link-daemon pairs --json
target/debug/link-daemon ping --peer 192.168.1.20:53536
```

Commands return JSON. Pairing commands use the daemon's private Unix socket;
the daemon must stay running while users confirm. `--yes` was removed: it
cannot bypass either side's confirmation. The desktop UI handles these steps.

Network: UDP broadcast `53535`, mDNS `_linktolinux._udp.local.`, TLS TCP `53536`.
The service name retains `_udp` for discovery compatibility; data currently
uses TLS over TCP. QUIC remains a skeleton.

## Layout and checks

- `crates/link-core`: discovery, TLS, CBOR, pairing sessions and stores.
- `crates/link-daemon`: listener, presence cache, private socket, CLI.
- `crates/link-app`: Dioxus desktop UI (`--features ui`).
- `crates/link-android-svc`: JNI and Android-owned Rust runtime.
- `proto/link.cddl`: wire types, including pairing hello and approval messages.
- `android`: Kotlin app, Gradle build, and emulator instrumentation tests.
- `packaging`: local Flatpak build and systemd service template.

Install `cargo-nextest` to run `just test`. The workspace checks use the
headless desktop build; check the graphical frontend separately with its
`ui` feature enabled:

```sh
just check
just test
cargo clippy -p link-app --features ui --all-targets -- -D warnings
```

MIT OR Apache-2.0. See [CONTRIBUTING.md](CONTRIBUTING.md),
[SECURITY.md](SECURITY.md), and the [MIT](LICENSE-MIT) and
[Apache-2.0](LICENSE-APACHE) license files.
