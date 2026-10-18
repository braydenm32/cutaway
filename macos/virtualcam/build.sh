#!/bin/bash
# Adds the Cutaway Virtual Camera to Cutaway.app and signs everything.
#
#   ./macos/virtualcam/build.sh "Developer ID Application: Your Name (TEAMID)" TEAMID path/to/profile.provisionprofile
#
# macOS only loads camera extensions from apps signed with a paid Apple
# Developer account, with a provisioning profile that allows the
# "System Extension" capability, and installed in /Applications.
# Run ./macos/bundle.sh first.
set -euo pipefail
cd "$(dirname "$0")/../.."

identity="${1:?signing identity, e.g. \"Developer ID Application: Your Name (TEAMID)\"}"
team="${2:?team ID (10 characters)}"
profile="${3:?provisioning profile for app.cutaway.Cutaway with System Extension}"
app=target/Cutaway.app
[[ -d "$app" ]] || { echo "Run ./macos/bundle.sh first" >&2; exit 1; }
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
work=target/virtualcam
rm -rf "$work" && mkdir -p "$work"

ext="$app/Contents/Library/SystemExtensions/app.cutaway.Cutaway.Camera.systemextension"
rm -rf "$ext" && mkdir -p "$ext/Contents/MacOS"
swiftc -O -target arm64-apple-macos13.0 -o "$ext/Contents/MacOS/app.cutaway.Cutaway.Camera" macos/virtualcam/CameraExtension.swift
sed -e "s/__VERSION__/$version/g" -e "s/__TEAM__/$team/g" macos/virtualcam/Info.plist > "$ext/Contents/Info.plist"
swiftc -O -target arm64-apple-macos13.0 -o "$app/Contents/MacOS/cutaway-camera" macos/virtualcam/Installer.swift

sed "s/__TEAM__/$team/g" macos/virtualcam/Extension.entitlements > "$work/ext.entitlements"
sed "s/__TEAM__/$team/g" macos/virtualcam/App.entitlements > "$work/app.entitlements"
cp "$profile" "$app/Contents/embedded.provisionprofile"

codesign --force --options runtime --sign "$identity" --entitlements "$work/ext.entitlements" "$ext"
codesign --force --options runtime --sign "$identity" --entitlements "$work/app.entitlements" "$app/Contents/MacOS/cutaway-camera"
codesign --force --options runtime --sign "$identity" --entitlements "$work/app.entitlements" "$app"
codesign --verify --deep --strict "$app"
echo "Signed $app with the virtual camera. Copy it to /Applications, open it, and choose"
echo "Settings → Screens & webcam → Install…"
