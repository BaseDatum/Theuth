#!/usr/bin/env bash
# Publish a wth release to the update bucket and bump the Homebrew tap (39 §5).
#
# Pipeline (after package-macos, notarize-macos, package-update-macos; Linux tarballs
# already staged by `just stage-linux` on each Linux build host):
#   gates 1-8 (§5.2)     version == Info.plist, Developer ID (app + wth-app-parser helper), notarized,
#                        pubkeys match, <ver>/ absent (immutability), staging/<ver>/ complete, each Linux
#                        tarball prints VERSION in a clean ubuntu:22.04 of its arch and its helper answers
#                        `--version` with the library hash, every CLI tarball holds exactly
#                        `wth` + `wth-app-parser-<hash>` (46 §2.1; scripts/app-parser-env.sh)
#   pull staging/<ver>/  → dist/
#   sign                 cargo tauri signer sign  dist/wth_<ver>_*.tar.gz  → .sig
#   SHA256SUMS           shasum -a 256 over tarballs + DMG
#   upload               dist/wth_<ver>_* + SHA256SUMS → s3://$BUCKET/$PREFIX/<ver>/
#   verify               re-download every tarball + .sig via the PUBLIC url; minisign -V
#                        against crates/update/pubkey.txt (sha256 vs SHA256SUMS when minisign
#                        is not installed — cargo tauri signer has no verify subcommand)
#   latest.json          jq; {version, platforms: {darwin-aarch64, cli-macos-aarch64,
#                        cli-linux-x86_64, cli-linux-aarch64}} — written last
#   delete staging/<ver>/
#   tap bump (§5.3)      clone $WTH_TAP_REPO, render Formula/wth.rb + Casks/wth.rb, brew audit,
#                        commit "wth <ver>", push
#
# dist/ = target/release/dist (package-update-macos.sh). Objects under <ver>/ are never
# re-uploaded; latest.json is the only mutable object.
#
# Env (defaults for rustfs @ s3.rwx.dev; repo-root .env.signing is loaded):
#   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
#   WTH_S3_ENDPOINT          API endpoint (default: https://s3.rwx.dev)
#   WTH_S3_BUCKET            bucket (default: wth)
#   WTH_S3_PREFIX            key prefix (default: releases)
#   WTH_S3_PUBLIC_BASE_URL   public base for downloads (default: ${ENDPOINT}/${BUCKET}/${PREFIX})
#   WTH_UPDATE_KEY           minisign private key: path (~/.tauri/wth.key) or base64 contents
#   WTH_UPDATE_KEY_PASSWORD  its password (empty for --ci keys)
#   WTH_RELEASE_LINUX        require | skip (default: require). skip = manifest without
#                            cli-linux-* keys, formula without on_linux (macOS-only hotfix)
#   WTH_RELEASE_PRERELEASE=1 ad-hoc/unnotarized allowed; writes latest-prerelease.json only,
#                            never latest.json, never bumps the tap
#   WTH_TAP_REPO             tap git url (default: git@github.com:skymoore/homebrew-tap)
#   WTH_S3_DRY_RUN=1         no writes: uploads, staging delete and tap push are printed
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
[[ -n "${WTH_UPDATE_KEY:-}" ]] \
  || { echo "error: WTH_UPDATE_KEY unset (cargo tauri signer generate -w ~/.tauri/wth.key; see .env.signing.example)" >&2; exit 1; }

ENDPOINT="${WTH_S3_ENDPOINT:-https://s3.rwx.dev}"; ENDPOINT="${ENDPOINT%/}"
BUCKET="${WTH_S3_BUCKET:-wth}"
PREFIX="${WTH_S3_PREFIX:-releases}"; PREFIX="${PREFIX#/}"; PREFIX="${PREFIX%/}"
PUBLIC_BASE="${WTH_S3_PUBLIC_BASE_URL:-${ENDPOINT}/${BUCKET}/${PREFIX}}"; PUBLIC_BASE="${PUBLIC_BASE%/}"
LINUX="${WTH_RELEASE_LINUX:-require}"
PRERELEASE="${WTH_RELEASE_PRERELEASE:-0}"
TAP_REPO="${WTH_TAP_REPO:-git@github.com:skymoore/homebrew-tap}"
DRY_RUN="${WTH_S3_DRY_RUN:-0}"
case "$LINUX" in require | skip) ;; *) echo "error: WTH_RELEASE_LINUX must be require|skip" >&2; exit 1 ;; esac

# rustfs: path-style, no mandatory checksums, no IMDS probe (jarvis release-s3.sh:164-174).
export AWS_S3_FORCE_PATH_STYLE=true
export AWS_REQUEST_CHECKSUM_CALCULATION="${AWS_REQUEST_CHECKSUM_CALCULATION:-when_required}"
export AWS_RESPONSE_CHECKSUM_VALIDATION="${AWS_RESPONSE_CHECKSUM_VALIDATION:-when_required}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"
export AWS_EC2_METADATA_DISABLED=true
AWS=(aws --endpoint-url "$ENDPOINT")

# Signing key → the env the tauri CLI reads (path or contents). An unset password makes
# the CLI prompt on a TTY and die in a script, so it is always exported.
if [[ -f "$WTH_UPDATE_KEY" ]]; then
  export TAURI_SIGNING_PRIVATE_KEY_PATH="$WTH_UPDATE_KEY"; unset TAURI_SIGNING_PRIVATE_KEY
else
  export TAURI_SIGNING_PRIVATE_KEY="$WTH_UPDATE_KEY"; unset TAURI_SIGNING_PRIVATE_KEY_PATH
fi
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${WTH_UPDATE_KEY_PASSWORD-}"

VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="wth") | .version')"
KEY="$(uname -s | tr '[:upper:]' '[:lower:]' | sed s/darwin/macos/)-$(uname -m | sed s/arm64/aarch64/)"
[[ "$KEY" == "macos-aarch64" ]] || { echo "error: release runs on the arm64 release Mac (got $KEY)" >&2; exit 1; }
DIST="target/release/dist"
APP="target/release/bundle/macos/wth.app"
PUBKEY_FILE="crates/update/pubkey.txt"
PUBKEY="$(head -n1 "$PUBKEY_FILE")"
S3_VER="s3://${BUCKET}/${PREFIX}/${VERSION}"
S3_STAGING="s3://${BUCKET}/staging/${VERSION}"
PUB_VER="${PUBLIC_BASE}/${VERSION}"
APP_TGZ="wth_${VERSION}_${KEY}.app.tar.gz"
CLI_TGZ="wth_${VERSION}_${KEY}.tar.gz"
DMG="wth_${VERSION}_${KEY}.dmg"
LINUX_TGZ=()
[[ "$LINUX" == "skip" ]] || LINUX_TGZ=("wth_${VERSION}_linux-x86_64.tar.gz" "wth_${VERSION}_linux-aarch64.tar.gz")
MANIFEST_NAME="latest.json"; [[ "$PRERELEASE" == "1" ]] && MANIFEST_NAME="latest-prerelease.json"

for f in "$APP_TGZ" "$CLI_TGZ" "$DMG"; do
  [[ -f "$DIST/$f" ]] || { echo "error: $DIST/$f missing — run just package-update-macos" >&2; exit 1; }
done
# Tauri's pubkey encoding is base64 of the whole .pub file, so it starts with "untrusted comment:" encoded.
[[ "$PUBKEY" == dW50cnVzdGVk* ]] || { echo "error: $PUBKEY_FILE is not a Tauri minisign public key (cargo tauri signer generate -w ~/.tauri/wth.key)" >&2; exit 1; }

echo "==> release wth $VERSION"
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
  echo "==> gates 2-3 skipped (WTH_RELEASE_PRERELEASE=1)"
else
  gate 2 "codesign shows a Developer ID (app and $HELPER)"
  codesign -dv "$APP" 2>&1 | grep -q 'Authority=Developer ID' || fail 2 "ad-hoc build (set WTH_SIGN_IDENTITY, rerun just package-macos)"
  codesign -dv "$APP/Contents/MacOS/$HELPER" 2>&1 | grep -q 'Authority=Developer ID' || fail 2 "$HELPER not signed with a Developer ID (rerun just package-macos)"
  codesign -d --entitlements - --xml "$APP/Contents/MacOS/$HELPER" 2>/dev/null | grep -q 'com.apple.security.cs.allow-jit' || fail 2 "$HELPER lacks the JIT entitlement (scripts/entitlements-app-parser.plist)"
  gate 3 "spctl --assess (notarized)"
  spctl --assess --type execute "$APP" 2>/dev/null || fail 3 "not notarized (just notarize-macos)"
fi

gate 4 "pubkey.txt == tauri.conf.json plugins.updater.pubkey"
CONF_PUBKEY="$(jq -r '.plugins.updater.pubkey // empty' crates/desktop/tauri.conf.json)"
[[ "$CONF_PUBKEY" == "$PUBKEY" ]] || fail 4 "the two pubkey copies diverged"

gate 5 "$S3_VER/ does not exist"
if "${AWS[@]}" s3 ls "$S3_VER/" >/dev/null 2>&1; then
  fail 5 "$S3_VER/ already published; objects under <ver>/ are immutable — bump the version"
fi

gate 6 "staging/$VERSION/ holds every Linux tarball"
if [[ "$LINUX" == "skip" ]]; then
  echo "    WTH_RELEASE_LINUX=skip: manifest and formula without Linux"
else
  for f in "${LINUX_TGZ[@]}"; do
    "${AWS[@]}" s3 ls "$S3_STAGING/$f" >/dev/null 2>&1 || fail 6 "$S3_STAGING/$f missing (just stage-linux on that arch, or WTH_RELEASE_LINUX=skip)"
    echo "    pull $f"
    "${AWS[@]}" s3 cp "$S3_STAGING/$f" "$DIST/$f" >/dev/null
  done
fi

gate 7 "each Linux tarball runs in a clean ubuntu:22.04 of its arch"
if [[ "$LINUX" == "skip" ]]; then
  echo "    skipped (no Linux tarballs)"
elif ! command -v docker >/dev/null 2>&1; then
  echo "    WARNING: docker not found — Linux tarballs are NOT smoke-tested (glibc/shared-lib breakage would reach users)" >&2
else
  for f in "${LINUX_TGZ[@]}"; do
    case "$f" in *linux-x86_64*) plat=linux/amd64 ;; *) plat=linux/arm64 ;; esac
    out="$(docker run --rm --platform "$plat" -v "$PWD/$DIST:/d:ro" ubuntu:22.04 \
      bash -c "tar xzf /d/$f -C /tmp && /tmp/wth --version && echo wants=\$(/tmp/wth --app-parser-hash) && /tmp/$HELPER --version && printf '' | /tmp/$HELPER" 2>&1)" || fail 7 "$f: $out"
    [[ "$out" == *"$VERSION"* ]] || fail 7 "$f printed '$out', expected $VERSION"
    [[ "$out" == *"wants=$HELPER_HASH"* ]] || fail 7 "$f: wth --app-parser-hash does not name $HELPER_HASH: '$out'"
    [[ "$out" == *"wth-app-parser $HELPER_HASH"* ]] || fail 7 "$f: $HELPER --version printed '$out', expected 'wth-app-parser $HELPER_HASH'"
    echo "    $f: $(tr '\n' ' ' <<<"$out")($plat)"
  done
fi

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

gate 8 "every CLI tarball holds exactly wth + $HELPER; the macOS helper runs here"
for f in "$CLI_TGZ" ${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"}; do
  listed="$(tar -tzf "$DIST/$f" | sed 's|^\./||' | sort | tr '\n' ' ')"
  [[ "$listed" == "wth $HELPER " ]] || fail 8 "$f lists '$listed', expected 'wth $HELPER' (stale dist/ or staging from another library hash)"
  echo "    $f: wth $HELPER"
done
mkdir -p "$WORK/cli" && tar -xzf "$DIST/$CLI_TGZ" -C "$WORK/cli"
check_helper "$WORK/cli/$HELPER"
[[ "$("$WORK/cli/wth" --version 2>&1)" == *"$VERSION"* ]] || fail 8 "$CLI_TGZ: wth --version did not print $VERSION"
[[ "$("$WORK/cli/wth" --app-parser-hash 2>&1)" == "$HELPER_HASH" ]] || fail 8 "$CLI_TGZ: wth --app-parser-hash does not name the shipped $HELPER (the updater refuses such a payload)"

# ── sign + sums ──────────────────────────────────────────────────────────────
TARBALLS=("$APP_TGZ" "$CLI_TGZ" ${LINUX_TGZ[@]+"${LINUX_TGZ[@]}"})
echo "==> signing ${#TARBALLS[@]} tarballs (cargo tauri signer sign)"
for f in "${TARBALLS[@]}"; do
  rm -f "$DIST/$f.sig"
  cargo tauri signer sign "$DIST/$f" >/dev/null
  [[ -s "$DIST/$f.sig" ]] || { echo "error: $DIST/$f.sig not produced" >&2; exit 1; }
  echo "    $f.sig"
done

echo "==> SHA256SUMS"
(cd "$DIST" && shasum -a 256 "${TARBALLS[@]}" "$DMG" >SHA256SUMS && cat SHA256SUMS)
sha_of() { awk -v f="$1" '$2 == f {print $1}' "$DIST/SHA256SUMS"; }

# ── upload ───────────────────────────────────────────────────────────────────
s3_cp() { # src dest — content type from the destination key; DRY_RUN prints
  local ct
  case "$2" in
    *.json) ct=application/json ;; *.dmg) ct=application/x-apple-diskimage ;;
    *.tar.gz) ct=application/gzip ;; *.sig) ct=application/octet-stream ;; *) ct=text/plain ;;
  esac
  if [[ "$DRY_RUN" == "1" ]]; then echo "    DRY_RUN: aws s3 cp $1 $2"; return 0; fi
  "${AWS[@]}" s3 cp "$1" "$2" --content-type "$ct" >/dev/null
  echo "    $2"
}

echo "==> upload → $S3_VER/"
UPLOADS=("${TARBALLS[@]}" "$DMG" SHA256SUMS)
for f in "${TARBALLS[@]}"; do UPLOADS+=("$f.sig"); done
for f in "${UPLOADS[@]}"; do s3_cp "$DIST/$f" "$S3_VER/$f"; done

TMP="$WORK/dl"; mkdir -p "$TMP"

# ── verify what the client will fetch ────────────────────────────────────────
echo "==> verify via $PUB_VER/"
if [[ "$DRY_RUN" == "1" ]]; then
  echo "    DRY_RUN: nothing uploaded, nothing to verify"
else
  if command -v minisign >/dev/null 2>&1; then
    # pubkey.txt / .sig are Tauri's base64-of-the-minisign-file; minisign wants the raw key line.
    RAW_PUB="$(base64 -d "$PUBKEY_FILE" | sed -n 2p)"
  else
    echo "    minisign not installed (brew install minisign): comparing sha256 with SHA256SUMS and .sig bytes with dist/ instead"
  fi
  for f in "${TARBALLS[@]}"; do
    curl -fsSL --retry 3 -o "$TMP/$f" "$PUB_VER/$f"
    curl -fsSL --retry 3 -o "$TMP/$f.sig" "$PUB_VER/$f.sig"
    if [[ -n "${RAW_PUB:-}" ]]; then
      base64 -d "$TMP/$f.sig" >"$TMP/$f.minisig"
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
      -e "s|@SHA_LINUX_X86@|$(sha_of "wth_${VERSION}_linux-x86_64.tar.gz")|g" \
      -e "s|@SHA_LINUX_ARM@|$(sha_of "wth_${VERSION}_linux-aarch64.tar.gz")|g" "$1" \
    | if [[ "$LINUX" == "skip" ]]; then sed '/^# @LINUX_BEGIN@$/,/^# @LINUX_END@$/d'
      else sed -e '/^# @LINUX_BEGIN@$/d' -e '/^# @LINUX_END@$/d'; fi >"$2"
}
mkdir -p "$DIST/tap/Formula" "$DIST/tap/Casks"
render scripts/tap/wth.rb.formula.tmpl "$DIST/tap/Formula/wth.rb"
render scripts/tap/wth.rb.cask.tmpl "$DIST/tap/Casks/wth.rb"

TAP_STATUS="not bumped (prerelease); rendered at $DIST/tap/"
if [[ "$PRERELEASE" != "1" ]]; then
  echo "==> tap bump: $TAP_REPO"
  TAP="$WORK/tap"
  git clone --quiet --depth 1 "$TAP_REPO" "$TAP"
  mkdir -p "$TAP/Formula" "$TAP/Casks"
  cp "$DIST/tap/Formula/wth.rb" "$TAP/Formula/wth.rb"
  cp "$DIST/tap/Casks/wth.rb" "$TAP/Casks/wth.rb"
  AUDIT_OK=1
  if command -v brew >/dev/null 2>&1; then
    (cd "$TAP" && brew audit --strict --formula Formula/wth.rb && brew audit --cask Casks/wth.rb) || AUDIT_OK=0
  else
    echo "    brew not found: audit skipped"
  fi
  if [[ "$AUDIT_OK" != "1" ]]; then
    # §5.3: a failed audit leaves the tap untouched; the release is otherwise complete.
    echo "    WARNING: brew audit failed — tap NOT updated; fix and commit $DIST/tap/ by hand" >&2
    TAP_STATUS="NOT bumped (brew audit failed); rendered at $DIST/tap/"
  elif [[ "$DRY_RUN" == "1" ]]; then
    echo "    DRY_RUN: git commit -m 'wth $VERSION' && git push"
    TAP_STATUS="not pushed (dry run); rendered at $DIST/tap/"
  else
    (cd "$TAP" && git add Formula/wth.rb Casks/wth.rb && git commit --quiet -m "wth $VERSION" && git push --quiet)
    TAP_STATUS="pushed wth $VERSION"
  fi
fi

# ── summary ──────────────────────────────────────────────────────────────────
echo
echo "done: wth $VERSION"
echo "  objects:   $S3_VER/  (${#UPLOADS[@]} files)"
echo "  manifest:  ${PUBLIC_BASE}/${MANIFEST_NAME}"
echo "  platforms: $(jq -r '.platforms | keys | join(", ")' "$DIST/$MANIFEST_NAME")"
echo "  dmg:       $PUB_VER/$DMG"
echo "  tap:       $TAP_STATUS"
[[ "$DRY_RUN" != "1" ]] || echo "  DRY RUN: nothing was written to the bucket or the tap"
