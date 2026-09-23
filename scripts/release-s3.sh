#!/usr/bin/env bash
# Publish a theuth release to the update bucket and bump the Homebrew tap (39 §5).
#
# Pipeline (after package-macos, notarize-macos, package-update-macos; Windows and Linux artifacts
# already in dist/ from `just build-remote <host> windows|linux`, or staged in the bucket by
# `just stage-linux` / `just package-linux` on a Linux box):
#   gates 1-8 (§5.2)     version == Info.plist, Developer ID (app + theuth-app-parser helper), notarized,
#                        pubkeys match, <ver>/ absent (immutability), every Linux tarball present, each Linux
#                        tarball prints VERSION in a clean ubuntu:24.04 of its arch and its helper answers
#                        `--version` with the library hash, every CLI tarball holds exactly
#                        `t` + `theuth-app-parser-<hash>` + `theuth-documents-<ver>` and the Windows CLI zip the
#                        same three as .exe (46 §2.1, 59 §3.2; scripts/app-parser-env.sh)
#   dist/ first          Linux/Windows artifacts are read from dist/; staging/<ver>/ in the bucket is pulled
#                        only for what dist/ lacks (tarballs required; AppImage/deb/rpm/-setup.exe/zip optional)
#   sign                 cargo tauri signer sign  dist/theuth_<ver>_*.tar.gz, *.AppImage, *-setup.exe, *.zip → .sig
#   SHA256SUMS           shasum -a 256 over everything published (signed payloads + DMG + deb + rpm)
#   upload               dist/theuth_<ver>_* + SHA256SUMS → s3://$BUCKET/$PREFIX/<ver>/
#   verify               re-download every signed payload + .sig via the PUBLIC url; minisign -V
#                        against crates/update/pubkey.txt (sha256 vs SHA256SUMS when minisign
#                        is not installed — cargo tauri signer has no verify subcommand)
#   latest.json          jq; {version, platforms: {darwin-aarch64, cli-macos-aarch64, cli-linux-x86_64,
#                        cli-linux-aarch64, and when the artifact exists: windows-x86_64 (-setup.exe),
#                        cli-windows-x86_64 (zip), linux-x86_64 (AppImage) — absent → one WARN, key omitted}}
#                        — written last
#   tap bump (§5.3)      clone $THEUTH_TAP_REPO, render Formula/theuth.rb + Casks/theuth.rb,
#                        brew style + brew audit --strict --online by name (path audit is disabled),
#                        commit "theuth <ver>", push
#
# dist/ = target/release/dist (package-update-macos.sh). Objects under <ver>/ are never
# re-uploaded; latest.json is the only mutable object.
#
# Env (defaults for rustfs @ s3.rwx.dev; repo-root .env.signing is loaded):
#   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
#   THEUTH_S3_ENDPOINT          API endpoint (default: https://s3.rwx.dev)
#   THEUTH_S3_BUCKET            bucket (default: theuth)
#   THEUTH_S3_PREFIX            key prefix (default: releases)
#   THEUTH_S3_PUBLIC_BASE_URL   public base for downloads (default: ${ENDPOINT}/${BUCKET}/${PREFIX})
#   THEUTH_UPDATE_KEY           minisign private key: path (~/.tauri/theuth.key) or base64 contents
#   THEUTH_UPDATE_KEY_PASSWORD  its password (empty for --ci keys)
#   THEUTH_RELEASE_LINUX        require | skip (default: require). skip = manifest without
#                            cli-linux-* / linux-* keys, formula without on_linux (macOS-only hotfix)
#   THEUTH_SKIP_LINUX_SMOKE=1   gate 7 off: no Docker on this Mac (the Ubuntu build host already ran
#                            `t --version` natively; glibc breakage of the Docker-built aarch64 tarball
#                            would reach users). Without it, a missing docker fails gate 7.
#   THEUTH_RELEASE_PRERELEASE=1 ad-hoc/unnotarized allowed; writes latest-prerelease.json only,
#                            never latest.json, never bumps the tap
#   THEUTH_TAP_REPO             tap git url (default: git@github.com:basedatum/homebrew-tap)
#   THEUTH_S3_DRY_RUN=1         no writes: uploads, staging delete and tap push are printed
set -euo pipefail

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=load-signing-env.sh
source "$ROOT/scripts/load-signing-env.sh"
cd "$ROOT"
# shellcheck source=app-parser-env.sh
source "$ROOT/scripts/app-parser-env.sh"

need() { command -v "$1" >/dev/null 2>&1 || { echo "error: $1 required${2:+ ($2)}" >&2; exit 1; }; }
need aws "brew install awscli"
need jq
need curl
need git
need shasum
need cargo 'export PATH="$(just toolchain):$PATH"'
cargo tauri --version >/dev/null 2>&1 || { echo "error: cargo tauri required (cargo install tauri-cli)" >&2; exit 1; }
[[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] \
  || { echo "error: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY unset (see .env.signing.example)" >&2; exit 1; }
[[ -n "${THEUTH_UPDATE_KEY:-}" ]] \
  || { echo "error: THEUTH_UPDATE_KEY unset (cargo tauri signer generate -w ~/.tauri/theuth.key; see .env.signing.example)" >&2; exit 1; }

ENDPOINT="${THEUTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"
BUCKET="${THEUTH_S3_BUCKET:-theuth}"
PREFIX="${THEUTH_S3_PREFIX:-releases}"; PREFIX="${PREFIX#/}"; PREFIX="${PREFIX%/}"
PUBLIC_BASE="${THEUTH_S3_PUBLIC_BASE_URL:-${ENDPOINT}/${BUCKET}/${PREFIX}}"; PUBLIC_BASE="${PUBLIC_BASE%/}"
LINUX="${THEUTH_RELEASE_LINUX:-require}"
PRERELEASE="${THEUTH_RELEASE_PRERELEASE:-0}"
TAP_REPO="${THEUTH_TAP_REPO:-git@github.com:basedatum/homebrew-tap}"
DRY_RUN="${THEUTH_S3_DRY_RUN:-0}"
case "$LINUX" in require | skip) ;; *) echo "error: THEUTH_RELEASE_LINUX must be require|skip" >&2; exit 1 ;; esac

# rustfs: path-style, no mandatory checksums, no IMDS probe (jarvis release-s3.sh:164-174).
export AWS_S3_FORCE_PATH_STYLE=true
export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
export AWS_EC2_METADATA_DISABLED=true
AWS=(aws --endpoint-url "$ENDPOINT")

# Signing key → the env the tauri CLI reads (path or contents). An unset password makes
# the CLI prompt on a TTY and die in a script, so it is always exported.
if [[ -f "$THEUTH_UPDATE_KEY" ]]; then
  export TAURI_SIGNING_PRIVATE_KEY_PATH="$THEUTH_UPDATE_KEY"; unset TAURI_SIGNING_PRIVATE_KEY
else
  export TAURI_SIGNING_PRIVATE_KEY="$THEUTH_UPDATE_KEY"; unset TAURI_SIGNING_PRIVATE_KEY_PATH
fi
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${THEUTH_UPDATE_KEY_PASSWORD-}"

VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="theuth") | .version')"
KEY="$(uname -s | tr '[:upper:]' '[:lower:]' | sed s/darwin/macos/)-$(uname -m | sed s/arm64/aarch64/)"
[[ "$KEY" == "macos-aarch64" ]] || { echo "error: release runs on the arm64 release Mac (got $KEY)" >&2; exit 1; }
DIST="target/release/dist"
APP="target/release/bundle/macos/theuth.app"
PUBKEY_FILE="crates/update/pubkey.txt"
PUBKEY="$(head -n1 "$PUBKEY_FILE")"
S3_VER="s3://${BUCKET}/${PREFIX}/${VERSION}"
S3_STAGING="s3://${BUCKET}/staging/${VERSION}"
PUB_VER="${PUBLIC_BASE}/${VERSION}"
APP_TGZ="theuth_${VERSION}_${KEY}.app.tar.gz"
CLI_TGZ="theuth_${VERSION}_${KEY}.tar.gz"
DMG="theuth_${VERSION}_${KEY}.dmg"
LINUX_TGZ=()
[[ "$LINUX" == "skip" ]] || LINUX_TGZ=("theuth_${VERSION}_linux-x86_64.tar.gz" "theuth_${VERSION}_linux-aarch64.tar.gz")
MANIFEST_NAME="latest.json"; [[ "$PRERELEASE" == "1" ]] && MANIFEST_NAME="latest-prerelease.json"

for f in "$APP_TGZ" "$CLI_TGZ" "$DMG"; do
  [[ -f "$DIST/$f" ]] || { echo "error: $DIST/$f missing — run just package-update-macos" >&2; exit 1; }
done
# Tauri's pubkey encoding is base64 of the whole .pub file, so it starts with "untrusted comment:" encoded.
[[ "$PUBKEY" == dW50cnVzdGVk* ]] || { echo "error: $PUBKEY_FILE is not a Tauri minisign public key (cargo tauri signer generate -w ~/.tauri/theuth.key)" >&2; exit 1; }

echo "==> release theuth $VERSION"
echo "    endpoint:   $ENDPOINT"
echo "    bucket:     s3://${BUCKET}/${PREFIX}/"
echo "    public:     $PUBLIC_BASE"
echo "    linux:      $LINUX"
echo "    manifest:   $MANIFEST_NAME"
[[ "$PRERELEASE" == "1" ]] && echo "    prerelease: gates 2-3 off, no tap bump"
echo "    dry run:    $DRY_RUN"
echo

# ── gates (§5.2) ─────────────────────────────────────────────────────────────
gate() { echo "==> gate $1: $2"; }
fail() { echo "error: gate $1 failed: $2" >&2; exit 1; }

gate 1 "VERSION == CFBundleShortVersionString"
[[ -d "$APP" ]] || fail 1 "$APP missing — run just package-macos"
PLIST_VER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
[[ "$PLIST_VER" == "$VERSION" ]] || fail 1 "Cargo.toml $VERSION vs Info.plist $PLIST_VER (rebuild the app)"

if [[ "$PRERELEASE" == "1" ]]; then
  echo "==> gates 2-3 skipped (THEUTH_RELEASE_PRERELEASE=1)"
else
  gate 2 "codesign shows a Developer ID (app, $HELPER and $DOC_HELPER)"
  # Capture, don't `grep -q` in a pipe: with pipefail, grep -q closes early,
  # codesign gets SIGPIPE (141), the gate fails even when Authority is present.
  signed_developer_id() {
    local out
    out="$(codesign -d --verbose=2 "$1" 2>&1)" || true
    [[ "$out" == *"Authority=Developer ID"* ]]
  }
  signed_developer_id "$APP" || fail 2 "ad-hoc build (set THEUTH_SIGN_IDENTITY, rerun just package-macos)"
  for h in "$HELPER" "$DOC_HELPER"; do
    signed_developer_id "$APP/Contents/MacOS/$h" || fail 2 "$h not signed with a Developer ID (rerun just package-macos)"
    ents="$(codesign -d --entitlements - --xml "$APP/Contents/MacOS/$h" 2>/dev/null)" || true
    [[ "$ents" == *"com.apple.security.cs.allow-jit"* ]] || fail 2 "$h lacks the JIT entitlement (scripts/entitlements-app-parser.plist)"
  done
  gate 3 "spctl --assess (notarized)"
  spctl --assess --type execute "$APP" 2>/dev/null || fail 3 "not notarized (just notarize-macos)"
fi

gate 4 "pubkey.txt == tauri.conf.json plugins.updater.pubkey"
CONF_PUBKEY="$(jq -r '.plugins.updater.pubkey // empty' crates/desktop/tauri.conf.json)"
[[ "$CONF_PUBKEY" == "$PUBKEY" ]] || fail 4 "the two pubkey copies diverged"

gate 5 "$S3_VER/ does not exist"
RESUME=0
if "${AWS[@]}" s3 ls "$S3_VER/" >/dev/null 2>&1; then
  LIVE="$(curl -fsSL --max-time 15 "$PUBLIC_BASE/$MANIFEST_NAME" 2>/dev/null | jq -r '.version // empty' || true)"
  if [[ "$LIVE" == "$VERSION" ]]; then
    fail 5 "$S3_VER/ already published and $MANIFEST_NAME is $VERSION — bump the version"
  fi
  # Partial previous run (objects uploaded, manifest not). Skip sign/upload; verify + write manifest.
  echo "    $S3_VER/ already has objects; $MANIFEST_NAME is not $VERSION — resuming at verify"
  RESUME=1
fi

gate 6 "dist/ (primary) or staging/$VERSION/ (fallback) holds every Linux tarball"
# fetch <name>: 0 when dist/ has it (already, or pulled from staging/); 1 when nowhere.
fetch() {
  if [[ -f "$DIST/$1" ]]; then echo "    dist/ $1"; return 0; fi
  if "${AWS[@]}" s3 ls "$S3_STAGING/$1" >/dev/null 2>&1; then
    echo "    pull  $1"
    "${AWS[@]}" s3 cp "$S3_STAGING/$1" "$DIST/$1" >/dev/null
    return 0
  fi
  return 1
}
LINUX_DESKTOP=()   # deb + rpm: published as downloads with SHA256SUMS, not updater payloads
LINUX_APPIMAGE=()  # signed; latest.json linux-<arch> (Tauri's AppImage updater)
WIN_SETUP=""; WIN_ZIP=""
if [[ "$LINUX" == "skip" ]]; then
  echo "    THEUTH_RELEASE_LINUX=skip: manifest and formula without Linux"
else
  for f in "${LINUX_TGZ[@]}"; do
    fetch "$f" || fail 6 "$f neither in $DIST/ nor $S3_STAGING/ (just build-remote <host> linux, just stage-linux on that arch, or THEUTH_RELEASE_LINUX=skip)"
  done
  # Desktop bundles (just build-remote <host> linux / just package-linux) are optional.
  for arch in x86_64 aarch64; do
    for ext in deb rpm AppImage; do
      f="theuth_${VERSION}_linux-${arch}.${ext}"
      if fetch "$f"; then
        case "$ext" in AppImage) LINUX_APPIMAGE+=("$f") ;; *) LINUX_DESKTOP+=("$f") ;; esac
      fi
    done
  done
  echo "    desktop bundles: ${#LINUX_DESKTOP[@]} deb/rpm, ${#LINUX_APPIMAGE[@]} AppImage"
  [[ ${#LINUX_APPIMAGE[@]} -gt 0 ]] || echo "    WARN: no theuth_${VERSION}_linux-*.AppImage — latest.json gets no linux-* key" >&2
fi
# Windows (just build-remote <host> windows) is optional: absent → keys omitted, one WARN each.
f="theuth_${VERSION}_windows-x86_64-setup.exe"
if fetch "$f"; then WIN_SETUP="$f"; else echo "    WARN: no $f — latest.json gets no windows-x86_64 key" >&2; fi
f="theuth_${VERSION}_windows-x86_64.zip"
if fetch "$f"; then WIN_ZIP="$f"; else echo "    WARN: no $f — latest.json gets no cli-windows-x86_64 key" >&2; fi

gate 7 "each Linux tarball runs in a clean ubuntu:24.04 of its arch"
if [[ "$LINUX" == "skip" ]]; then
  echo "    skipped (no Linux tarballs)"
elif [[ "${THEUTH_SKIP_LINUX_SMOKE:-0}" == "1" ]]; then
  echo "    WARNING: THEUTH_SKIP_LINUX_SMOKE=1 — Linux tarballs are NOT smoke-tested (glibc/shared-lib breakage would reach users)" >&2
elif ! command -v docker >/dev/null 2>&1; then
  fail 7 "docker not found — install Docker Desktop, or THEUTH_SKIP_LINUX_SMOKE=1 to release without the clean-ubuntu smoke test"
else
  for f in "${LINUX_TGZ[@]}"; do
    case "$f" in *linux-x86_64*) plat=linux/amd64 ;; *) plat=linux/arm64 ;; esac
    out="$(docker run --rm --platform "$plat" -v "$PWD/$DIST:/d:ro" ubuntu:24.04 \
      bash -c "tar xzf /d/$f -C /tmp && /tmp/t --version && echo wants=\$(/tmp/t --app-parser-hash) && /tmp/$HELPER --version && printf '' | /tmp/$HELPER && /tmp/$DOC_HELPER --version && printf '' | /tmp/$DOC_HELPER" 2>&1)" || fail 7 "$f: $out"
    [[ "$out" == *"$VERSION"* ]] || fail 7 "$f printed '$out', expected $VERSION"
    [[ "$out" == *"wants=$HELPER_HASH"* ]] || fail 7 "$f: t --app-parser-hash does not name $HELPER_HASH: '$out'"
    [[ "$out" == *"theuth-app-parser $HELPER_HASH"* ]] || fail 7 "$f: $HELPER --version printed '$out', expected 'theuth-app-parser $HELPER_HASH'"
    [[ "$out" == *"theuth-documents $DOC_VERSION"* ]] || fail 7 "$f: $DOC_HELPER --version printed '$out', expected 'theuth-documents $DOC_VERSION'"
    echo "    $f: $(tr '\n' ' ' <<<"$out")($plat)"
  done
fi

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

gate 8 "every CLI tarball holds exactly t + $HELPER + $DOC_HELPER (the Windows zip: the same three .exe); the macOS helpers run here"
for f in "$CLI_TGZ" ${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"}; do
  listed="$(tar -tzf "$DIST/$f" | sed 's|^\./||' | sort | tr '\n' ' ')"
  [[ "$listed" == "t $HELPER $DOC_HELPER " ]] || fail 8 "$f lists '$listed', expected 't $HELPER $DOC_HELPER' (stale dist/ or staging from another library hash / protocol version)"
  echo "    $f: t $HELPER $DOC_HELPER"
done
if [[ -n "$WIN_ZIP" ]]; then
  need unzip
  listed="$(unzip -Z1 "$DIST/$WIN_ZIP" | sed 's|^\./||' | sort | tr '\n' ' ')"
  [[ "$listed" == "t.exe $HELPER.exe $DOC_HELPER.exe " ]] || fail 8 "$WIN_ZIP lists '$listed', expected 't.exe $HELPER.exe $DOC_HELPER.exe' (stale dist/ from another library hash / protocol version)"
  echo "    $WIN_ZIP: t.exe $HELPER.exe $DOC_HELPER.exe"
fi
mkdir -p "$WORK/cli" && tar -xzf "$DIST/$CLI_TGZ" -C "$WORK/cli"
check_helper "$WORK/cli/$HELPER"
check_doc_helper "$WORK/cli/$DOC_HELPER"
[[ "$("$WORK/cli/t" --version 2>&1)" == *"$VERSION"* ]] || fail 8 "$CLI_TGZ: t --version did not print $VERSION"
[[ "$("$WORK/cli/t" --app-parser-hash 2>&1)" == "$HELPER_HASH" ]] || fail 8 "$CLI_TGZ: t --app-parser-hash does not name the shipped $HELPER (the updater refuses such a payload)"

# ── sign + sums ──────────────────────────────────────────────────────────────
# Every updater payload gets a minisign .sig: tarballs, AppImages, the NSIS installer, the Windows CLI zip.
SIGNED=("$APP_TGZ" "$CLI_TGZ" ${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"} ${LINUX_APPIMAGE[@]+"${LINUX_APPIMAGE[@]}"})
[[ -z "$WIN_SETUP" ]] || SIGNED+=("$WIN_SETUP")
[[ -z "$WIN_ZIP" ]] || SIGNED+=("$WIN_ZIP")
s3_cp() { # src dest — content type from the destination key; DRY_RUN prints
  local ct
  case "$2" in
    *.json) ct=application/json ;; *.dmg) ct=application/x-apple-diskimage ;; *.exe) ct=application/vnd.microsoft.portable-executable ;;
    *.tar.gz) ct=application/gzip ;; *.zip) ct=application/zip ;; *.sig | *.deb | *.rpm | *.AppImage) ct=application/octet-stream ;; *) ct=text/plain ;;
  esac
  if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: aws s3 cp $1 $2"; return 0; fi
  "${AWS[@]}" s3 cp "$1" "$2" --content-type "$ct" >/dev/null
  echo "    $2"
}

if [[ "$RESUME" == "1" ]]; then
  echo "==> resume: skip sign/upload (leave $S3_VER/ immutable)"
  for f in "${SIGNED[@]}"; do
    [[ -s "$DIST/$f.sig" ]] || { echo "error: $DIST/$f.sig missing — cannot resume" >&2; exit 1; }
  done
  [[ -s "$DIST/SHA256SUMS" ]] || { echo "error: $DIST/SHA256SUMS missing — cannot resume" >&2; exit 1; }
else
  echo "==> signing ${#SIGNED[@]} payloads (cargo tauri signer sign)"
  for f in "${SIGNED[@]}"; do
    rm -f "$DIST/$f.sig"
    cargo tauri signer sign "$DIST/$f" >/dev/null
    [[ -s "$DIST/$f.sig" ]] || { echo "error: $DIST/$f.sig not produced" >&2; exit 1; }
    echo "    $f.sig"
  done

  echo "==> SHA256SUMS"
  (cd "$DIST" && shasum -a 256 "${SIGNED[@]}" "$DMG" ${LINUX_DESKTOP[@]+"${LINUX_DESKTOP[@]}"} >SHA256SUMS && cat SHA256SUMS)

  echo "==> upload → $S3_VER/"
  UPLOADS=("${SIGNED[@]}" "$DMG" ${LINUX_DESKTOP[@]+"${LINUX_DESKTOP[@]}"} SHA256SUMS)
  for f in "${SIGNED[@]}"; do UPLOADS+=("$f.sig"); done
  for f in "${UPLOADS[@]}"; do s3_cp "$DIST/$f" "$S3_VER/$f"; done
fi
sha_of() { awk -v f="$1" '$2 == f {print $1}' "$DIST/SHA256SUMS"; }

TMP="$WORK/dl"; mkdir -p "$TMP"

# ── verify what the client will fetch ────────────────────────────────────────
echo "==> verify via $PUB_VER/"
if [[ "$DRY_RUN" == "1" ]]; then
  echo "    DRY_RUN: nothing uploaded, nothing to verify"
else
  if command -v minisign >/dev/null 2>&1; then
    # pubkey.txt / .sig are Tauri's base64-of-the-minisign-file; minisign wants the raw key line.
    # macOS base64 takes stdin (`-i`), not a file operand; GNU accepts stdin too.
    RAW_PUB="$(base64 -d < "$PUBKEY_FILE" | sed -n 2p)"
  else
    echo "    minisign not installed (brew install minisign): comparing sha256 with SHA256SUMS and .sig bytes with dist/ instead"
  fi
  for f in "${SIGNED[@]}"; do
    curl -fsSL --retry 3 -o "$TMP/$f" "$PUB_VER/$f"
    curl -fsSL --retry 3 -o "$TMP/$f.sig" "$PUB_VER/$f.sig"
    if [[ -n "${RAW_PUB:-}" ]]; then
      base64 -d < "$TMP/$f.sig" >"$TMP/$f.minisig"
      minisign -Vq -P "$RAW_PUB" -m "$TMP/$f" -x "$TMP/$f.minisig" || { echo "error: $f: signature does not verify against $PUBKEY_FILE" >&2; exit 1; }
    else
      [[ "$(shasum -a 256 "$TMP/$f" | awk '{print $1}')" == "$(sha_of "$f")" ]] || { echo "error: $f: sha256 differs from SHA256SUMS after upload" >&2; exit 1; }
      cmp -s "$TMP/$f.sig" "$DIST/$f.sig" || { echo "error: $f.sig differs after upload" >&2; exit 1; }
    fi
    echo "    $f ok"
  done
fi

# ── manifest (§3.1) ──────────────────────────────────────────────────────────
echo "==> $MANIFEST_NAME"
MANIFEST="$(jq -n --arg v "$VERSION" '{version: $v, platforms: {}}')"
platform() { # key file
  MANIFEST="$(jq --arg k "$1" --arg u "$PUB_VER/$2" --arg s "$(cat "$DIST/$2.sig")" \
    '.platforms[$k] = {url: $u, signature: $s}' <<<"$MANIFEST")"
}
platform darwin-aarch64 "$APP_TGZ"
platform cli-macos-aarch64 "$CLI_TGZ"
for f in ${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"}; do
  case "$f" in *linux-x86_64*) platform cli-linux-x86_64 "$f" ;; *) platform cli-linux-aarch64 "$f" ;; esac
done
# Desktop updaters (Tauri: <os>-<arch>): AppImage per arch, the NSIS installer; the Windows CLI zip for `t --update`.
for f in ${LINUX_APPIMAGE[@]+"${LINUX_APPIMAGE[@]}"}; do
  case "$f" in *linux-x86_64*) platform linux-x86_64 "$f" ;; *) platform linux-aarch64 "$f" ;; esac
done
[[ -z "$WIN_SETUP" ]] || platform windows-x86_64 "$WIN_SETUP"
[[ -z "$WIN_ZIP" ]] || platform cli-windows-x86_64 "$WIN_ZIP"
printf '%s\n' "$MANIFEST" >"$DIST/$MANIFEST_NAME"
jq . "$DIST/$MANIFEST_NAME"
s3_cp "$DIST/$MANIFEST_NAME" "s3://${BUCKET}/${PREFIX}/${MANIFEST_NAME}"

# ── staging cleanup ──────────────────────────────────────────────────────────
if [[ "$LINUX" != "skip" ]]; then
  echo "==> delete $S3_STAGING/"
  if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: aws s3 rm --recursive $S3_STAGING/"
  else "${AWS[@]}" s3 rm --recursive "$S3_STAGING/" >/dev/null; fi
fi

# ── tap bump (§5.3) ──────────────────────────────────────────────────────────
# Rendered files always land in dist/tap/ so a failed audit or a dry run leaves something to inspect.
render() { # tmpl out
  sed -e "s|@VERSION@|$VERSION|g" -e "s|@BASE_URL@|$PUBLIC_BASE|g" \
      -e "s|@SHA_MACOS_CLI@|$(sha_of "$CLI_TGZ")|g" -e "s|@SHA_DMG@|$(sha_of "$DMG")|g" \
      -e "s|@SHA_LINUX_X86@|$(sha_of "theuth_${VERSION}_linux-x86_64.tar.gz")|g" \
      -e "s|@SHA_LINUX_ARM@|$(sha_of "theuth_${VERSION}_linux-aarch64.tar.gz")|g" "$1" \
    | if [[ "$LINUX" == "skip" ]]; then sed '/^# @LINUX_BEGIN@$/,/^# @LINUX_END@$/d'
      else sed -e '/^# @LINUX_BEGIN@$/d' -e '/^# @LINUX_END@$/d'; fi >"$2"
}
mkdir -p "$DIST/tap/Formula" "$DIST/tap/Casks"
render scripts/tap/theuth.rb.formula.tmpl "$DIST/tap/Formula/theuth.rb"
render scripts/tap/theuth.rb.cask.tmpl "$DIST/tap/Casks/theuth.rb"

TAP_STATUS="not bumped (prerelease); rendered at $DIST/tap/"
if [[ "$PRERELEASE" != "1" ]]; then
  echo "==> tap bump: $TAP_REPO"
  TAP="$WORK/tap"
  git clone --quiet --depth 1 "$TAP_REPO" "$TAP"
  mkdir -p "$TAP/Formula" "$TAP/Casks"
  cp "$DIST/tap/Formula/theuth.rb" "$TAP/Formula/theuth.rb"
  cp "$DIST/tap/Casks/theuth.rb" "$TAP/Casks/theuth.rb"
  AUDIT_OK=1
  if command -v brew >/dev/null 2>&1; then
    # Homebrew rejects `brew audit path.rb` (odisabled). Named audit reads the
    # installed tap, so stage the rendered files there and restore afterwards.
    # --online checks url reachability and sha256 (39 §5.3).
    INSTALLED="$(brew --repo basedatum/homebrew-tap 2>/dev/null || true)"
    if [[ ! -d "${INSTALLED:-}/Formula" ]]; then
      brew tap basedatum/tap
      INSTALLED="$(brew --repo basedatum/homebrew-tap)"
    fi
    BACKUP="$(mktemp -d)"
    [[ -f "$INSTALLED/Formula/theuth.rb" ]] && cp "$INSTALLED/Formula/theuth.rb" "$BACKUP/formula.rb"
    [[ -f "$INSTALLED/Casks/theuth.rb" ]] && cp "$INSTALLED/Casks/theuth.rb" "$BACKUP/cask.rb"
    restore_installed_tap() {
      if [[ -f "$BACKUP/formula.rb" ]]; then cp "$BACKUP/formula.rb" "$INSTALLED/Formula/theuth.rb"
      else rm -f "$INSTALLED/Formula/theuth.rb"; fi
      if [[ -f "$BACKUP/cask.rb" ]]; then cp "$BACKUP/cask.rb" "$INSTALLED/Casks/theuth.rb"
      else rm -f "$INSTALLED/Casks/theuth.rb"; fi
      rm -rf "$BACKUP"
    }
    cp "$TAP/Formula/theuth.rb" "$INSTALLED/Formula/theuth.rb"
    cp "$TAP/Casks/theuth.rb" "$INSTALLED/Casks/theuth.rb"
    if brew style --formula basedatum/tap/theuth \
      && brew style --cask basedatum/tap/theuth \
      && brew audit --strict --online --formula basedatum/tap/theuth \
      && brew audit --strict --online --cask basedatum/tap/theuth
    then
      AUDIT_OK=1
    else
      AUDIT_OK=0
    fi
    restore_installed_tap
  else
    echo "    brew not found: audit skipped"
  fi
  if [[ "$AUDIT_OK" != "1" ]]; then
    # §5.3: a failed audit leaves the tap untouched; the release is otherwise complete.
    echo "    WARNING: brew audit failed — tap NOT updated; fix and commit $DIST/tap/ by hand" >&2
    TAP_STATUS="NOT bumped (brew audit failed); rendered at $DIST/tap/"
  elif [[ "$DRY_RUN" == "1" ]]; then
    echo "    DRY_RUN: git commit -m 'theuth $VERSION' && git push"
    TAP_STATUS="not pushed (dry run); rendered at $DIST/tap/"
  else
    (cd "$TAP" && git add Formula/theuth.rb Casks/theuth.rb && git commit --quiet -m "theuth $VERSION" && git push --quiet)
    TAP_STATUS="pushed theuth $VERSION"
    # The release Mac's brew checkout still has the previous formula until this.
    if [[ -n "${INSTALLED:-}" ]]; then
      git -C "$INSTALLED" pull --ff-only --quiet || echo "    installed tap not fast-forwarded; brew update will pick it up"
    fi
  fi
fi

# ── summary ──────────────────────────────────────────────────────────────────
echo
echo "done: theuth $VERSION"
echo "  objects:   $S3_VER/  (${#UPLOADS[@]} files)"
echo "  manifest:  ${PUBLIC_BASE}/${MANIFEST_NAME}"
echo "  platforms: $(jq -r '.platforms | keys | join(", ")' "$DIST/$MANIFEST_NAME")"
echo "  dmg:       $PUB_VER/$DMG"
echo "  tap:       $TAP_STATUS"
[[ "$DRY_RUN" != "1" ]] || echo "  DRY RUN: nothing was written to the bucket or the tap"
