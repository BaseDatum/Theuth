#!/usr/bin/env bash
# Render every theuth logo raster from the masters in assets/brand (T0 "Theta moon": a Θ whose ring is a crescent
# moon and whose bar is a reed pen; ring soot #1a1612 / bone #e8dcc8, bar carnelian #8f2f1f in both themes;
# wordmark = Copperplate, outlined). Needs inkscape + ImageMagick + iconutil (macOS). Run from anywhere:
#   scripts/render-brand.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BRAND="$ROOT/assets/brand"
ICONS="$ROOT/crates/desktop/icons"
IOS="$ROOT/crates/desktop/gen/apple/Assets.xcassets/AppIcon.appiconset"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
r() { inkscape "$1" -w "$3" -h "$3" -o "$2" >/dev/null 2>&1; }

echo "== desktop (macOS / Windows / Linux): app-icon-dark"
cp "$BRAND/app-icon-dark.svg" "$ICONS/icon.svg"
r "$BRAND/app-icon-dark.svg" "$ICONS/icon.png" 512
r "$BRAND/app-icon-dark.svg" "$ICONS/32x32.png" 32
r "$BRAND/app-icon-dark.svg" "$ICONS/64x64.png" 64
r "$BRAND/app-icon-dark.svg" "$ICONS/128x128.png" 128
r "$BRAND/app-icon-dark.svg" "$ICONS/128x128@2x.png" 256
SET="$TMP/icon.iconset"; mkdir -p "$SET"
for s in 16 32 128 256 512; do
  r "$BRAND/app-icon-dark.svg" "$SET/icon_${s}x${s}.png" $s
  r "$BRAND/app-icon-dark.svg" "$SET/icon_${s}x${s}@2x.png" $((s*2))
done
iconutil -c icns "$SET" -o "$ICONS/icon.icns"
for s in 16 24 32 48 64 256; do r "$BRAND/app-icon-dark.svg" "$TMP/ico-$s.png" $s; done
magick "$TMP"/ico-16.png "$TMP"/ico-24.png "$TMP"/ico-32.png "$TMP"/ico-48.png "$TMP"/ico-64.png "$TMP"/ico-256.png "$ICONS/icon.ico"

echo "== tray / menu bar (template, monochrome small cut)"
cp "$BRAND/theta-mark-mono.svg" "$ICONS/tray.svg"
r "$BRAND/theta-mark-mono.svg" "$ICONS/tray.png" 22
r "$BRAND/theta-mark-mono.svg" "$ICONS/tray@2x.png" 44
r "$BRAND/tray-windows.svg" "$ICONS/tray-windows.png" 32

echo "== iOS / iPadOS AppIcon: single-size (Xcode 14+) light (papyrus), dark (ink), tinted; opaque, no alpha"
# Dark and tinted appearances only take effect in the single-size format (one 1024 px universal image per
# appearance); a per-size legacy set with "appearances" compiles but iOS always shows the light icon.
find "$IOS" -name '*.png' -delete
for v in light dark tinted; do
  r "$BRAND/ios-icon-$v.svg" "$TMP/ios-$v.png" 1024
  magick "$TMP/ios-$v.png" -background black -alpha remove -alpha off "PNG24:$IOS/AppIcon-$v.png"
done
cat > "$IOS/Contents.json" <<'EOF'
{
  "images" : [
    {
      "filename" : "AppIcon-light.png",
      "idiom" : "universal",
      "platform" : "ios",
      "size" : "1024x1024"
    },
    {
      "appearances" : [ { "appearance" : "luminosity", "value" : "dark" } ],
      "filename" : "AppIcon-dark.png",
      "idiom" : "universal",
      "platform" : "ios",
      "size" : "1024x1024"
    },
    {
      "appearances" : [ { "appearance" : "luminosity", "value" : "tinted" } ],
      "filename" : "AppIcon-tinted.png",
      "idiom" : "universal",
      "platform" : "ios",
      "size" : "1024x1024"
    }
  ],
  "info" : { "author" : "xcode", "version" : 1 }
}
EOF

echo "== companion harness glyph (white, like the others)"
sed 's/fill="#000"/fill="#fff"/g' "$BRAND/theta-mark-mono.svg" > "$ROOT/crates/desktop/companion-assets/harness/theuth.svg"
for s in 16 20 24 32; do r "$ROOT/crates/desktop/companion-assets/harness/theuth.svg" "$ROOT/crates/desktop/companion-assets/harness/theuth-$s.png" $s; done

echo "== website"
r "$BRAND/ios-icon-dark.svg" "$TMP/ati.png" 256
magick "$TMP/ati.png" -alpha off "PNG24:$ROOT/site/public/apple-touch-icon.png"

echo "== github: repository social preview (1280x640, upload to Settings > Social preview)"
# Dark plate on soot, bone lockup, tagline; GitHub wants a 1280x640 PNG under 1 MB and has no API for it, so this
# file is uploaded by hand (repo Settings > Social preview) and kept here so the upload is reproducible from the
# masters. The organisation avatar on GitHub is this same dark plate: app-icon-dark rendered at 1024.
r "$BRAND/app-icon-dark.svg" "$TMP/gh-plate.png" 288
sed 's/fill="currentColor"/fill="#e8dcc8"/' "$BRAND/wordmark.svg" > "$TMP/gh-word.svg"
inkscape "$TMP/gh-word.svg" -w 520 -o "$TMP/gh-word.png" >/dev/null 2>&1
magick -size 1280x640 xc:'#1a1612' \
  "$TMP/gh-plate.png" -geometry +104+176 -composite \
  "$TMP/gh-word.png" -geometry +468+236 -composite \
  -font /System/Library/Fonts/Supplemental/Copperplate.ttc -fill '#b9ab95' -pointsize 27 -kerning 1.2 \
  -annotate +470+352 'Your agents, your machines, your keys.' \
  -fill '#8f2f1f' -draw 'rectangle 470,392 610,395' \
  -fill '#6b6357' -font /System/Library/Fonts/Helvetica.ttc -pointsize 22 -kerning 2 \
  -annotate +470+436 'THEUTH.IO' \
  -alpha off -colorspace sRGB "PNG24:$BRAND/github-social.png"

echo "done"
