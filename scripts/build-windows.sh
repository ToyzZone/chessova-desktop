#!/usr/bin/env bash
#
# Cross-build a distributable Windows package of Chessova Desktop from macOS.
#
# Output: dist/Chessova-Desktop-<VERSION>-windows-x64.zip
#
# What this does:
#   1. Cross-compiles the helper for x86_64-pc-windows-gnu via MinGW
#   2. Downloads (and caches) official Stockfish 17 Windows binary
#   3. Stages chessova-desktop.exe + stockfish.exe + start.bat + README.txt
#   4. Zips into a single distributable archive
#
# We pick the `sse41-popcnt` Stockfish variant — it works on every Intel/AMD
# CPU from ~2009 onward. AVX2 would be ~10% faster but breaks pre-2013 chips.
# Compatibility wins for a first release.
#
# Prereqs (run once):
#   rustup target add x86_64-pc-windows-gnu
#   brew install mingw-w64
#
# No code signing here. Windows SmartScreen will warn on first launch
# (more aggressive than macOS Gatekeeper). For a clean install we'd need
# a Windows code-signing cert (~$300/yr EV from Sectigo/DigiCert).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
EXE_NAME="chessova-desktop"
TARGET="x86_64-pc-windows-gnu"

DIST_DIR="$PROJECT_DIR/dist"
STAGE_NAME="Chessova-Desktop-$VERSION-windows-x64"
STAGE_DIR="$DIST_DIR/$STAGE_NAME"
ZIP_NAME="$STAGE_NAME.zip"

CACHE_DIR="$PROJECT_DIR/.cache"
SF_VARIANT="stockfish-windows-x86-64-sse41-popcnt"
SF_VERSION="sf_18"
SF_URL="https://github.com/official-stockfish/Stockfish/releases/download/$SF_VERSION/$SF_VARIANT.zip"
SF_ZIP="$CACHE_DIR/$SF_VARIANT.zip"

echo "→ Building Chessova Desktop $VERSION (Windows x64)"

# Prereq sanity checks — fail loud if the toolchain isn't ready.
if ! rustup target list --installed | grep -q "$TARGET"; then
  echo "✗ Missing Rust target: $TARGET" >&2
  echo "  Run: rustup target add $TARGET" >&2
  exit 1
fi
if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
  echo "✗ Missing MinGW cross-compiler" >&2
  echo "  Run: brew install mingw-w64" >&2
  exit 1
fi

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR" "$CACHE_DIR"

# 1. Cross-compile
echo "→ Compiling $TARGET"
cargo build --release --target "$TARGET"

cp "target/$TARGET/release/$EXE_NAME.exe" "$STAGE_DIR/$EXE_NAME.exe"

# 2. Stockfish — download if not cached
if [[ ! -f "$SF_ZIP" ]]; then
  echo "→ Downloading $SF_VARIANT"
  curl -fSL --retry 3 -o "$SF_ZIP.tmp" "$SF_URL"
  mv "$SF_ZIP.tmp" "$SF_ZIP"
fi

# The official SF zip contains `stockfish/<variant>.exe`. Extract into a
# temp dir, find the .exe, rename to `stockfish.exe`. config.rs's bundled
# lookup (current_exe() sibling) only cares about the literal filename.
SF_TMP="$(mktemp -d)"
trap 'rm -rf "$SF_TMP"' EXIT
unzip -q "$SF_ZIP" -d "$SF_TMP"
SF_EXE="$(find "$SF_TMP" -name '*.exe' -type f | head -1)"
if [[ -z "$SF_EXE" ]]; then
  echo "✗ No .exe found inside $SF_ZIP" >&2
  exit 1
fi
cp "$SF_EXE" "$STAGE_DIR/stockfish.exe"

# 3. start.bat — launches helper with a console window so users can see
# logs and know the helper is running. They close the window to stop it.
cat > "$STAGE_DIR/start.bat" <<'BAT'
@echo off
title Chessova Desktop
echo Starting Chessova Desktop helper on ws://127.0.0.1:9876/uci
echo Keep this window open while using chessova.com/review.
echo Close it to stop the helper.
echo.
"%~dp0chessova-desktop.exe"
BAT

# 4. README.txt — Windows users won't read the README.md in the macOS
# bundle, so duplicate the essentials here.
cat > "$STAGE_DIR/README.txt" <<EOF
Chessova Desktop $VERSION (Windows x64)
========================================

This is the local helper for chessova.com/review. While running it
exposes Stockfish over a local WebSocket so the review page can do
deeper, faster analysis than the server engine.

To run:
  1. Double-click start.bat
  2. Leave the console window open
  3. Open chessova.com/review in your browser — it auto-detects the
     helper and switches to native Stockfish for analysis

To stop:
  Close the console window.

First-launch warning:
  Windows SmartScreen may warn that this app is "unrecognized" because
  it's not code-signed. Click "More info" -> "Run anyway".

Files:
  chessova-desktop.exe   The helper (Rust, ~6 MB)
  stockfish.exe          Stockfish 18 ($SF_VARIANT)
  start.bat              Launcher
  README.txt             This file
EOF

# 5. Zip
echo "→ Building $ZIP_NAME"
(cd "$DIST_DIR" && zip -r -q "$ZIP_NAME" "$STAGE_NAME")

ZIP_SIZE="$(du -h "$DIST_DIR/$ZIP_NAME" | cut -f1)"
echo ""
echo "✓ Built $DIST_DIR/$ZIP_NAME"
echo "  Size: $ZIP_SIZE"
echo ""
echo "  ⚠ Unsigned. Users will see a SmartScreen warning on first launch."
echo "    For a clean install you'd need an EV code-signing cert."
