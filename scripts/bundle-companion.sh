#!/bin/bash
# Build `Theuth Companion.app` from the SwiftPM package with Xcode (189 D5/F1: SwiftStreamingMarkdown's `#Preview`
# blocks and asset catalogs do not build with the Command Line Tools, and its `Bundle.module` must find its resources
# in Contents/Resources): `xcodebuild` on the package's `Companion` scheme, a hand-written Info.plist, PkgInfo,
# theuth's own tray glyph and .icns from crates/desktop/icons, codesign (ad-hoc, or `THEUTH_SIGN_IDENTITY`),
# `codesign --verify --deep --strict`. Proof the resources load: `"$APP/Contents/MacOS/Companion" --probe-markdown`.
# Output: target/release/companion/Theuth Companion.app. Nesting into the Tauri bundle is W1S's.
set -euo pipefail
DEV_DIR="$(xcode-select -p 2>/dev/null || true)"
case "$DEV_DIR" in
  *.app/Contents/Developer) ;;
  *) echo "error: xcode-select -p is '${DEV_DIR:-unset}': SwiftStreamingMarkdown needs Xcode's toolchain (#Preview macros); run on the release Mac or install Xcode" >&2; exit 1 ;;
esac
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="$ROOT/crates/desktop/companion"
OUT="${CARGO_TARGET_DIR:-$ROOT/target}/release/companion"
APP="$OUT/Theuth Companion.app"
EXE=Companion
BUNDLE_ID=io.theuth.mac.companion
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
IDENTITY="${THEUTH_SIGN_IDENTITY:--}"

echo "== build (xcodebuild -scheme $EXE -configuration Release)"
DERIVED="$PKG/.build/xcode"
( cd "$PKG" && xcodebuild -scheme "$EXE" -configuration Release -destination 'generic/platform=macOS' \
    -derivedDataPath "$DERIVED" -skipMacroValidation -skipPackagePluginValidation \
    ARCHS=arm64 ONLY_ACTIVE_ARCH=NO CODE_SIGNING_ALLOWED=NO build )
BIN_DIR="$DERIVED/Build/Products/Release"
test -x "$BIN_DIR/$EXE"
# Every package product links statically today; a dynamic one would need Contents/Frameworks and an rpath, which this
# script does not do — refuse rather than ship an app that cannot load it.
if compgen -G "$BIN_DIR/PackageFrameworks/*.framework" > /dev/null; then
  echo "error: $BIN_DIR/PackageFrameworks holds dynamic frameworks; bundle-companion.sh copies none" >&2; exit 1
fi

echo "== assemble $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_DIR/$EXE" "$APP/Contents/MacOS/$EXE"
chmod 755 "$APP/Contents/MacOS/$EXE"
printf 'APPL????' > "$APP/Contents/PkgInfo"
# The packages' resource bundles (KeyboardShortcuts' localisations; SwiftStreamingMarkdown's asset catalog and strings,
# HighlightSwift's highlight.js, iosMath's fonts) go to Contents/Resources. `Bundle.module` is safe ONLY because the
# executable is built with xcodebuild: Xcode's generated accessor searches `Bundle.main.resourceURL` (Contents/Resources)
# before the bundle root. `swift build`'s accessor looks only at the .app root and the absolute .build path baked in at
# compile time (130 W6 item 1), and a copy or symlink at the .app root fails `codesign --verify --strict` ("unsealed
# contents present in the bundle root") — never go back to `swift build` for this bundle.
for b in "$BIN_DIR"/*.bundle; do [ -d "$b" ] && cp -R "$b" "$APP/Contents/Resources/"; done
chmod -R u+w "$APP/Contents/Resources"

echo "== icons (theuth's own: the Tauri tray glyph and the main app's .icns)"
ICONS="$ROOT/crates/desktop/icons"
cp "$ICONS/tray.png" "$ICONS/tray@2x.png" "$APP/Contents/Resources/"
cp "$ICONS/icon.icns" "$APP/Contents/Resources/AppIcon.icns"
# 149: the harness glyph set (SVG + 16/20/24/32 PNGs from scripts/render-harness-icons.swift) → Contents/Resources/harness/.
cp -R "$ROOT/crates/desktop/companion-assets/harness" "$APP/Contents/Resources/harness"
ICON_KEY="<key>CFBundleIconFile</key><string>AppIcon</string>"

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
  <key>NSMicrophoneUsageDescription</key><string>Theuth Companion records only while you hold a hotkey — the Ask bar's, an agent row's mic, or the voice key (⌃S) — to dictate a request to theuth.</string>
  <key>NSCalendarsFullAccessUsageDescription</key><string>theuth matches detected meetings to your calendar and seeds the meeting's context. Events never leave this Mac except as text sent to the model you chose.</string>
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
