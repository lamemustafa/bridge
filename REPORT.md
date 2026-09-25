# Cloud Lane T (trial) report

Append-only log. Newest entries at the bottom. This branch is never merged.

## Fri Sep 25 14:23:36 UTC 2026 — start

- Base: `origin/master` at `a51ae783054fddb77e73ed9f044f7516335e8826`.
- Host: Ubuntu 24.04.4 LTS, x86_64, 4 vCPU, 15 GiB RAM, running as root (sudo not needed).
- Toolchain: `rustup show` reports `1.96.0-x86_64-unknown-linux-gnu`, active via `rust-toolchain.toml`.
- Installed via `apt-get install` in 41 s (179 new packages, 19 upgraded): `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev`. Already present: `libssl-dev build-essential pkg-config perl`.
- Next: cold `cargo test --locked -p bridge --lib` from `src-tauri`.
