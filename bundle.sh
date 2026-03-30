#!/bin/bash
set -euo pipefail

cargo build --release -p ferrovela -p ferrovela-ui -p ferrovela-app
cargo bundle -p ferrovela-app --release

BUNDLE="target/release/bundle/osx/FerroVela.app/Contents/MacOS"
cp target/release/ferrovela "$BUNDLE/"
cp target/release/ferrovela-ui "$BUNDLE/"

echo "Bundle ready: target/release/bundle/osx/FerroVela.app"
