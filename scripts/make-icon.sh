#!/bin/sh
# Makes assets/AppIcon.icns and the screen savers' thumbnails from snapshots
# of the show. The outputs are committed; run this again only to change them.
set -eu
cd "$(dirname "$0")/.."
cargo build --release -p electropaint
bin=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release/electropaint
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The icon: seed 2 after ten seconds, a chrome ring in ep's colours.
"$bin" --snapshot "$work/render.png" --style remastered --strands 1 --size 1600x1600 --seed 2 --frames 600
swift scripts/icon.swift "$work/render.png" "$work/icon.png"
set_dir="$work/AppIcon.iconset"
mkdir "$set_dir"
for size in 16 32 128 256 512; do
	sips -z $size $size "$work/icon.png" --out "$set_dir/icon_${size}x${size}.png" >/dev/null
	double=$((size * 2))
	sips -z $double $double "$work/icon.png" --out "$set_dir/icon_${size}x${size}@2x.png" >/dev/null
done
mkdir -p assets
iconutil -c icns "$set_dir" -o assets/AppIcon.icns

# The savers' thumbnails in System Settings (90x58 points), each in its style.
for style in classic remastered; do
	"$bin" --snapshot "$work/$style.png" --style "$style" --strands 1 --size 360x232 --seed 2 --frames 600
	sips -z 116 180 "$work/$style.png" --out "assets/thumbnail-$style@2x.png" >/dev/null
	sips -z 58 90 "$work/$style.png" --out "assets/thumbnail-$style.png" >/dev/null
done
echo "made assets/AppIcon.icns and assets/thumbnail-*.png"
