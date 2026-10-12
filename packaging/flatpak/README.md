# Build a local Flatpak

Run these commands from the project root. On Manjaro, install the builder
with `sudo pacman -S flatpak-builder`.

```sh
flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y flathub org.gnome.Platform//50 org.gnome.Sdk//50 org.freedesktop.Sdk.Extension.rust-stable//25.08
flatpak-builder --user --force-clean --repo=flatpak-repo flatpak-build packaging/flatpak/org.linktolinux.App.json
flatpak build-bundle flatpak-repo LinkToLinux.flatpak org.linktolinux.App --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user ./LinkToLinux.flatpak
flatpak run org.linktolinux.App
```

The build includes the graphical frontend, daemon, libxdo, and Wayland
clipboard tools. SDK downloads require several GB of disk space. Initial
release compilation can take several minutes. The local manifest allows
network access during the Rust build to fetch locked Cargo dependencies;
a Flathub submission would need generated offline Cargo sources and AppStream
metadata.

Flatpak stores app data separately from a native installation, so existing
native pairings are not automatically imported. Stop the native daemon before
launching the Flatpak: both use the same network listener ports. Clipboard
sync still requires the frontend to remain running. This package does not
install a host systemd service.

To build an updated bundle, repeat the builder and bundle commands. Install it
with `flatpak install --user ./LinkToLinux.flatpak`.
