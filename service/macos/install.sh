#!/bin/bash
set -e

# Define installation directories
INSTALL_BIN_DIR="$HOME/.local/bin"
INSTALL_CONFIG_DIR="$HOME/.config/ferrovela"
PLIST_DIR="$HOME/Library/LaunchAgents"
SERVICE_NAME="com.ferrovela"

# Check dependencies
if ! command -v cargo &> /dev/null; then
    echo "Error: cargo is not installed."
    exit 1
fi

echo "Building FerroVela..."
cargo build --release

echo "Installing binary to $INSTALL_BIN_DIR..."
mkdir -p "$INSTALL_BIN_DIR"
cp target/release/ferrovela "$INSTALL_BIN_DIR/"

echo "Installing configuration to $INSTALL_CONFIG_DIR..."
mkdir -p "$INSTALL_CONFIG_DIR"
if [ ! -f "$INSTALL_CONFIG_DIR/config.toml" ]; then
    cp config.toml "$INSTALL_CONFIG_DIR/"
    echo "Copied default config.toml to $INSTALL_CONFIG_DIR/config.toml"
else
    echo "Config file already exists at $INSTALL_CONFIG_DIR/config.toml, skipping copy."
fi

# Warn about PAC files if local path is used
if grep -q 'pac_file.*=.*"[^http]' "$INSTALL_CONFIG_DIR/config.toml"; then
    echo "Warning: Your config.toml seems to reference a local PAC file."
    echo "Please ensure the PAC file is located in $INSTALL_CONFIG_DIR/"
fi

echo "Creating launchd plist..."
mkdir -p "$PLIST_DIR"
TEMPLATE_FILE="service/macos/com.ferrovela.plist.template"
PLIST_FILE="$PLIST_DIR/$SERVICE_NAME.plist"

# Replace placeholders
sed -e "s|{{BIN_PATH}}|$INSTALL_BIN_DIR/ferrovela|g" \
    -e "s|{{CONFIG_PATH}}|$INSTALL_CONFIG_DIR/config.toml|g" \
    -e "s|{{CONFIG_DIR}}|$INSTALL_CONFIG_DIR|g" \
    "$TEMPLATE_FILE" > "$PLIST_FILE"

echo "Generated plist at $PLIST_FILE"

echo "Loading service..."
# Unload if exists to force reload
if launchctl list | grep -q "$SERVICE_NAME"; then
    launchctl unload "$PLIST_FILE" || true
fi
launchctl load "$PLIST_FILE"

echo "Service $SERVICE_NAME installed and loaded."
echo "Logs are available at /tmp/ferrovela.log and /tmp/ferrovela.err"
