# Android app

Gradle builds the APK. Rust owns TLS, device identity, and pairing; Kotlin owns
the activity, foreground service, Android NSD discovery, and confirmation UI.

## Pairing and discovery

The activity browses `_linktolinux._udp.` while visible. The foreground service
starts on launch, advertises the phone, and keeps its TLS listener alive.
Both devices must be reachable on the same network for normal LAN discovery.
Android system NSD owns multicast; the app uses no multicast socket or lock.

Tap a nearby device or enter `host:port` under **Connect to address**. Pairing
shows the same six-digit code in Android and the Rust desktop app. Compare
both screens and tap **Codes match — confirm** on each within two minutes.
The code is independently derived from their TLS connection, never sent over
the network. One approval does not save trust. Reject mismatches; expired or
rejected sessions require a new request. Desktop-initiated requests appear in
the Android device list while the activity is open.

Trusted devices can be pinged or unpaired (long-press). Reopening the activity
checks saved peers once; a newly resolved address triggers another bounded
check. These checks are ping/pong, not permanent message sessions. Changes to
a trusted certificate fail closed and require unpairing before pairing again.

Clipboard sync is off by default; turn on **Clipboard sync** on both devices.
Both devices remember this choice across app and background-service restarts.
Linux saves it in `settings.json` in the app's data directory; Android saves
sharing choices in its app preferences. The Linux theme is remembered separately.
Android's foreground service applies incoming clips while the
activity is minimized. Android only reads local clipboard data while Link has
window focus, per Android's clipboard privacy restrictions; returning to Link
checks and sends the latest clip. This is an Android platform restriction, not
a Link pairing or network issue. The running notification's **Send clipboard**
action opens Link and sends the current clip. A possible future workaround is
an optional Link keyboard (IME), since Android permits the active keyboard to
read clipboard data in the background. Users would need to select it as their
default keyboard and trust it with their typing. Sensitive clips are skipped. The Rust service
checks the sender's TLS certificate against the paired-device record. Text is
limited to 32 KiB and is not written to disk. PNG, JPEG, and WebP images up to
5 MiB are supported. The Linux desktop currently uses `wl-paste`/`wl-copy`
(Wayland).

Linux can open this Android device's gallery as a live, paged photo collection.
Tap **Allow gallery access** in Android Link settings and grant full or
selected-photo access. Linux shows thumbnails from the gallery and reads each
full photo over the paired TLS connection only when opened. This is independent
of clipboard sync. Full photo transfers are limited to 5 MiB; larger photos are
resized and converted to WebP on Android, and the collection is not bulk-copied
onto Linux storage.

The Linux **Messages** view reads SMS and MMS from Android's shared telephony
provider, preserving native thread IDs and group recipients. Enable message
sharing and grant read/send access. **Allow contact access** separately enables
contact search and contact names. Linux can choose multiple phone contacts to
create a real group MMS, using Android's `SmsManager.sendMultimediaMessage` and
an AOSP-derived PDU composer (android-smsmms, Apache 2.0). The library's legacy
network sender is not used. Android owns the carrier connection and sent-message
persistence. Multipart SMS failures and MMS send results return to Linux.

Contacts, conversations, and mixed SMS/MMS history are paginated (100 per page).
Linux refreshes visible messaging data and merges history by SMS/MMS provider IDs.
No contact or message contents are persisted on Linux. RCS is unavailable through
this provider, and MMS attachments currently display type labels rather than
media previews. Carrier group MMS needs mobile data and carrier configuration;
carrier charges may apply. Testing must use the emulator for outbound sends,
or explicitly authorized test recipients on a real phone.

Phone notification forwarding is separately off by default. Turn on
**Forwarding** in the Android app, then grant its notification access in
Android Settings. Titles and message text are sent only to trusted paired
devices over the pinned TLS connection. The Linux background service displays
the alerts, even while its window is closed.

Trust now lives in Rust's `paired.json` so incoming and outgoing requests use
the same store. Earlier Android fingerprint-only SharedPreferences pairings
are not imported: pair those devices again using approval on both sides.
The install ID and TLS certificate remain unchanged. Linux legacy pins remain
readable; all new pairing requests require the code flow.

## Build and install

```sh
cargo check -p link-android-svc
cargo nextest run -p link-android-svc
# Install any missing Rust targets first:
rustup target add aarch64-linux-android x86_64-linux-android
cargo ndk -t arm64-v8a -t x86_64 -o android/app/src/main/jniLibs build -p link-android-svc --release
# Assemble with Android Studio or Gradle:
gradle -p android :app:assembleDebug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n org.linktolinux.app.debug/org.linktolinux.app.MainActivity
```

Minimum SDK 26, target/compile SDK 34; JDK 17 and an Android NDK are required.
Debug application ID is `org.linktolinux.app.debug`. Gradle preBuild now rebuilds
ARM64 and x86_64 native libraries together when Rust or JNI changes, preventing
stale physical-device libraries from being packaged. Optional ARMv7 support needs
`armv7-linux-androideabi` and `-t armeabi-v7a` in the native build command.

## Emulator ↔ Linux proof

The emulator reaches the host through `10.0.2.2:53536`. For desktop-initiated
pairing, forward a host port into the emulator:

```sh
adb -e forward tcp:53537 tcp:53536
```

Open both apps. In the Rust desktop app, enter `127.0.0.1:53537` and click
**Pair device**. Compare and confirm the code in **both** apps. Linux stores
the forwarded address. Android maps the incoming emulator loopback address
back to the host alias for later ping checks. Alternatively, Android's
**Ping host (10.0.2.2)** button starts pairing with the host if it isn't trusted.
`autoping` intent launches now also require confirmation for new peers.

Newer emulators support NSD between emulators on their shared virtual Wi-Fi.
Host/physical LAN mDNS visibility is not established by this emulator proof;
test that path using a phone and Linux machine on the same network.

## Physical LAN check

1. Start the desktop UI and open Android on the same LAN.
2. Check that each finds the other, excluding itself.
3. Start pairing from either side; verify that both show the same code.
4. Confirm only one side and verify neither has saved new trust yet.
5. Confirm the other; verify both display a trusted device and TLS ping works.
6. Reopen both apps; verify the pairing survives.
7. Reject a new request, or let it expire, and verify no trust is saved.

## Native lifecycle

`filesDir` is passed explicitly from Kotlin. `device.json`, `cert.der`,
`key.der`, and `paired.json` live there. `MainActivity` uses JNI methods for
identity, begin/list/confirm pairing, list/unpair peers, and pinned ping.
`LinkService` owns start/stop of the Rust runtime and TLS listener. Android NSD
advertises only after the native listener is ready. `LinkNotifications`
forwards Android alerts to paired Linux devices only when enabled, skips
ongoing notifications, and honors the user's per-app block list. App choices
are learned from non-ongoing notifications seen by the listener.


## Messaging checks

`cargo test -p link-app --features ui messaging_tests` checks paginated history
merging. `:app:assembleDebugAndroidTest` builds emulator instrumentation tests
for contact lookup, combined SMS/MMS history and pagination, group recipient
preservation, and Unicode MMS PDU encoding. The provider tests refuse to run
on physical devices, create temporary fixture rows, and delete them afterward.
Temporarily allow the emulator app's WRITE_SMS app-op for provider fixtures;
restore its previous value after the run. These tests never send carrier MMS.
