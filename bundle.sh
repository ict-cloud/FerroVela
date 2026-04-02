#!/bin/bash
set -euo pipefail

cargo build --release -p ferrovela -p ferrovela-ui
cargo bundle -p ferrovela-ui --release

APP="target/release/bundle/osx/FerroVela.app"
BUNDLE="$APP/Contents/MacOS"
ENTITLEMENTS="crates/ferrovela-ui/entitlements.plist"

cp target/release/ferrovela "$BUNDLE/"

# Codesign both binaries with network entitlements so launchd / TCC
# allows the proxy to bind and connect.
codesign --force --sign - --entitlements "$ENTITLEMENTS" "$BUNDLE/ferrovela"
codesign --force --sign - --entitlements "$ENTITLEMENTS" "$BUNDLE/ferrovela-ui"
# Re-sign the outer bundle to pick up the new signatures.
codesign --force --sign - --entitlements "$ENTITLEMENTS" "$APP"

echo "Bundle ready: $APP"
