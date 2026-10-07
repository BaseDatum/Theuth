#!/bin/bash
# Build `Theuth Companion.app` from the SwiftPM package with the Command Line Tools alone (no Xcode;
# spike SA): `swift build -c release`, a hand-written Info.plist, PkgInfo, an .icns via `iconutil` from a
# PNG drawn in Swift, codesign (ad-hoc, or `THEUTH_SIGN_IDENTITY`), `codesign --verify --deep --strict`.
# Output: target/release/companion/Theuth Companion.app. Nesting into the Tauri bundle is W1S's.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="$ROOT/crates/desktop/companion"
OUT="${CARGO_TARGET_DIR:-$ROOT/target}/release/companion"
APP="$OUT/Theuth Companion.app"
EXE=Companion
BUNDLE_ID=io.theuth.mac.companion
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
IDENTITY="${THEUTH_SIGN_IDENTITY:--}"
# The machine-wide build semaphore when present (shared build hosts); plain `swift` otherwise.
if command -v build-lock >/dev/null 2>&1; then LOCK=(env "BUILD_LABEL=${BUILD_LABEL:-companion}" build-lock); else LOCK=(); fi

echo "== build (swift build -c release)"
( cd "$PKG" && "${LOCK[@]}" swift build -c release 2>&1 | grep -v '^\[build-lock\]' | tail -3 )
BIN_DIR="$PKG/.build/release"
test -x "$BIN_DIR/$EXE"

echo "== assemble $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_DIR/$EXE" "$APP/Contents/MacOS/$EXE"
chmod 755 "$APP/Contents/MacOS/$EXE"
printf 'APPL????' > "$APP/Contents/PkgInfo"
# SwiftPM resource bundles of dependencies (KeyboardShortcuts' localisations) live beside the binary; the
# generated `Bundle.module` accessor also looks in the app's Resources.
for b in "$BIN_DIR"/*.bundle; do [ -d "$b" ] && cp -R "$b" "$APP/Contents/Resources/"; done

echo "== icon (iconutil)"
ICON_KEY=""
if swift "$PKG/bundle/icon.swift" "$OUT/icon-1024.png"; then
  ICONSET="$OUT/AppIcon.iconset"; rm -rf "$ICONSET"; mkdir -p "$ICONSET"
  for s in 16 32 128 256 512; do
    sips -z $s $s "$OUT/icon-1024.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    d=$((s*2)); sips -z $d $d "$OUT/icon-1024.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
  ICON_KEY="<key>CFBundleIconFile</key><string>AppIcon</string>"
else
  echo "icon.swift failed — bundling without an icon" >&2
fi

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>$EXE</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleName</key><string>Theuth Companion</string>
  <key>CFBundleDisplayName</key><string>Theuth Companion</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHumanReadableCopyright</key><string>Copyright © 2026 Theuth. All rights reserved.</string>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSSupportsAutomaticTermination</key><false/>
  <key>NSSupportsSuddenTermination</key><false/>
  $ICON_KEY
</dict>
</plist>
EOF
plutil -lint "$APP/Contents/Info.plist"

echo "== sign ($IDENTITY)"
xattr -crs "$APP" || true
if [ "$IDENTITY" = "-" ]; then
  codesign --force --deep --sign - "$APP"
else
  codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
fi
codesign --verify --deep --strict --verbose=2 "$APP"
echo "== done: $APP ($VERSION)"
