# Synapse — agent notes

## After changing the app, rebuild it yourself — don't make the user run the build

The web chat bundle (`crates/app/web/`) is **compiled into the binary** (`include_str!` / `include_bytes!`), and the iOS app is the surface the user actually tests. So **any** change under `crates/app/web/**` or `crates/app/src/**` has *zero* effect until the app is rebuilt + reinstalled — editing the files alone changes nothing on the running app.

When you finish a feature / UI tweak / fix that touches the app, **rebuild and relaunch it automatically**. Do not leave a manual "now run X" step for the user.

- **iOS simulator (default test surface):** `./mobile/build-sim.sh` — builds Debug for `iphonesimulator`, installs on the booted sim, and launches it. The Xcode "Build Rust staticlib" phase recompiles the `aarch64-apple-ios-sim` lib so the new web bundle is embedded.
- **Physical device:** `./mobile/build-ios.sh` (needs `DEVELOPMENT_TEAM`).

Server-only changes (`crates/server/**`) don't need an app rebuild — rebuild + restart `synapse-server` instead.

Always verify on the real surface (the sim app), not just a browser tab.
