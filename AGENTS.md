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
- **Local run:** `./target/debug/synapse-server --port 4173 --token CODE --cwd <dir> [--bin <claude>]`, then `./target/debug/synapse-web --port 8765` and open `http://127.0.0.1:8765/?host=127.0.0.1&port=4173&token=CODE` (host/port/token query params auto-connect the web client; no native bridge needed). `synapse-server` only serves the WS + `/api/*` endpoints; the web HTML/JS is served separately by `synapse-web` (default `:8765`).

### Backing the `claude` CLI with DeepSeek (no Anthropic account)

`synapse-server` spawns whatever `claude` CLI it's given (PATH / `--bin` / `CLAUDE_BIN`); that CLI just needs an Anthropic-compatible endpoint. DeepSeek exposes one, so you can run the real product against DeepSeek instead of Anthropic:

1. Install the CLI once: `npm config set prefix ~/.npm-global && npm i -g @anthropic-ai/claude-code` (installs `~/.npm-global/bin/claude`; global `npm i -g` to the default prefix needs root here).
2. Provide DeepSeek config to the CLI. **Never commit the key.** Preferred for cloud agents: store the key as a Secret and let the spawned `claude` inherit these from the environment (Claude Code reads them from env): `ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic`, `ANTHROPIC_AUTH_TOKEN=<DeepSeek key>` (also set `ANTHROPIC_API_KEY` to the same value — some versions check both), `ANTHROPIC_MODEL=deepseek-v4-flash`. Manual alternative: write the same keys into `~/.claude/settings.json` under an `"env": { … }` object (home dir, not the repo). Claude Code maps `claude-opus*`→`deepseek-v4-pro`, `claude-sonnet*`/`claude-haiku*`→`deepseek-v4-flash`; the current DeepSeek models are `deepseek-v4-flash` and `deepseek-v4-pro[1m]` (`deepseek-chat`/`deepseek-reasoner` are legacy aliases).
3. Point the server at it: `synapse-server … --bin /home/<user>/.npm-global/bin/claude`. Verify with `echo hi | ~/.npm-global/bin/claude -p --output-format json` — a real reply with `modelUsage.deepseek-v4-flash` confirms it. Note: replies may self-identify as "Claude" (Claude Code's system prompt) even though inference is DeepSeek.

### Reaching the web UI from a phone / off-VM

The web UI runs on the VM's localhost, so a phone can't hit it directly. Expose **both** ports over HTTPS/WSS (e.g. two `cloudflared tunnel --url http://127.0.0.1:<port>` quick tunnels — one for `:8765`, one for `:4173`), then open on the phone:
`https://<web-tunnel-host>/?host=<server-tunnel-host>&port=443&token=CODE&tls=1`. The page loads from the `:8765` tunnel and the in-page JS dials `wss://<server-tunnel-host>:443/?token=CODE`. (`synapse-server --tunnel` only tunnels the server/WS port; it does not serve the HTML, so the `:8765` page still needs its own tunnel — or use the native iOS app, which embeds the bundle and only needs the server tunnel.)
