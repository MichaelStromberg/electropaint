#!/bin/sh
# Builds electropaint.saver (Apple Silicon) into target/saver/.
# Install by double-clicking it, or: cp -R target/saver/electropaint.saver ~/Library/Screen\ Savers/
set -eu
cd "$(dirname "$0")/.."
cargo build --release -p ep-saver
bundle=target/saver/electropaint.saver
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS"
cp target/release/libep_saver.dylib "$bundle/Contents/MacOS/electropaint"
version=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"name":"ep-saver","version":"\([^"]*\)".*/\1/p')
cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key><string>en</string>
	<key>CFBundleExecutable</key><string>electropaint</string>
	<key>CFBundleIdentifier</key><string>local.electropaint.saver</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key><string>electropaint</string>
	<key>CFBundlePackageType</key><string>BNDL</string>
	<key>CFBundleShortVersionString</key><string>${version:-0.1.0}</string>
	<key>CFBundleVersion</key><string>${version:-0.1.0}</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSPrincipalClass</key><string>EPSaverView</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$bundle"
echo "built $bundle"
