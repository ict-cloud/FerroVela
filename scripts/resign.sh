#!/usr/bin/env bash
# Re-codesign a FerroVela .app bundle with the current user's Developer ID.
# Usage: ./scripts/resign.sh /path/to/FerroVela.app

set -euo pipefail

APP="${1:?Usage: $0 <path/to/App.app>}"
ENTITLEMENTS="$(dirname "$0")/../crates/ferrovela-ui/entitlements.plist"

if [[ ! -d "$APP" ]]; then
    echo "error: '$APP' is not a directory" >&2
    exit 1
fi

if [[ ! -f "$ENTITLEMENTS" ]]; then
    echo "error: entitlements not found at $ENTITLEMENTS" >&2
    exit 1
fi

# Find the first Developer ID Application identity in the current user's keychain.
IDENTITY=$(security find-identity -v -p codesigning \
    | grep "Developer ID Application" \
    | head -1 \
    | sed -E 's/.*"(.+)"/\1/')

if [[ -z "$IDENTITY" ]]; then
    echo "error: no 'Developer ID Application' certificate found in keychain" >&2
    echo "       Install one at developer.apple.com or via Xcode → Settings → Accounts" >&2
    exit 1
fi

echo "Identity : $IDENTITY"
echo "App      : $APP"
echo "Entitlements: $ENTITLEMENTS"
echo ""

# Sign nested bundles and frameworks first, then the outer bundle.
codesign --force --deep --options runtime \
    --entitlements "$ENTITLEMENTS" \
    --sign "$IDENTITY" \
    "$APP"

echo ""
echo "Verifying..."
codesign --verify --deep --strict --verbose=2 "$APP"
spctl --assess --type execute --verbose "$APP" 2>&1 || \
    echo "note: Gatekeeper assessment failed — app may need notarization for distribution outside direct download"

echo ""
echo "Done. Bundle identifier: $(codesign -d --verbose=2 "$APP" 2>&1 | grep Identifier | head -1)"
