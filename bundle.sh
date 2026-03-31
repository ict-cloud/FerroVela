#!/bin/bash
set -euo pipefail

cargo build --release -p ferrovela -p ferrovela-ui
cargo bundle -p ferrovela-ui --release

BUNDLE="target/release/bundle/osx/FerroVela.app/Contents/MacOS"
cp target/release/ferrovela "$BUNDLE/"

echo "Bundle ready: target/release/bundle/osx/FerroVela.app"
