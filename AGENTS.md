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
`https://<web-tunnel-host>/?host=<server-tunnel-host>&port=443&token=CODE&tls=1`. The page loads from the `:8765` tunnel and the in-page JS dials `wss://<server-tunnel-host>:443/?token=CODE`. (`synapse-server --tunnel` only tunnels the server/WS port; it does not serve the HTML, so the `:8765` page still needs its own tunnel — or use the native iOS app, which embeds the bundle and only needs the server tunnel.) `trycloudflare.com` quick-tunnel hostnames are ephemeral (new random host per run, dead once the tunnel/VM stops); use a named tunnel or relay for anything stable.

### Live-editing the web UI (instant, no Rust rebuild) vs. shipping it

The web bundle is `include_*`-baked into the `synapse-web` / `synapse-app` binaries, so `synapse-web` (`:8765`) and the iOS WKWebView always serve the **compiled-in** copy — editing `crates/app/web/**` does nothing there until you rebuild (see the top of this file). For a fast dev loop, serve the source directory from disk with any static server and point a browser at it — edits show on **plain browser refresh**, no rebuild:

```sh
python3 -m http.server 8770 --directory crates/app/web   # or any static server
# open: http://127.0.0.1:8770/?host=127.0.0.1&port=4173&token=CODE  (or via a tunnel for phone)
```

It dials the same running `synapse-server`, so only the static server reloads from disk; the server keeps running. This is dev-only — once the UI looks right, **rebuild to bake it in** (and reinstall the iOS app) or the change won't ship. There is no in-app/over-the-air hot-reload of the bundle in production.

### Deploying updates

CI (`.github/workflows/ci.yml`) only fmt/clippy/test/builds — there is no auto-deploy. To ship:

- **Server / relay (Rust):** `cargo build --release -p synapse-server` (and `-p synapse-relay`), copy the binary to the host, restart the process. Expose remotely with `--tls` (real or self-signed cert), `--tunnel` (Cloudflare), or a self-hosted `synapse-relay` — all documented in `README.md`.
- **Web UI change reaching users:** because the bundle is compiled in, a page change ships only by **rebuilding + redeploying** the binary that serves it (`synapse-web`, and/or rebuilding+reinstalling the iOS app via `mobile/build-*.sh` on macOS). Updating `crates/app/web/**` alone never reaches an already-deployed binary.
- **Desktop/iOS app:** rebuild the relevant artifact (desktop `synapse-app`, or the iOS `.app`/staticlib) and reinstall on the device — there is no auto-update channel.

### What carries over to a new session (and what does NOT)

A fresh cloud-agent session does **not** know the previous session's running server or its `trycloudflare.com` URL — quick-tunnel hostnames are random per run and die with the tunnel/VM. Only three things persist:

1. **Committed repo files, once merged to the base branch.** This AGENTS.md reaches future sessions only after merge — a new session checks out base, not an open PR branch. So durable knowledge (CI/CD, run steps) must live here and be merged, not kept as a one-off address.
2. **Secrets** (right-hand panel) — injected as env vars every session. Put the DeepSeek key (and any tunnel token / fixed hostname) here.
3. **VM snapshot** — installed `cloudflared`, `~/.npm-global/bin/claude`, `~/.claude/settings.json`, `rustup default stable`.

For a **stable address** across sessions, don't rely on quick tunnels: use a Cloudflare *named* tunnel (fixed hostname) or a `synapse-relay` deployment, store its token/hostname as a Secret, and record the fixed hostname here.

### Per-session bring-up runbook

After the startup update script has run, any session can reproduce the full stack:

```sh
cargo build                                   # or --release
./target/debug/synapse-server --port 4173 --token CODE \
    --cwd /tmp/synapse-demo --bin ~/.npm-global/bin/claude   # DeepSeek via env/settings.json
./target/debug/synapse-web --port 8765
# local:  http://127.0.0.1:8765/?host=127.0.0.1&port=4173&token=CODE
# phone:  cloudflared tunnel --url http://127.0.0.1:4173  AND  --url http://127.0.0.1:8765
#         then https://<web-host>/?host=<server-host>&port=443&token=CODE&tls=1
```

### Auto pull + redeploy on this cloud VM

Run the watcher (pulls `origin/master` every 30s, rebuilds/restarts on new commits):

```sh
chmod +x scripts/cloud-dev-watch.sh
./scripts/cloud-dev-watch.sh              # deploy now + watch forever
./scripts/cloud-dev-watch.sh --once       # deploy once only
```

What it does each deploy:
- `cargo build -p synapse-server` (and `-p synapse-app` if app paths changed)
- restarts `synapse-server` on `:4173`
- serves **web from disk** on `:8770` (`python3 -m http.server crates/app/web`) so UI file changes from `git pull` show on browser refresh without rebuilding `synapse-web`
- restarts two `cloudflared` quick tunnels and writes the phone URL to `/tmp/synapse-public-url.txt`

Logs: `/tmp/synapse-deploy.log`. Env overrides: `SYNAPSE_GIT_BRANCH`, `SYNAPSE_POLL_SECS`, `SYNAPSE_TOKEN`, `CLAUDE_BIN`.

Keep it running in tmux: `tmux new -d -s synapse-watch './scripts/cloud-dev-watch.sh'`.

### Fixed domain on your own VPS (RackNerd + DuckDNS, no Cloudflare)

For a **stable** phone URL, deploy the full stack on a VPS with a domain (e.g. `zx0623.duckdns.org`). One domain serves web + WebSocket via Caddy (`deploy/docker-compose.yml`).

On the VPS (SSH as root, after `git clone`):

```sh
export DEEPSEEK_API_KEY='sk-...'
export SYNAPSE_DOMAIN='zx0623.duckdns.org'
bash scripts/bootstrap-vps.sh
```

Phone URL (fixed): `https://<domain>/?host=<domain>&port=443&token=CODE&tls=1`. Open ports 80/443 on the VPS firewall. The Cursor cloud VM may not be able to SSH into your VPS (fail2ban / password); run the bootstrap **on the VPS** or add the VM's SSH public key to `/root/.ssh/authorized_keys`.
