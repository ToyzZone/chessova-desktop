#!/usr/bin/env bash
#
# Build a distributable macOS DMG of Chessova Desktop (Apple Silicon).
#
# Output: dist/Chessova-Desktop-<VERSION>-arm64.dmg
#
# What this does:
#   1. Compiles the helper for aarch64-apple-darwin (Apple Silicon)
#   2. Bundles the Homebrew Stockfish binary inside the .app
#   3. Builds a minimal .app bundle (LSBackgroundOnly — no Dock icon)
#   4. Ad-hoc signs (no Apple Developer cert assumed); set APPLE_DEV_ID to
#      enable Developer ID signing + later notarization
#   5. Creates the DMG via hdiutil
#
# Intel Macs are not supported by this script (vanishingly rare for the
# target audience). Intel users can fall back to the server engine path
# on /review, or build from source with `cargo run --release`.
#
# Prereqs (run once):
#   rustup target add aarch64-apple-darwin
#   brew install stockfish        # picked up from /opt/homebrew/bin/stockfish
#
# Optional env:
#   APPLE_DEV_ID="Developer ID Application: Your Name (TEAMID)"
#     If set, sign with this identity. Otherwise ad-hoc sign (-).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
APP_NAME="Chessova Desktop"
BUNDLE_ID="com.chessova.desktop"
EXE_NAME="chessova-desktop"

DIST_DIR="$PROJECT_DIR/dist"
APP_DIR="$DIST_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
DMG_NAME="Chessova-Desktop-$VERSION-arm64.dmg"

echo "→ Building $APP_NAME $VERSION (arm64)"
rm -rf "$DIST_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# 1. Compile arm64
echo "→ Compiling aarch64-apple-darwin"
cargo build --release --target aarch64-apple-darwin

TARGET_ARM="target/aarch64-apple-darwin/release/$EXE_NAME"
OUT_BIN="$MACOS_DIR/$EXE_NAME"
cp "$TARGET_ARM" "$OUT_BIN"
chmod +x "$OUT_BIN"

# 3. Bundle Stockfish next to the helper. config.rs auto-resolves it
#    from the same directory as the running executable.
STOCKFISH_SRC="${STOCKFISH_SRC:-/opt/homebrew/bin/stockfish}"
if [[ ! -f "$STOCKFISH_SRC" ]]; then
  STOCKFISH_SRC="/usr/local/bin/stockfish"
fi
if [[ ! -f "$STOCKFISH_SRC" ]]; then
  echo "✗ Stockfish not found. Install with: brew install stockfish" >&2
  exit 1
fi
echo "→ Bundling Stockfish from $STOCKFISH_SRC"
cp "$STOCKFISH_SRC" "$MACOS_DIR/stockfish"
chmod +x "$MACOS_DIR/stockfish"

# 4. Info.plist — LSBackgroundOnly so the app runs headless without
#    grabbing a Dock icon. CFBundleExecutable points at our helper.
cat > "$CONTENTS_DIR/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>$EXE_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSBackgroundOnly</key>
  <true/>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>© Chessova</string>
</dict>
</plist>
EOF

# 5. Code signing. With APPLE_DEV_ID set, sign with that identity (needed
#    for notarization). Otherwise ad-hoc sign — works locally and lets
#    macOS recognize the bundle, but users get a Gatekeeper warning on
#    first open and must right-click → Open.
SIGN_ID="${APPLE_DEV_ID:--}"  # - means ad-hoc
echo "→ Code signing with identity: $SIGN_ID"
codesign --force --sign "$SIGN_ID" --timestamp=none --options=runtime \
  "$MACOS_DIR/stockfish"
codesign --force --sign "$SIGN_ID" --timestamp=none --options=runtime \
  "$OUT_BIN"
codesign --force --sign "$SIGN_ID" --timestamp=none --options=runtime \
  --deep "$APP_DIR"
codesign --verify --verbose "$APP_DIR" >/dev/null

# 6. Build the DMG
echo "→ Building $DMG_NAME"
DMG_STAGING="$DIST_DIR/dmg-staging"
mkdir -p "$DMG_STAGING"
cp -R "$APP_DIR" "$DMG_STAGING/"
ln -s /Applications "$DMG_STAGING/Applications"

hdiutil create \
  -volname "$APP_NAME" \
  -srcfolder "$DMG_STAGING" \
  -ov \
  -format UDZO \
  "$DIST_DIR/$DMG_NAME" >/dev/null

rm -rf "$DMG_STAGING"

echo ""
echo "✓ Built $DIST_DIR/$DMG_NAME"
echo "  Size: $(du -h "$DIST_DIR/$DMG_NAME" | cut -f1)"
if [[ "$SIGN_ID" == "-" ]]; then
  echo ""
  echo "  ⚠ Ad-hoc signed. First-time users must right-click → Open."
  echo "    For a clean install set APPLE_DEV_ID and notarize:"
  echo "      xcrun notarytool submit '$DIST_DIR/$DMG_NAME' \\"
  echo "        --apple-id you@example.com --team-id TEAMID --keychain-profile NAME --wait"
fi
