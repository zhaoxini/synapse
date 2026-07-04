#!/usr/bin/env bash
# Build + install + launch Synapse on a real iPhone (Debug, wireless).
#
# Companion to build-sim.sh (simulator) and build-ios.sh (Release). This is the
# Rust-change loop: after editing crates/app (Rust) you must rebuild + reinstall.
# WEB-only edits (crates/app/web) do NOT need this — Debug builds load web/ from
# your Mac over the LAN (see SYNAPSE_DEV_* in mobile/ios/Sources/main.m), so just
# refresh the page on the phone. Keep the Mac side running in two terminals:
#   (cd crates/app/web && python3 -m http.server 8000)   # serves web/ from disk
#   cargo run -p synapse-server -- --token CODE           # backend, binds 0.0.0.0
#
# ONE-TIME setup before this works (devicectl must see the phone):
#   1. iPhone: Settings > Privacy & Security > Developer Mode = ON (reboots).
#   2. Plug in via USB once, tap "Trust" on the phone.
#   3. Xcode > Window > Devices and Simulators > select the phone >
#      check "Connect via network". After that this runs cable-free on Wi-Fi.
#   4. Verify it's visible:  xcrun devicectl list devices
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_ID="com.synapse.app.gnjza"
TEAM="${DEVELOPMENT_TEAM:-GNJZA74ZW4}"

# Pick the device: $DEVICE env (its devicectl Identifier) or the first one seen.
DEVICE="${DEVICE:-$(xcrun devicectl list devices 2>/dev/null \
  | grep -Eio '[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}' | head -1)}"
if [[ -z "$DEVICE" ]]; then
  echo "error: no real device found via devicectl. Do the ONE-TIME setup in this" >&2
  echo "       script's header, then re-run. (xcrun devicectl list devices)" >&2
  exit 1
fi
echo "==> Target device: $DEVICE"

# Build the device arm64 slice. 'generic/platform=iOS' avoids needing the device
# present at build time; signing is required to install on a real phone. The
# Xcode "Build Rust staticlib" phase compiles the aarch64-apple-ios lib per
# PLATFORM_NAME. Debug config => main.m's dev branch (load web/backend from Mac).
echo "==> Building Synapse.app (Debug, iphoneos, arm64), team $TEAM"
cd "$ROOT/mobile/ios"
xcodebuild \
  -project Synapse.xcodeproj \
  -scheme Synapse \
  -configuration Debug \
  -sdk iphoneos \
  -destination 'generic/platform=iOS' \
  -derivedDataPath build_device \
  DEVELOPMENT_TEAM="$TEAM" CODE_SIGN_IDENTITY="Apple Development" \
  -allowProvisioningUpdates \
  build

APP="build_device/Build/Products/Debug-iphoneos/Synapse.app"
[[ -d "$APP" ]] || { echo "error: $APP not found after build" >&2; exit 1; }

echo "==> Installing on device $DEVICE"
xcrun devicectl device install app --device "$DEVICE" "$APP"
echo "==> Launching $BUNDLE_ID"
xcrun devicectl device process launch --device "$DEVICE" "$BUNDLE_ID" || true
echo "==> Done."
