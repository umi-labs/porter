#!/bin/sh
# Builds an unsigned Porter.app into target/Porter.app (or $CARGO_TARGET_DIR; release by default; PROFILE=debug
# for a faster, larger bundle). Run from anywhere.
set -eu
root=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$root"
profile=${PROFILE:-release}
target=${CARGO_TARGET_DIR:-target}
if [ "$profile" = release ]; then
  cargo build --release -p porter-desktop
else
  cargo build -p porter-desktop
fi
version=$(cargo pkgid -p porter-desktop | sed 's/.*[#@]//')
app=$target/Porter.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$target/$profile/porter-desktop" "$app/Contents/MacOS/porter-desktop"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>uk.co.umidigital.porter</string>
  <key>CFBundleName</key><string>Porter</string>
  <key>CFBundleDisplayName</key><string>Porter</string>
  <key>CFBundleExecutable</key><string>porter-desktop</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist"
echo "built $app (unsigned, $profile, v$version)"
