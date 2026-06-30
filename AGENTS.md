# Synapse — agent notes

## After changing the app, rebuild it yourself — don't make the user run the build

The web chat bundle (`crates/app/web/`) is **compiled into the binary** (`include_str!` / `include_bytes!`), and the iOS app is the surface the user actually tests. So **any** change under `crates/app/web/**` or `crates/app/src/**` has *zero* effect until the app is rebuilt + reinstalled — editing the files alone changes nothing on the running app.

When you finish a feature / UI tweak / fix that touches the app, **rebuild and relaunch it automatically**. Do not leave a manual "now run X" step for the user.

- **iOS simulator (default test surface):** `./mobile/build-sim.sh` — builds Debug for `iphonesimulator`, installs on the booted sim, and launches it. The Xcode "Build Rust staticlib" phase recompiles the `aarch64-apple-ios-sim` lib so the new web bundle is embedded.
- **Physical device:** `./mobile/build-ios.sh` (needs `DEVELOPMENT_TEAM`).

Server-only changes (`crates/server/**`) don't need an app rebuild — rebuild + restart `synapse-server` instead.

Always verify on the real surface (the sim app), not just a browser tab.

## Cursor Cloud specific instructions

This is a **Linux** dev VM. Standard build/lint/test/run commands live in `README.md` and `.github/workflows/ci.yml` — use those. Notes specific to this environment:

- **iOS is out of scope here.** `./mobile/build-sim.sh` / `./mobile/build-ios.sh` need macOS + Xcode (the `iphoneos` SDK for the `ring` crate) and cannot run on this Linux VM. The in-scope surfaces are the Rust workspace (`synapse-server`, `synapse-app`, `synapse-relay`, `synapse-web`) and the **web chat bundle** served by `synapse-web`. On Linux, verify on the web bundle in a browser instead of the sim app.
- **Toolchain:** there is no `Cargo.lock` or `rust-toolchain.toml`, so `cargo` resolves the newest crates, some of which need a recent stable Rust (`edition2024`). Use the latest `rustup default stable` — an older default (e.g. 1.83) fails to parse such manifests. System libs `libfontconfig1-dev` + `pkg-config` are required (Slint, in the `synapse-app` crate); `cargo build --workspace` fails without them.
- **`cargo clippy -D warnings` currently fails** on a pre-existing `collapsible_match` lint in `crates/app/src/lib.rs` flagged by newer stable clippy. `cargo fmt --all -- --check`, `cargo test --workspace`, and `cargo build --workspace` are clean. Don't "fix" this as part of unrelated work.
- **End-to-end run / the `claude` dependency:** `synapse-server` shells out to a real `claude -p` CLI (resolved from PATH or `--bin`/`CLAUDE_BIN`), which needs Anthropic auth to produce real replies. To exercise the full pipeline **without** credentials, point `--bin` at a stub that speaks the stream-json protocol (handle `agents --json` → `[]`; on a streaming turn read the user line from stdin and emit `system/init`, `assistant`, then `result` lines). Real usage needs the actual `claude` CLI authenticated.
- **Local run:** `./target/debug/synapse-server --port 4173 --token CODE --cwd <dir> [--bin <claude>]`, then `./target/debug/synapse-web --port 8765` and open `http://127.0.0.1:8765/?host=127.0.0.1&port=4173&token=CODE` (host/port/token query params auto-connect the web client; no native bridge needed).
