#!/usr/bin/env bash

# Build MP3deck.app. Pass --install to copy it into /Applications.

set -euo pipefail
cd "$(dirname "$0")"

cargo build --release

APP="target/MP3deck.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/mp3deck "$APP/Contents/MacOS/MP3deck"
cp LICENSE THIRD_PARTY_LICENSES "$APP/Contents/Resources/"

# Icon: 1024 px PNG -> .icns
ICONSET="target/MP3deck.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z $size $size assets/icon.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) assets/icon.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/MP3deck.icns"
rm -rf "$ICONSET"

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>MP3deck</string>
  <key>CFBundleDisplayName</key><string>MP3deck</string>
  <key>CFBundleIdentifier</key><string>local.mp3deck</string>
  <key>CFBundleExecutable</key><string>MP3deck</string>
  <key>CFBundleIconFile</key><string>MP3deck</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature. The app is not notarized, so Gatekeeper blocks the first
# launch of a downloaded copy until the user allows it.
codesign --force --deep --sign - "$APP"

echo "Built $APP"

if [[ "${1:-}" == "--install" ]]; then
  rm -rf "/Applications/MP3deck.app"
  cp -R "$APP" /Applications/
  echo "Installed to /Applications/MP3deck.app"
fi
