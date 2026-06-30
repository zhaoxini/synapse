#!/usr/bin/env bash
# Build + install + launch Synapse on the booted iOS simulator.
#
# Companion to build-ios.sh (which targets a physical device). The web bundle
# (crates/app/web) is compiled into the Rust staticlib via include_str!, so this
# is required after ANY crates/app change to see it on the sim. The Xcode
# "Build Rust staticlib" phase compiles the aarch64-apple-ios-sim lib itself.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_ID="com.synapse.app.gnjza"

# Target the booted simulator by udid. A concrete device pins the arch to the
# sim's (arm64 on Apple Silicon); a 'generic' destination drags in x86_64, which
# the arm64-only Rust staticlib can't satisfy (link fails).
UDID="$(xcrun simctl list devices booted | grep -Eo '[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}' | head -1)"
[[ -n "$UDID" ]] || { echo "error: no booted simulator (boot one in Simulator.app first)" >&2; exit 1; }
echo "==> Building Synapse.app (Debug, iphonesimulator, arm64) for sim $UDID"
cd "$ROOT/mobile/ios"
xcodebuild \
  -project Synapse.xcodeproj \
  -scheme Synapse \
  -configuration Debug \
  -sdk iphonesimulator \
  -destination "platform=iOS Simulator,id=$UDID" \
  -derivedDataPath build_sim \
  ARCHS=arm64 ONLY_ACTIVE_ARCH=YES \
  build

APP="build_sim/Build/Products/Debug-iphonesimulator/Synapse.app"
[[ -d "$APP" ]] || { echo "error: $APP not found after build" >&2; exit 1; }

echo "==> Installing + launching on the booted simulator"
xcrun simctl install booted "$APP"
xcrun simctl launch booted "$BUNDLE_ID" || true
echo "==> Done."
