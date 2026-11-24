#!/usr/bin/env bash
# Render every theuth logo raster from the masters in assets/brand (T0 "Theta moon": a Θ whose ring is a crescent
# moon and whose bar is a reed pen; ring soot #1a1612 / bone #e8dcc8, bar ochre #b4531e in both themes;
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

echo "== iOS / iPadOS AppIcon: light (papyrus), dark (ink), tinted; opaque, no alpha"
python3 - "$IOS" <<'EOF'
import json, sys
d = sys.argv[1]
c = json.load(open(f"{d}/Contents.json"))
base = [i for i in c["images"] if "appearances" not in i]
out = []
for i in base:
    out.append(i)
    stem = i["filename"][:-4]
    for v in ("dark", "tinted"):
        j = dict(i); j["filename"] = f"{stem}-{v}.png"
        j["appearances"] = [{"appearance": "luminosity", "value": v}]
        out.append(j)
c["images"] = out
json.dump(c, open(f"{d}/Contents.json", "w"), indent=2)
open(f"{d}/Contents.json", "a").write("\n")
for i in out:
    w = float(i["size"].split("x")[0]) * float(i["scale"].rstrip("x"))
    v = next((a["value"] for a in i.get("appearances", [])), "light")
    print(f'{i["filename"]} {round(w)} {v}')
EOF
python3 - "$IOS" <<'EOF' > "$TMP/ios.txt"
import json, sys
for i in json.load(open(f"{sys.argv[1]}/Contents.json"))["images"]:
    w = float(i["size"].split("x")[0]) * float(i["scale"].rstrip("x"))
    v = next((a["value"] for a in i.get("appearances", [])), "light")
    print(i["filename"], round(w), v)
EOF
while read -r f px v; do
  r "$BRAND/ios-icon-$v.svg" "$TMP/$f" "$px"
  magick "$TMP/$f" -background black -alpha remove -alpha off "PNG24:$IOS/$f"
done < "$TMP/ios.txt"

echo "== companion harness glyph (white, like the others)"
sed 's/fill="#000"/fill="#fff"/g' "$BRAND/theta-mark-mono.svg" > "$ROOT/crates/desktop/companion-assets/harness/theuth.svg"
for s in 16 20 24 32; do r "$ROOT/crates/desktop/companion-assets/harness/theuth.svg" "$ROOT/crates/desktop/companion-assets/harness/theuth-$s.png" $s; done

echo "== website"
r "$BRAND/ios-icon-dark.svg" "$TMP/ati.png" 256
magick "$TMP/ati.png" -alpha off "PNG24:$ROOT/site/public/apple-touch-icon.png"
echo "done"
