#!/bin/bash
set -euo pipefail

cargo build --release
cargo bundle --bin ferrovela-ui --release

BUNDLE="target/release/bundle/osx/FerroVela.app/Contents/MacOS"
cp target/release/ferrovela "$BUNDLE/"

echo "Bundle ready: target/release/bundle/osx/FerroVela.app"
