#!/bin/sh
# Builds the two screen savers (Apple Silicon) into target/saver/:
# electropaint.saver, as ep drew it, and "electropaint Remastered.saver".
# Install by double-clicking them, or:
#   cp -R target/saver/*.saver ~/Library/Screen\ Savers/
set -eu
cd "$(dirname "$0")/.."
# Where cargo builds (CARGO_TARGET_DIR or a config can move it from target/).
meta=$(cargo metadata --format-version 1 --no-deps)
release=$(printf '%s' "$meta" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release
version=$(cargo pkgid -p ep-saver)
version=${version##*[#@]}
[ -n "$version" ] && [ "$release" != /release ] || { echo "make-saver: cargo metadata or pkgid failed" >&2; exit 1; }

# saver NAME EXECUTABLE BUNDLE-ID PRINCIPAL-CLASS THUMBNAIL [CARGO-FEATURES]
saver() {
	name=$1 exe=$2 id=$3 class=$4 thumbnail=$5 features=${6:-}
	cargo build --release -p ep-saver ${features:+--features "$features"}
	bundle="target/saver/$name.saver"
	rm -rf "$bundle"
	mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
	# What System Settings shows for the saver.
	cp "assets/thumbnail-$thumbnail.png" "$bundle/Contents/Resources/thumbnail.png"
	cp "assets/thumbnail-$thumbnail@2x.png" "$bundle/Contents/Resources/thumbnail@2x.png"
	cp "$release/libep_saver.dylib" "$bundle/Contents/MacOS/$exe"
	# The build just made, not another (another cargo run can replace it).
	grep -aq "$class" "$bundle/Contents/MacOS/$exe" || { echo "make-saver: $exe lacks $class" >&2; exit 1; }
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
	<key>CFBundleShortVersionString</key><string>$version</string>
	<key>CFBundleVersion</key><string>$version</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSPrincipalClass</key><string>$class</string>
</dict>
</plist>
PLIST
	codesign --force --sign - "$bundle"
	echo "built $bundle"
}

saver electropaint electropaint io.github.michaelstromberg.electropaint.saver EPSaverView classic
saver "electropaint Remastered" electropaint-remastered io.github.michaelstromberg.electropaint.remastered-saver EPRemasteredSaverView remastered remastered
