# Contributing

Phase 0 rules: keep `link-core` OS-agnostic. No `std::env` home-dir hacks outside `platform.rs`. All wire changes update `proto/link.cddl` + `link-core/src/codec.rs` + tests.

- `just check` must pass before push (fmt, clippy `-D warnings`, check workspace).
- `just test` uses `cargo nextest`.
- Conventional commits (`feat:`, `fix:`). Sign off DCO-style.
