#!/bin/bash
# Builds Cutaway.app for Apple Silicon Macs.
#
#   ./macos/bundle.sh            → target/Cutaway.app
#   ./macos/bundle.sh --install  → also copies it to /Applications
#
# Needs only Rust (https://rustup.rs) and the Xcode command line tools
# (xcode-select --install). Nothing else is installed or bundled: capture,
# encoding and compositing all use frameworks that ship with macOS.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "Run this on a Mac." >&2
    exit 1
fi

cargo build --release --target aarch64-apple-darwin
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

app=target/Cutaway.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/aarch64-apple-darwin/release/cutaway "$app/Contents/MacOS/Cutaway"
sed "s/__VERSION__/$version/g" macos/Info.plist > "$app/Contents/Info.plist"
cp macos/Cutaway.icns "$app/Contents/Resources/"
# The Stream Deck plugin (Settings → Remote & MIDI → Install…).
if [[ -f streamdeck/Cutaway.streamDeckPlugin ]]; then
    cp streamdeck/Cutaway.streamDeckPlugin "$app/Contents/Resources/"
fi

# Ad-hoc signature: enough for macOS to remember the camera, microphone and
# screen recording permissions you grant. (Distributing to other Macs
# needs a Developer ID signature and notarization instead.)
codesign --force --sign - "$app"

echo "Built $app"
if [[ "${1:-}" == "--install" ]]; then
    rm -rf /Applications/Cutaway.app
    cp -R "$app" /Applications/
    echo "Installed to /Applications/Cutaway.app"
fi
