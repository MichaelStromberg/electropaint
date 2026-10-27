#!/bin/sh
# Builds target/dmg/electropaint-VERSION.dmg (Apple Silicon): electropaint.app,
# both screen savers and a Read Me, with a checksum beside it. The bundles are
# signed ad hoc only, so macOS asks before opening them the first time; the
# Read Me says how.
set -eu
cd "$(dirname "$0")/.."
meta=$(cargo metadata --format-version 1 --no-deps)
release=$(printf '%s' "$meta" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release
version=$(cargo pkgid -p electropaint)
version=${version##*[#@]}
[ -n "$version" ] && [ "$release" != /release ] || { echo "make-dmg: cargo metadata or pkgid failed" >&2; exit 1; }

out=target/dmg
stage=$out/stage
rm -rf "$stage"
mkdir -p "$stage"

# The app.
cargo build --release -p electropaint
app="$stage/electropaint.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$release/electropaint" "$app/Contents/MacOS/electropaint"
cp assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key><string>en</string>
	<key>CFBundleDisplayName</key><string>electropaint</string>
	<key>CFBundleExecutable</key><string>electropaint</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundleIdentifier</key><string>io.github.michaelstromberg.electropaint</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>CFBundleName</key><string>electropaint</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$version</string>
	<key>CFBundleVersion</key><string>$version</string>
	<key>LSApplicationCategoryType</key><string>public.app-category.entertainment</string>
	<key>LSMinimumSystemVersion</key><string>14.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>NSHumanReadableCopyright</key><string>Copyright © 2026 Michael Strömberg. MIT licence.</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$app"

# The screen savers.
sh scripts/make-saver.sh
mkdir -p "$stage/Screen Savers"
cp -R target/saver/*.saver "$stage/Screen Savers/"

ln -s /Applications "$stage/Applications"
cat > "$stage/Read Me.txt" <<TEXT
electropaint $version
https://github.com/MichaelStromberg/electropaint

SGI's electropaint demo from IRIX, ported to macOS. Apple silicon, macOS 14
or later.

THE APP
Drag electropaint to Applications. Opened from Finder it runs the remastered
show fullscreen with its control panel; Esc or Command-Q quits. From Terminal,
/Applications/electropaint.app/Contents/MacOS/electropaint --help lists the
options, including the classic look.

THE SCREEN SAVERS
Double-click electropaint.saver (as ep drew it) or electropaint
Remastered.saver in the Screen Savers folder, and System Settings installs it
for you. Choose it in System Settings > Screen Saver.

THE FIRST TIME
These downloads aren't signed by an Apple developer account, so macOS stops
them the first time with "Apple could not verify ...". To allow one, try to
open it once, then go to System Settings > Privacy & Security and click Open
Anyway. Or, in Terminal, after copying them:

    xattr -dr com.apple.quarantine /Applications/electropaint.app
    xattr -dr com.apple.quarantine ~/Library/Screen\ Savers/electropaint*.saver
TEXT

dmg="$out/electropaint-$version.dmg"
rm -f "$dmg"
hdiutil create -quiet -volname "electropaint $version" -srcfolder "$stage" -format UDZO -ov "$dmg"
(cd "$out" && shasum -a 256 "electropaint-$version.dmg" > "electropaint-$version.dmg.sha256")
echo "built $dmg"
cat "$dmg.sha256"
