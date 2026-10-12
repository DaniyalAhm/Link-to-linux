# Security

LAN-only in v0.1. Report issues privately to the maintainers. Do not file public issues for key-pinning bypasses, pairing-code brute force, or file-transfer path traversal until triaged.
Threat model: untrusted LAN. All post-pair traffic must be TLS with pinned certs. Clipboard sync is opt-in and off by default on each endpoint. Clipboard packets require a mutual TLS client certificate whose fingerprint and declared device ID match a stored pairing. Text is limited to 32 KiB; PNG/JPEG/WebP images are limited to 5 MiB. Contents are kept only in memory while queued for Android foreground delivery; they are not logged or persisted.
