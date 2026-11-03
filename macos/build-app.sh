#!/bin/bash
# Builds "Zyntra Launcher.app" and dist/ZyntraLauncher-<version>-macos-arm64.dmg
# (on GitHub this runs in .github/workflows/release.yml).
# Run on a Mac (Apple Silicon) with Rust installed:  bash macos/build-app.sh
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
TARGET=aarch64-apple-darwin
APP="dist/Zyntra Launcher.app"
DMG="dist/ZyntraLauncher-$VERSION-macos-arm64.dmg"

rustup target add "$TARGET" >/dev/null
MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --target "$TARGET"

rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$TARGET/release/zyntra" "$APP/Contents/MacOS/zyntra"
sed "s/VERSION/$VERSION/g" macos/Info.plist > "$APP/Contents/Info.plist"

# Icon: the same "Z" mark as on Windows; macos/icon.png already has the transparent margin
# macOS icons have.
ICONSET=$(mktemp -d)/zyntra.iconset && mkdir -p "$ICONSET"
BASE=macos/icon.png
for n in 16 32 128 256 512; do
    sips -z $n $n "$BASE" --out "$ICONSET/icon_${n}x${n}.png" >/dev/null
    sips -z $((n * 2)) $((n * 2)) "$BASE" --out "$ICONSET/icon_${n}x${n}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/zyntra.icns"

# Ad-hoc signature: Apple Silicon only runs signed code (no paid Apple ID needed for this).
codesign --force --deep --sign - "$APP"

# Disk image with a shortcut to Applications to drag the app onto.
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "Zyntra Launcher" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
echo "Готово: $DMG"
