#!/bin/sh
# Builds the two screen savers (Apple Silicon) into target/saver/:
# electropaint.saver, as ep drew it, and "electropaint Remastered.saver".
# Install by double-clicking them, or:
#   cp -R target/saver/*.saver ~/Library/Screen\ Savers/
set -eu
cd "$(dirname "$0")/.."
version=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"name":"ep-saver","version":"\([^"]*\)".*/\1/p')

# saver NAME EXECUTABLE BUNDLE-ID PRINCIPAL-CLASS [CARGO-FEATURES]
saver() {
	name=$1 exe=$2 id=$3 class=$4 features=${5:-}
	cargo build --release -p ep-saver ${features:+--features "$features"}
	bundle="target/saver/$name.saver"
	rm -rf "$bundle"
	mkdir -p "$bundle/Contents/MacOS"
	cp target/release/libep_saver.dylib "$bundle/Contents/MacOS/$exe"
	# Each its own identity, as both can be loaded into one process.
	install_name_tool -id "@loader_path/$exe" "$bundle/Contents/MacOS/$exe"
	cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key><string>en</string>
	<key>CFBundleExecutable</key><string>$exe</string>
	<key>CFBundleIdentifier</key><string>$id</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key><string>$name</string>
	<key>CFBundlePackageType</key><string>BNDL</string>
	<key>CFBundleShortVersionString</key><string>${version:-0.1.0}</string>
	<key>CFBundleVersion</key><string>${version:-0.1.0}</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSPrincipalClass</key><string>$class</string>
</dict>
</plist>
PLIST
	codesign --force --sign - "$bundle"
	echo "built $bundle"
}

saver electropaint electropaint local.electropaint.saver EPSaverView
saver "electropaint Remastered" electropaint-remastered local.electropaint.remastered.saver EPRemasteredSaverView remastered
