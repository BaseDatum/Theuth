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
#                        `t` + `theuth-app-parser-<hash>` and the Windows CLI zip the same two as .exe
#                        (46 §2.1; scripts/app-parser-env.sh)
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
# re-uploaded; latest.json is mutable, SHA256SUMS is append-only (add-arch below).
#
# Add-arch (THEUTH_RELEASE_ADD=<platforms>|auto): publish platforms built after a version went live —
#   linux-x86_64 | linux-aarch64 | windows-x86_64, one or a comma list; `auto` (= just release-update)
#   picks, after gate 5, every platform whose files exist in dist/ or staging/<ver>/ (Linux: the tarball,
#   else "not built") but are not all under <ver>/ or whose manifest keys are missing, and exits 0
#   "up to date" when there is none. Gate 5 inverted (<ver>/ exists, live manifest == VERSION), gates 2-3
#   and the macOS parts of 6/8 off, gate 7/8 for the new files only (Windows: -setup.exe and/or zip, at
#   least one); refuses any object already under <ver>/ unless byte-identical (then that upload is
#   skipped and the published .sig is reused); appends the new lines to the live SHA256SUMS, adds
#   cli-<platform> (+ <platform> with an AppImage) / windows-x86_64 + cli-windows-x86_64 to the live
#   manifest, re-renders the tap from the merged SHA256SUMS and commits "theuth <ver>: add <platforms>"
#   only when the rendered files changed (a Windows-only add leaves the tap alone). staging/<ver>/ is
#   left alone.
#
# Env (defaults for rustfs @ s3.rwx.dev; repo-root .env.signing is loaded):
#   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
#   THEUTH_S3_ENDPOINT          API endpoint (default: https://s3.rwx.dev)
#   THEUTH_S3_BUCKET            bucket (default: theuth)
#   THEUTH_S3_PREFIX            key prefix (default: releases)
#   THEUTH_S3_PUBLIC_BASE_URL   public base for downloads (default: ${ENDPOINT}/${BUCKET}/${PREFIX})
#   THEUTH_UPDATE_KEY           minisign private key: path (~/.tauri/theuth.key) or base64 contents
#   THEUTH_UPDATE_KEY_PASSWORD  its password (empty for --ci keys)
#   THEUTH_RELEASE_LINUX        present | require | skip (default: present). present = every Linux CLI
#                            tarball found in dist/ or staging/ ships (at least one; a missing arch is a
#                            WARN, its latest.json keys and formula branch are omitted); require = both
#                            x86_64 and aarch64 must exist; skip = manifest without cli-linux-* / linux-*
#                            keys, formula without on_linux (macOS-only hotfix)
#   THEUTH_RELEASE_ADD          auto | linux-x86_64 | linux-aarch64 | windows-x86_64 (comma list allowed):
#                            add-arch mode (above) instead of a release. With THEUTH_RELEASE_LINUX=skip,
#                            auto leaves the Linux platforms out; naming one is an error.
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
LINUX="${THEUTH_RELEASE_LINUX:-present}"
PRERELEASE="${THEUTH_RELEASE_PRERELEASE:-0}"
TAP_REPO="${THEUTH_TAP_REPO:-git@github.com:basedatum/homebrew-tap}"
DRY_RUN="${THEUTH_S3_DRY_RUN:-0}"
case "$LINUX" in present | require | skip) ;; *) echo "error: THEUTH_RELEASE_LINUX must be present|require|skip" >&2; exit 1 ;; esac
ADD="${THEUTH_RELEASE_ADD:-}"   # non-empty = add-arch mode ("auto" or the platform list as given)
PLATFORMS=(linux-x86_64 linux-aarch64 windows-x86_64)
ADDS=()                          # the platforms to add; auto resolves them after gate 5
if [[ -n "$ADD" && "$ADD" != auto ]]; then
  IFS=, read -r -a ADDS <<<"$ADD"
  for p in ${ADDS[@]+"${ADDS[@]}"}; do
    case "$p" in linux-x86_64 | linux-aarch64 | windows-x86_64) ;;
      *) echo "error: THEUTH_RELEASE_ADD must be auto, or one or a comma list of linux-x86_64|linux-aarch64|windows-x86_64 (got '$p')" >&2; exit 1 ;;
    esac
    [[ "$LINUX" != "skip" || "$p" != linux-* ]] || { echo "error: THEUTH_RELEASE_LINUX=skip contradicts THEUTH_RELEASE_ADD=$p" >&2; exit 1; }
  done
fi

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
LINUX_ARCHES=(x86_64 aarch64)
LINUX_TGZ=()   # the CLI tarballs that ship; filled by gate 6 (present) or fixed (require)
[[ "$LINUX" != "require" ]] || LINUX_TGZ=("theuth_${VERSION}_linux-x86_64.tar.gz" "theuth_${VERSION}_linux-aarch64.tar.gz")
MANIFEST_NAME="latest.json"; [[ "$PRERELEASE" == "1" ]] && MANIFEST_NAME="latest-prerelease.json"

if [[ -z "$ADD" ]]; then
  for f in "$APP_TGZ" "$CLI_TGZ" "$DMG"; do
    [[ -f "$DIST/$f" ]] || { echo "error: $DIST/$f missing — run just package-update-macos" >&2; exit 1; }
  done
fi
# Tauri's pubkey encoding is base64 of the whole .pub file, so it starts with "untrusted comment:" encoded.
[[ "$PUBKEY" == dW50cnVzdGVk* ]] || { echo "error: $PUBKEY_FILE is not a Tauri minisign public key (cargo tauri signer generate -w ~/.tauri/theuth.key)" >&2; exit 1; }

echo "==> release theuth $VERSION"
echo "    endpoint:   $ENDPOINT"
echo "    bucket:     s3://${BUCKET}/${PREFIX}/"
echo "    public:     $PUBLIC_BASE"
echo "    linux:      $LINUX"
echo "    manifest:   $MANIFEST_NAME"
[[ "$PRERELEASE" == "1" ]] && echo "    prerelease: gates 2-3 off, no tap bump"
[[ -z "$ADD" ]] || echo "    add-arch:   $ADD (gates 2-3 off, gate 5 inverted; SHA256SUMS + $MANIFEST_NAME appended)"
echo "    dry run:    $DRY_RUN"
echo

# ── gates (§5.2) ─────────────────────────────────────────────────────────────
gate() { echo "==> gate $1: $2"; }
fail() { echo "error: gate $1 failed: $2" >&2; exit 1; }
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

# Review M1: the committed in-process bundles must match their inputs and embed the library hash `t`
# advertises (`t --app-parser-hash`); shasum + grep, no bun. Cheapest gate, so first.
gate 0 "in-process app bundles match their inputs and library hash $HELPER_HASH"
( check_apps_bundle ) || fail 0 "crates/app-js/js is stale (just apps-bundle, commit, rebuild)"

gate 1 "VERSION == CFBundleShortVersionString"
[[ -d "$APP" ]] || fail 1 "$APP missing — run just package-macos"
PLIST_VER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
[[ "$PLIST_VER" == "$VERSION" ]] || fail 1 "Cargo.toml $VERSION vs Info.plist $PLIST_VER (rebuild the app)"

if [[ -n "$ADD" ]]; then
  echo "==> gates 2-3 skipped (add-arch: the macOS app is not involved)"
elif [[ "$PRERELEASE" == "1" ]]; then
  echo "==> gates 2-3 skipped (THEUTH_RELEASE_PRERELEASE=1)"
else
  gate 2 "codesign shows a Developer ID (app and $HELPER)"
  # Capture, don't `grep -q` in a pipe: with pipefail, grep -q closes early,
  # codesign gets SIGPIPE (141), the gate fails even when Authority is present.
  signed_developer_id() {
    local out
    out="$(codesign -d --verbose=2 "$1" 2>&1)" || true
    [[ "$out" == *"Authority=Developer ID"* ]]
  }
  signed_developer_id "$APP" || fail 2 "ad-hoc build (set THEUTH_SIGN_IDENTITY, rerun just package-macos)"
  for h in "$HELPER"; do
    signed_developer_id "$APP/Contents/MacOS/$h" || fail 2 "$h not signed with a Developer ID (rerun just package-macos)"
  done
  # No JIT-entitled helper is left: the app-parser entry is a plain stub (apps-inproc-as-built-P1.md) and the
  # documents helper is compiled into `t` (documents-inproc-as-built-P3.md).
  gate 3 "spctl --assess (notarized)"
  spctl --assess --type execute "$APP" 2>/dev/null || fail 3 "not notarized (just notarize-macos)"
fi

gate 4 "pubkey.txt == tauri.conf.json plugins.updater.pubkey"
CONF_PUBKEY="$(jq -r '.plugins.updater.pubkey // empty' crates/desktop/tauri.conf.json)"
[[ "$CONF_PUBKEY" == "$PUBKEY" ]] || fail 4 "the two pubkey copies diverged"

RESUME=0
if [[ -n "$ADD" ]]; then
  gate 5 "$S3_VER/ exists and $MANIFEST_NAME is $VERSION (add-arch)"
  # The listing and the live manifest are reused: auto detection, gate 6 immutability, the manifest merge.
  EXISTING="$("${AWS[@]}" s3 ls "$S3_VER/" 2>/dev/null | awk '{print $4}')" || fail 5 "$S3_VER/ does not exist — nothing to add to (release first)"
  LIVE_MANIFEST="$(curl -fsSL --max-time 15 "$PUBLIC_BASE/$MANIFEST_NAME")" || fail 5 "cannot read $PUBLIC_BASE/$MANIFEST_NAME"
  LIVE="$(jq -r '.version // empty' <<<"$LIVE_MANIFEST")"
  [[ "$LIVE" == "$VERSION" ]] || fail 5 "$MANIFEST_NAME is '$LIVE', not $VERSION — add-arch only extends the live version"
  published() { grep -qxF "$1" <<<"$EXISTING"; }
else
  gate 5 "$S3_VER/ does not exist"
  if "${AWS[@]}" s3 ls "$S3_VER/" >/dev/null 2>&1; then
    LIVE="$(curl -fsSL --max-time 15 "$PUBLIC_BASE/$MANIFEST_NAME" 2>/dev/null | jq -r '.version // empty' || true)"
    if [[ "$LIVE" == "$VERSION" ]]; then
      fail 5 "$S3_VER/ already published and $MANIFEST_NAME is $VERSION — bump the version"
    fi
    # Partial previous run (some objects uploaded, manifest not). Sign is skipped and only the objects
    # <ver>/ is missing go up: a run that died mid-upload (the 490 MB .dmg hitting the CLI's read
    # timeout) leaves a prefix of the set, and treating that prefix as the whole set is what made every
    # retry skip the upload and then 404 in verify, forever.
    EXISTING="$("${AWS[@]}" s3 ls "$S3_VER/" | awk '{print $4}')"
    published() { grep -qxF "$1" <<<"$EXISTING"; }
    echo "    $S3_VER/ already has objects; $MANIFEST_NAME is not $VERSION — resuming (missing objects only)"
    RESUME=1
  fi
fi

if [[ "$ADD" == auto ]]; then
  # A platform is built when its files exist in dist/ or staging/ (no download, no byte comparison —
  # a rebuilt artifact of an already released platform must not fail this); it needs adding when any
  # of them is not under <ver>/ or any manifest key it implies is missing (a run that died before the
  # manifest write is repaired by re-running).
  echo "==> add-arch auto: platforms built since the release"
  exists() { [[ -f "$DIST/$1" ]] || "${AWS[@]}" s3 ls "$S3_STAGING/$1" >/dev/null 2>&1; }
  has_key() { jq -e --arg k "$1" '.platforms[$k]' <<<"$LIVE_MANIFEST" >/dev/null; }
  for p in "${PLATFORMS[@]}"; do
    if [[ "$LINUX" == "skip" && "$p" == linux-* ]]; then echo "    $p: skipped (THEUTH_RELEASE_LINUX=skip)"; continue; fi
    files=(); keys=()
    case "$p" in
      linux-*)
        f="theuth_${VERSION}_${p}.tar.gz"
        if exists "$f"; then
          files=("$f"); keys=("cli-$p")
          for ext in AppImage deb rpm; do
            f="theuth_${VERSION}_${p}.${ext}"
            if exists "$f"; then files+=("$f"); [[ "$ext" != AppImage ]] || keys+=("$p"); fi
          done
        fi ;;
      windows-x86_64)
        f="theuth_${VERSION}_windows-x86_64-setup.exe"; if exists "$f"; then files+=("$f"); keys+=(windows-x86_64); fi
        f="theuth_${VERSION}_windows-x86_64.zip";       if exists "$f"; then files+=("$f"); keys+=(cli-windows-x86_64); fi ;;
    esac
    if [[ ${#files[@]} -eq 0 ]]; then echo "    $p: not built"; continue; fi
    why=""   # the first object or manifest key not yet live
    for f in "${files[@]}"; do published "$f" || why="${why:-$f not published yet}"; done
    for k in ${keys[@]+"${keys[@]}"}; do has_key "$k" || why="${why:-$MANIFEST_NAME has no $k}"; done
    if [[ -n "$why" ]]; then echo "    $p: added ($why)"; ADDS+=("$p")
    else echo "    $p: already released"; fi
  done
  if [[ ${#ADDS[@]} -eq 0 ]]; then
    echo
    echo "theuth $VERSION is up to date: nothing to add"
    exit 0
  fi
fi
ADD_LIST="${ADDS[*]+${ADDS[*]}}"; ADD_LIST="${ADD_LIST// /, }"   # "linux-aarch64, windows-x86_64"

gate 6 "dist/ (primary) or staging/$VERSION/ (fallback) holds every Linux tarball${ADD:+ / Windows file of $ADD_LIST}"
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
SKIP=()            # add-arch: already published byte-identical — not re-signed, not re-uploaded
skipped() { local s; for s in ${SKIP[@]+"${SKIP[@]}"}; do [[ "$s" == "$1" ]] && return 0; done; return 1; }
fetch_windows() { # optional on a release (absent → key omitted, one WARN each); add-arch needs at least one
  f="theuth_${VERSION}_windows-x86_64-setup.exe"
  if fetch "$f"; then WIN_SETUP="$f"; else echo "    WARN: no $f — $MANIFEST_NAME gets no windows-x86_64 key" >&2; fi
  f="theuth_${VERSION}_windows-x86_64.zip"
  if fetch "$f"; then WIN_ZIP="$f"; else echo "    WARN: no $f — $MANIFEST_NAME gets no cli-windows-x86_64 key" >&2; fi
}
if [[ -n "$ADD" ]]; then
  for p in "${ADDS[@]}"; do
    case "$p" in
      linux-*)
        f="theuth_${VERSION}_${p}.tar.gz"
        fetch "$f" || fail 6 "$f neither in $DIST/ nor $S3_STAGING/ (just build-remote <host> linux, or just stage-linux on that arch)"
        LINUX_TGZ+=("$f")
        for ext in AppImage deb rpm; do
          f="theuth_${VERSION}_${p}.${ext}"
          if fetch "$f"; then case "$ext" in AppImage) LINUX_APPIMAGE+=("$f") ;; *) LINUX_DESKTOP+=("$f") ;; esac
          elif [[ "$ext" == AppImage ]]; then echo "    WARN: no $f — $MANIFEST_NAME gets no $p key" >&2
          else echo "    WARN: no $f — not added" >&2; fi
        done ;;
      windows-x86_64)
        fetch_windows
        [[ -n "$WIN_SETUP$WIN_ZIP" ]] || fail 6 "neither theuth_${VERSION}_windows-x86_64-setup.exe nor theuth_${VERSION}_windows-x86_64.zip in $DIST/ or $S3_STAGING/ (just build-remote <win-host> windows)" ;;
    esac
  done
  # Everything add-arch publishes; signed payloads (.tar.gz, .AppImage, .exe, .zip) also get a .sig.
  NEW_FILES=(${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"} ${LINUX_APPIMAGE[@]+"${LINUX_APPIMAGE[@]}"} ${LINUX_DESKTOP[@]+"${LINUX_DESKTOP[@]}"} ${WIN_SETUP:+"$WIN_SETUP"} ${WIN_ZIP:+"$WIN_ZIP"})
  # Objects under <ver>/ are immutable: refuse anything already there unless byte-identical (then skip it).
  for f in ${NEW_FILES[@]+"${NEW_FILES[@]}"}; do
    if ! published "$f"; then
      ! published "$f.sig" || fail 6 "$f.sig is published without $f — remove it by hand first"
      continue
    fi
    curl -fsSL --retry 3 -o "$WORK/$f" "$PUB_VER/$f"
    cmp -s "$WORK/$f" "$DIST/$f" || fail 6 "$f is already published with different bytes — objects under $S3_VER/ are immutable"
    case "$f" in *.tar.gz | *.AppImage | *.exe | *.zip)
      published "$f.sig" || fail 6 "$f is published without $f.sig — remove it by hand first"
      curl -fsSL --retry 3 -o "$DIST/$f.sig" "$PUB_VER/$f.sig"   # the manifest carries the published signature
      ;;
    esac
    echo "    $f already published, byte-identical — its upload is skipped"
    SKIP+=("$f")
  done
elif [[ "$LINUX" == "skip" ]]; then
  echo "    THEUTH_RELEASE_LINUX=skip: manifest and formula without Linux"
else
  if [[ "$LINUX" == "require" ]]; then
    for f in "${LINUX_TGZ[@]}"; do
      fetch "$f" || fail 6 "$f neither in $DIST/ nor $S3_STAGING/ (just build-remote <host> linux, just stage-linux on that arch, or THEUTH_RELEASE_LINUX=present|skip)"
    done
  else
    for arch in "${LINUX_ARCHES[@]}"; do
      f="theuth_${VERSION}_linux-${arch}.tar.gz"
      if fetch "$f"; then LINUX_TGZ+=("$f"); else echo "    WARN: no $f — cli-linux-${arch} omitted from latest.json and the formula" >&2; fi
    done
    [[ ${#LINUX_TGZ[@]} -gt 0 ]] || fail 6 "no theuth_${VERSION}_linux-*.tar.gz in $DIST/ or $S3_STAGING/ (just build-remote <host> linux, or THEUTH_RELEASE_LINUX=skip)"
  fi
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
[[ -n "$ADD" ]] || fetch_windows   # just build-remote <host> windows

gate 7 "each Linux tarball runs in a clean ubuntu:24.04 of its arch"
if [[ ${#LINUX_TGZ[@]} -eq 0 ]]; then   # THEUTH_RELEASE_LINUX=skip, or a Windows-only add
  echo "    skipped (no Linux tarballs)"
elif [[ "${THEUTH_SKIP_LINUX_SMOKE:-0}" == "1" ]]; then
  echo "    WARNING: THEUTH_SKIP_LINUX_SMOKE=1 — Linux tarballs are NOT smoke-tested (glibc/shared-lib breakage would reach users)" >&2
elif ! command -v docker >/dev/null 2>&1; then
  fail 7 "docker not found — install Docker Desktop, or THEUTH_SKIP_LINUX_SMOKE=1 to release without the clean-ubuntu smoke test"
else
  for f in "${LINUX_TGZ[@]}"; do
    case "$f" in *linux-x86_64*) plat=linux/amd64 ;; *) plat=linux/arm64 ;; esac
    # `t` is the whole harness binary, not a CLI-only one: on Linux it links the GTK stack (the .deb's
    # Depends — libwebkit2gtk-4.1-0, libgtk-3-0 — pull the same libraries in), so a bare ubuntu:24.04
    # image fails in the loader before the binary runs. Install the libraries the loader names, not the
    # whole webkit stack: the gate still tests glibc, the tarball layout and the binary itself.
    SMOKE_DEPS='apt-get update -qq >/dev/null 2>&1 && apt-get install -y -qq --no-install-recommends libgbm1 libwayland-client0 libxcb1 libasound2t64 libpipewire-0.3-0 >/dev/null 2>&1 && '
    out="$(docker run --rm --platform "$plat" -v "$PWD/$DIST:/d:ro" ubuntu:24.04 \
      bash -c "${SMOKE_DEPS}tar xzf /d/$f -C /tmp && /tmp/t --version && echo wants=\$(/tmp/t --app-parser-hash) && /tmp/$HELPER --version && printf '' | /tmp/$HELPER" 2>&1)" || fail 7 "$f: $out"
    [[ "$out" == *"$VERSION"* ]] || fail 7 "$f printed '$out', expected $VERSION"
    [[ "$out" == *"wants=$HELPER_HASH"* ]] || fail 7 "$f: t --app-parser-hash does not name $HELPER_HASH: '$out'"
    [[ "$out" == *"theuth-app-parser $HELPER_HASH"* ]] || fail 7 "$f: $HELPER --version printed '$out', expected 'theuth-app-parser $HELPER_HASH'"
    echo "    $f: $(tr '\n' ' ' <<<"$out")($plat)"
  done
fi

CLI_TGZS=(${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"}); [[ -n "$ADD" ]] || CLI_TGZS=("$CLI_TGZ" ${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"})
gate 8 "every CLI tarball holds exactly t + $HELPER (the Windows zip: the same two .exe); the macOS helper runs here"
for f in ${CLI_TGZS[@]+"${CLI_TGZS[@]}"}; do
  listed="$(tar -tzf "$DIST/$f" | sed 's|^\./||' | sort | tr '\n' ' ')"
  [[ "$listed" == "t $HELPER " ]] || fail 8 "$f lists '$listed', expected 't $HELPER' (stale dist/ or staging from another library hash)"
  echo "    $f: t $HELPER"
done
if [[ -n "$WIN_ZIP" ]]; then
  need unzip
  listed="$(unzip -Z1 "$DIST/$WIN_ZIP" | sed 's|^\./||' | sort | tr '\n' ' ')"
  [[ "$listed" == "t.exe $HELPER.exe " ]] || fail 8 "$WIN_ZIP lists '$listed', expected 't.exe $HELPER.exe' (stale dist/ from another library hash)"
  echo "    $WIN_ZIP: t.exe $HELPER.exe"
fi
if [[ -n "$ADD" ]]; then
  echo "    $ADD_LIST: foreign platforms — t --version / --app-parser-hash not run on this host (gate 7 covers Linux in Docker)"
else
  mkdir -p "$WORK/cli" && tar -xzf "$DIST/$CLI_TGZ" -C "$WORK/cli"
  check_helper "$WORK/cli/$HELPER"
  [[ "$("$WORK/cli/t" --version 2>&1)" == *"$VERSION"* ]] || fail 8 "$CLI_TGZ: t --version did not print $VERSION"
  [[ "$("$WORK/cli/t" --app-parser-hash 2>&1)" == "$HELPER_HASH" ]] || fail 8 "$CLI_TGZ: t --app-parser-hash does not name the shipped $HELPER (the updater refuses such a payload)"
fi

# ── sign + sums ──────────────────────────────────────────────────────────────
# Every updater payload gets a minisign .sig: tarballs, AppImages, the NSIS installer, the Windows CLI zip.
if [[ -n "$ADD" ]]; then
  SIGNED=()
  for f in ${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"} ${LINUX_APPIMAGE[@]+"${LINUX_APPIMAGE[@]}"} ${WIN_SETUP:+"$WIN_SETUP"} ${WIN_ZIP:+"$WIN_ZIP"}; do skipped "$f" || SIGNED+=("$f"); done
else
  SIGNED=("$APP_TGZ" "$CLI_TGZ" ${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"} ${LINUX_APPIMAGE[@]+"${LINUX_APPIMAGE[@]}"})
  [[ -z "$WIN_SETUP" ]] || SIGNED+=("$WIN_SETUP")
  [[ -z "$WIN_ZIP" ]] || SIGNED+=("$WIN_ZIP")
fi
sha_of() { awk -v f="$1" '$2 == f {print $1}' "$DIST/SHA256SUMS"; }
s3_cp() { # src dest — content type from the destination key; DRY_RUN prints
  local ct
  case "$2" in
    *.json) ct=application/json ;; *.dmg) ct=application/x-apple-diskimage ;; *.exe) ct=application/vnd.microsoft.portable-executable ;;
    *.tar.gz) ct=application/gzip ;; *.zip) ct=application/zip ;; *.sig | *.deb | *.rpm | *.AppImage) ct=application/octet-stream ;; *) ct=text/plain ;;
  esac
  if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: aws s3 cp $1 $2"; return 0; fi
  # rustfs stalls the first byte of a large part past the CLI's 60 s default read timeout
  # (AWS_ERROR_HTTP_RESPONSE_FIRST_BYTE_TIMEOUT on the 490 MB .dmg): give it a longer window and
  # retry the object, so one stalled part cannot leave <ver>/ holding half a release.
  local n=1
  until "${AWS[@]}" s3 cp "$1" "$2" --content-type "$ct" \
      --cli-read-timeout 300 --cli-connect-timeout 30 >/dev/null; do
    n=$(( n + 1 ))
    [[ $n -le 3 ]] || { echo "error: $2: upload failed after 3 attempts" >&2; return 1; }
    echo "    retry $n: $2" >&2; sleep $(( n * 5 ))
  done
  echo "    $2"
}

if [[ "$RESUME" == "1" ]]; then
  echo "==> resume: sign skipped; only what $S3_VER/ is missing is uploaded"
  for f in ${SIGNED[@]+"${SIGNED[@]}"}; do
    [[ -s "$DIST/$f.sig" ]] || { echo "error: $DIST/$f.sig missing — cannot resume" >&2; exit 1; }
  done
  [[ -s "$DIST/SHA256SUMS" ]] || { echo "error: $DIST/SHA256SUMS missing — cannot resume" >&2; exit 1; }
  # The object set a finished release holds. Published objects are never rewritten (the prefix stays
  # immutable); a missing one — the .dmg the timeout killed, its SHA256SUMS, every .sig after it — goes up.
  RESUME_SET=(${SIGNED[@]+"${SIGNED[@]}"} "$DMG" ${LINUX_DESKTOP[@]+"${LINUX_DESKTOP[@]}"} SHA256SUMS)
  for f in ${SIGNED[@]+"${SIGNED[@]}"}; do RESUME_SET+=("$f.sig"); done
  MISSING=()
  for f in "${RESUME_SET[@]}"; do
    if published "$f"; then echo "    present $f"; else MISSING+=("$f"); fi
  done
  if [[ ${#MISSING[@]} -gt 0 ]]; then
    echo "==> upload ${#MISSING[@]} missing object(s) → $S3_VER/"
    for f in "${MISSING[@]}"; do s3_cp "$DIST/$f" "$S3_VER/$f"; done
  fi
  UPLOADS=(${RESUME_SET[@]+"${RESUME_SET[@]}"})   # the summary counts the release's object set
else
  echo "==> signing ${#SIGNED[@]} payloads (cargo tauri signer sign)"
  for f in ${SIGNED[@]+"${SIGNED[@]}"}; do
    rm -f "$DIST/$f.sig"
    cargo tauri signer sign "$DIST/$f" >/dev/null
    [[ -s "$DIST/$f.sig" ]] || { echo "error: $DIST/$f.sig not produced" >&2; exit 1; }
    echo "    $f.sig"
  done

  echo "==> SHA256SUMS"
  if [[ -n "$ADD" ]]; then
    # Append-only: the live file plus one line per new-arch file it does not list yet (a skipped
    # payload whose line is missing — SHA256SUMS upload died last time — gets its line now).
    curl -fsSL --retry 3 -o "$DIST/SHA256SUMS" "$PUB_VER/SHA256SUMS" || { echo "error: cannot read $PUB_VER/SHA256SUMS" >&2; exit 1; }
    SUM=()
    for f in ${NEW_FILES[@]+"${NEW_FILES[@]}"}; do
      [[ -n "$(sha_of "$f")" ]] || SUM+=("$f")
    done
    [[ ${#SUM[@]} -eq 0 ]] || (cd "$DIST" && shasum -a 256 "${SUM[@]}" >>SHA256SUMS)
    cat "$DIST/SHA256SUMS"
    UPLOADS=(${SIGNED[@]+"${SIGNED[@]}"})
    for f in ${LINUX_DESKTOP[@]+"${LINUX_DESKTOP[@]}"}; do skipped "$f" || UPLOADS+=("$f"); done
    UPLOADS+=(SHA256SUMS)
  else
    (cd "$DIST" && shasum -a 256 "${SIGNED[@]}" "$DMG" ${LINUX_DESKTOP[@]+"${LINUX_DESKTOP[@]}"} >SHA256SUMS && cat SHA256SUMS)
    UPLOADS=("${SIGNED[@]}" "$DMG" ${LINUX_DESKTOP[@]+"${LINUX_DESKTOP[@]}"} SHA256SUMS)
  fi
  echo "==> upload → $S3_VER/"
  for f in ${SIGNED[@]+"${SIGNED[@]}"}; do UPLOADS+=("$f.sig"); done
  for f in "${UPLOADS[@]}"; do s3_cp "$DIST/$f" "$S3_VER/$f"; done
fi

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
  for f in ${SIGNED[@]+"${SIGNED[@]}"}; do
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
if [[ -n "$ADD" ]]; then
  # The live manifest (gate 5): version, notes and every other platform stay as they are; jq round-trips them.
  MANIFEST="$LIVE_MANIFEST"
else
  MANIFEST="$(jq -n --arg v "$VERSION" '{version: $v, platforms: {}}')"
fi
platform() { # key file
  MANIFEST="$(jq --arg k "$1" --arg u "$PUB_VER/$2" --arg s "$(cat "$DIST/$2.sig")" \
    '.platforms[$k] = {url: $u, signature: $s}' <<<"$MANIFEST")"
}
if [[ -z "$ADD" ]]; then
  platform darwin-aarch64 "$APP_TGZ"
  platform cli-macos-aarch64 "$CLI_TGZ"
fi
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
if [[ -n "$ADD" ]]; then
  echo "==> $S3_STAGING/ left in place (add-arch; aws s3 rm --recursive it by hand once every arch is in)"
elif [[ "$LINUX" != "skip" ]]; then
  echo "==> delete $S3_STAGING/"
  if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: aws s3 rm --recursive $S3_STAGING/"
  else "${AWS[@]}" s3 rm --recursive "$S3_STAGING/" >/dev/null; fi
fi

# ── tap bump (§5.3) ──────────────────────────────────────────────────────────
# Rendered files always land in dist/tap/ so a failed audit or a dry run leaves something to inspect.
# has_linux_tgz <arch>: that arch's CLI tarball is in SHA256SUMS — what ships (release) or what is
# published plus what is being added (add-arch reads the merged live file). No Linux tarball at all
# (THEUTH_RELEASE_LINUX=skip, or adding to a macOS-only release) → no on_linux block.
has_linux_tgz() { [[ -n "$(sha_of "theuth_${VERSION}_linux-$1.tar.gz")" ]]; }
render() { # tmpl out
  sed -e "s|@VERSION@|$VERSION|g" -e "s|@BASE_URL@|$PUBLIC_BASE|g" \
      -e "s|@SHA_MACOS_CLI@|$(sha_of "$CLI_TGZ")|g" -e "s|@SHA_DMG@|$(sha_of "$DMG")|g" \
      -e "s|@SHA_LINUX_X86@|$(sha_of "theuth_${VERSION}_linux-x86_64.tar.gz")|g" \
      -e "s|@SHA_LINUX_ARM@|$(sha_of "theuth_${VERSION}_linux-aarch64.tar.gz")|g" "$1" \
    | if has_linux_tgz aarch64 || has_linux_tgz x86_64; then sed -e '/^# @LINUX_BEGIN@$/d' -e '/^# @LINUX_END@$/d'
      else sed '/^# @LINUX_BEGIN@$/,/^# @LINUX_END@$/d'; fi \
    | if has_linux_tgz aarch64; then sed -e '/^# @LINUX_ARM_BEGIN@$/d' -e '/^# @LINUX_ARM_END@$/d'
      else sed '/^# @LINUX_ARM_BEGIN@$/,/^# @LINUX_ARM_END@$/d'; fi \
    | if has_linux_tgz x86_64; then sed -e '/^# @LINUX_X86_BEGIN@$/d' -e '/^# @LINUX_X86_END@$/d'
      else sed '/^# @LINUX_X86_BEGIN@$/,/^# @LINUX_X86_END@$/d'; fi >"$2"
}
mkdir -p "$DIST/tap/Formula" "$DIST/tap/Casks"
render scripts/tap/theuth.rb.formula.tmpl "$DIST/tap/Formula/theuth.rb"
render scripts/tap/theuth.rb.cask.tmpl "$DIST/tap/Casks/theuth.rb"

TAP_MSG="theuth $VERSION${ADD:+: add $ADD_LIST}"
TAP_STATUS="not bumped (prerelease); rendered at $DIST/tap/"
if [[ "$PRERELEASE" != "1" ]]; then
  echo "==> tap bump: $TAP_REPO"
  TAP="$WORK/tap"
  git clone --quiet --depth 1 "$TAP_REPO" "$TAP"
  mkdir -p "$TAP/Formula" "$TAP/Casks"
  cp "$DIST/tap/Formula/theuth.rb" "$TAP/Formula/theuth.rb"
  cp "$DIST/tap/Casks/theuth.rb" "$TAP/Casks/theuth.rb"
  # Nothing to commit when the rendered files equal what is live (add-arch of platforms the tap does
  # not mention, e.g. Windows): `git commit` with nothing staged would fail after everything is published.
  TAP_CHANGED=1; [[ -n "$(git -C "$TAP" status --porcelain)" ]] || TAP_CHANGED=0
  AUDIT_OK=1
  if [[ "$TAP_CHANGED" == 0 ]]; then
    echo "    unchanged: the formula and cask do not mention $ADD_LIST — nothing to commit"
  elif command -v brew >/dev/null 2>&1; then
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
  if [[ "$TAP_CHANGED" == 0 ]]; then
    TAP_STATUS="unchanged (the formula and cask do not mention the added platforms)"
  elif [[ "$AUDIT_OK" != "1" ]]; then
    # §5.3: a failed audit leaves the tap untouched; the release is otherwise complete.
    echo "    WARNING: brew audit failed — tap NOT updated; fix and commit $DIST/tap/ by hand" >&2
    TAP_STATUS="NOT bumped (brew audit failed); rendered at $DIST/tap/"
  elif [[ "$DRY_RUN" == "1" ]]; then
    echo "    DRY_RUN: git commit -m '$TAP_MSG' && git push"
    TAP_STATUS="not pushed (dry run); rendered at $DIST/tap/"
  else
    (cd "$TAP" && git add Formula/theuth.rb Casks/theuth.rb && git commit --quiet -m "$TAP_MSG" && git push --quiet)
    TAP_STATUS="pushed $TAP_MSG"
    # The release Mac's brew checkout still has the previous formula until this.
    if [[ -n "${INSTALLED:-}" ]]; then
      git -C "$INSTALLED" pull --ff-only --quiet || echo "    installed tap not fast-forwarded; brew update will pick it up"
    fi
  fi
fi

# ── summary ──────────────────────────────────────────────────────────────────
echo
echo "done: ${TAP_MSG}"
echo "  objects:   $S3_VER/  (${#UPLOADS[@]} files)"
echo "  manifest:  ${PUBLIC_BASE}/${MANIFEST_NAME}"
echo "  platforms: $(jq -r '.platforms | keys | join(", ")' "$DIST/$MANIFEST_NAME")"
echo "  dmg:       $PUB_VER/$DMG"
echo "  tap:       $TAP_STATUS"
[[ "$DRY_RUN" != "1" ]] || echo "  DRY RUN: nothing was written to the bucket or the tap"
