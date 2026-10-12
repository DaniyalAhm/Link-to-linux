set shell := ["sh", "-cu"]

default:
    @just --list

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo check --workspace --all-targets

test:
    cargo nextest run --workspace

# GUI needs libxdo at link+run time. System package preferred
# (`sudo pacman -S xdotool`); fallback is the user-space build in
# ~/.local/lib (see README "Linux GUI" section). LINK_DAEMON_BIN defaults
# to target/debug/link-daemon (sibling of the app binary) when unset.
ui:
    RUSTFLAGS="-L $HOME/.local/lib" LD_LIBRARY_PATH="$HOME/.local/lib:$LD_LIBRARY_PATH" cargo run -p link-app --features ui

daemon:
    cargo run -p link-daemon -- serve

# Android emulator proof (needs ~/Android/Sdk + ~/.jdk from Phase 1a setup).
# Boot: `just emu-boot`, then in another shell: start daemon, then `just emu-proof`.
emu-boot:
    JAVA_HOME=$HOME/.jdk PATH=$HOME/.jdk/bin:$HOME/Android/Sdk/platform-tools:$HOME/Android/Sdk/emulator:$PATH ANDROID_HOME=$HOME/Android/Sdk emulator -avd link-pixel -no-window -no-audio -no-boot-anim -gpu swiftshader_indirect -memory 3072

# Open emulator pairing UI. Compare/confirm the code in both apps.
emu-proof:
    adb shell am start -n org.linktolinux.app.debug/org.linktolinux.app.MainActivity --ez startsvc true
    adb forward tcp:53537 tcp:53536
    @echo 'Open just ui, pair 127.0.0.1:53537, then confirm the code on both devices.'
