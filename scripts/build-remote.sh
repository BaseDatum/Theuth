#!/usr/bin/env bash
# Build the Windows or Linux release artifacts on a remote ssh host and pull them into dist/ (58 step 15).
#
#   scripts/build-remote.sh <user@host> <windows|linux> [--ref <git ref>] [--dist <dir>] [--keep]
#
# 1. ship     git archive --format=tar <ref> → fresh remote ~/theuth-build/<sha>/ (Windows:
#             $env:USERPROFILE\theuth-build\<sha>\). Linux: the tar is piped over ssh; Windows: scp of
#             the tar, then `tar -xf` (bsdtar ships with Windows 10+) — pwsh does not pass binary stdin
#             through to a native command. Nothing is fetched from GitHub; the exact local commit ships.
#             <sha>/target is a symlink / junction to ~/theuth-build/target, so every build shares one
#             cargo target dir and the package scripts' literal `target/release/...` paths keep working.
# 2. check    linux:   scripts/package-linux.sh --check   (HOST_PKGS + cargo, cargo tauri, bun, jq, just)
#             windows: cargo, cargo tauri, pwsh, bun, tar
#             exit 2 lists what is missing; nothing is installed.
# 3. build    linux:   THEUTH_SKIP_UPLOAD=1 scripts/package-linux.sh            → AppImage, deb, rpm
#                      THEUTH_LINUX_NATIVE=1 THEUTH_SKIP_UPLOAD=1 scripts/stage-linux.sh  → CLI tar.gz
#             windows: pwsh -NoProfile -File scripts/package-windows.ps1        → -setup.exe (NSIS), CLI zip
# 4. pull     scp remote target/release/dist/theuth_<ver>_<os>-* → --dist (default target/release/dist,
#             what release-s3.sh reads); print name / size / sha256; exit 1 if an expected artifact is missing
#             (windows: -setup.exe + .zip; linux: .AppImage + .deb + .rpm + .tar.gz).
# 5. clean    rm the remote source dir (--keep leaves it); ~/theuth-build/target stays for the next build.
#
# A dirty working tree is refused unless --ref names a commit/tag (HEAD is the default ref).
# Env: THEUTH_REMOTE_DRY_RUN=1  print every ssh/scp command and remote script instead of running them.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

usage() {
  echo "usage: scripts/build-remote.sh <user@host> <windows|linux> [--ref <git ref>] [--dist <dir>] [--keep]" >&2
  exit 64
}
HOST=""; OS=""; REF=""; DIST="target/release/dist"; KEEP=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ref) [[ $# -ge 2 ]] || usage; REF="$2"; shift 2 ;;
    --dist) [[ $# -ge 2 ]] || usage; DIST="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    -h | --help) usage ;;
    -*) echo "error: unknown option $1" >&2; usage ;;
    *) if [[ -z "$HOST" ]]; then HOST="$1"; elif [[ -z "$OS" ]]; then OS="$1"; else usage; fi; shift ;;
  esac
done
[[ -n "$HOST" && -n "$OS" ]] || usage
case "$OS" in windows | linux) ;; *) echo "error: os must be windows|linux (got $OS)" >&2; usage ;; esac
DRY_RUN="${THEUTH_REMOTE_DRY_RUN:-0}"

need() { command -v "$1" >/dev/null 2>&1 || { echo "error: $1 required${2:+ ($2)}" >&2; exit 1; }; }
need ssh; need scp; need git; need jq; need shasum
[[ "$OS" != "windows" ]] || { need iconv; need base64; }

# ── the commit that ships ────────────────────────────────────────────────────
if [[ -z "$REF" ]]; then
  [[ -z "$(git status --porcelain)" ]] || { echo "error: working tree is dirty — commit first, or build a commit/tag with --ref" >&2; exit 1; }
  REF=HEAD
fi
COMMIT="$(git rev-parse --verify --quiet "$REF^{commit}")" || { echo "error: $REF is not a commit" >&2; exit 1; }
SHA="$(git rev-parse --short=12 "$COMMIT")"
VERSION="$(git show "$COMMIT:crates/desktop/tauri.conf.json" | jq -r .version)"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]] || { echo "error: no version in crates/desktop/tauri.conf.json at $SHA (got '$VERSION')" >&2; exit 1; }
RBASE="theuth-build"          # relative to the remote home (ssh, scp and sftp all resolve it)
RSRC="$RBASE/$SHA"
RDIST="$RBASE/target/release/dist"
PATTERN="theuth_${VERSION}_${OS}-*"

echo "==> build-remote: theuth $VERSION @ $SHA ($REF) on $HOST ($OS) → $DIST"
[[ "$DRY_RUN" != "1" ]] || echo "    THEUTH_REMOTE_DRY_RUN=1: printing commands, running nothing"

# ── runners (DRY_RUN prints) ─────────────────────────────────────────────────
run() { # argv
  if [[ "$DRY_RUN" == "1" ]]; then printf 'DRY_RUN:'; printf ' %q' "$@"; echo; return 0; fi
  "$@"
}
indent() { printf '%s\n' "$1" | while IFS= read -r l; do printf '    | %s\n' "$l"; done; }
run_bash() { # script — runs on the Linux host under `bash -s`
  if [[ "$DRY_RUN" == "1" ]]; then
    echo "DRY_RUN: ssh $HOST bash -s <<'EOF'"; indent "$1"; echo "    EOF"; return 0
  fi
  ssh "$HOST" bash -s <<<"$1"
}
run_pwsh() { # script — runs on the Windows host under pwsh; -EncodedCommand (UTF-16LE base64) sidesteps
  # the quoting of whichever default shell sshd hands the command line to (cmd.exe or powershell.exe).
  local b64
  b64="$(printf '%s' "$1" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')"
  if [[ "$DRY_RUN" == "1" ]]; then
    echo "DRY_RUN: ssh $HOST pwsh -NoProfile -NonInteractive -EncodedCommand <${#b64} chars, base64(UTF-16LE) of:>"
    indent "$1"; return 0
  fi
  ssh "$HOST" pwsh -NoProfile -NonInteractive -EncodedCommand "$b64"
}
step() { # rc-aware wrapper for the check step: 2 = prerequisites missing
  local rc=0
  "$@" || rc=$?
  case "$rc" in
    0) ;;
    2) echo "error: $HOST lacks prerequisites (listed above); install them and rerun — build-remote installs nothing" >&2; exit 2 ;;
    *) echo "error: remote step failed on $HOST (exit $rc)" >&2; exit "$rc" ;;
  esac
}

mkdir -p "$DIST"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

# ── remote scripts ───────────────────────────────────────────────────────────
# Linux: a non-interactive ssh shell sources no rc file, so the user-local tool dirs are added here.
LINUX_ENV="set -euo pipefail
export PATH=\"\$HOME/.cargo/bin:\$HOME/.bun/bin:\$HOME/.local/bin:\$PATH\"
cd \"\$HOME/$RSRC\""
LINUX_CHECK="$LINUX_ENV
scripts/package-linux.sh --check"
LINUX_BUILD="$LINUX_ENV
rm -f target/release/dist/theuth_${VERSION}_*
THEUTH_SKIP_UPLOAD=1 scripts/package-linux.sh
THEUTH_LINUX_NATIVE=1 THEUTH_SKIP_UPLOAD=1 scripts/stage-linux.sh
ls -l target/release/dist/"

# Windows: PowerShell 7. Native exit codes are checked by hand (PSNativeCommandUseErrorActionPreference
# differs between 7.3 and 7.4), the junction is removed with `rmdir` so a recursive delete never
# follows it into the shared target dir.
WIN_ENV="\$ErrorActionPreference = 'Stop'
\$PSNativeCommandUseErrorActionPreference = \$false
\$env:Path = \"\$env:USERPROFILE\\.cargo\\bin;\$env:USERPROFILE\\.bun\\bin;\$env:Path\"
\$base = Join-Path \$env:USERPROFILE '$RBASE'
\$src = Join-Path \$base '$SHA'
function Remove-Source {
  if (Test-Path -LiteralPath \"\$src\\target\") { cmd /c rmdir \"\$src\\target\" | Out-Null }
  if (Test-Path -LiteralPath \$src) { Remove-Item -LiteralPath \$src -Recurse -Force }
}"
WIN_EXTRACT="$WIN_ENV
\$tarball = Join-Path \$env:USERPROFILE '$RBASE-$SHA.tar'
Remove-Source
New-Item -ItemType Directory -Force -Path \$src, (Join-Path \$base 'target') | Out-Null
tar -xf \$tarball -C \$src
if (\$LASTEXITCODE -ne 0) { Write-Error \"tar -xf \$tarball failed (\$LASTEXITCODE)\"; exit 1 }
Remove-Item -LiteralPath \$tarball
New-Item -ItemType Junction -Path (Join-Path \$src 'target') -Target (Join-Path \$base 'target') | Out-Null
Write-Host \"extracted \$src (target -> \$base\\target)\""
WIN_CHECK="$WIN_ENV
\$missing = @()
foreach (\$t in 'cargo', 'pwsh', 'bun', 'tar') { if (-not (Get-Command \$t -ErrorAction SilentlyContinue)) { \$missing += \$t } }
if (\$missing -notcontains 'cargo') { cargo tauri --version *> \$null; if (\$LASTEXITCODE -ne 0) { \$missing += 'cargo-tauri (cargo install tauri-cli)' } }
if (\$missing.Count -gt 0) { Write-Host \"error: missing on \$env:COMPUTERNAME: \$(\$missing -join ', ')\"; exit 2 }
Write-Host \"    prerequisites ok: \$(cargo --version), \$(cargo tauri --version), pwsh \$(\$PSVersionTable.PSVersion), bun \$(bun --version), \$(tar --version | Select-Object -First 1)\"
Set-Location \$src
if (-not (Test-Path -LiteralPath 'scripts\\package-windows.ps1')) { Write-Host 'error: scripts/package-windows.ps1 is not in this commit'; exit 1 }"
WIN_BUILD="$WIN_ENV
Set-Location \$src
Remove-Item -Force -ErrorAction SilentlyContinue -Path (Join-Path \$base 'target\\release\\dist\\theuth_${VERSION}_*')
\$env:THEUTH_SKIP_UPLOAD = '1'
pwsh -NoProfile -File scripts\\package-windows.ps1
if (\$LASTEXITCODE -ne 0) { exit \$LASTEXITCODE }
Get-ChildItem -Path (Join-Path \$base 'target\\release\\dist') | Format-Table Name, Length -AutoSize | Out-String -Width 200"
WIN_CLEAN="$WIN_ENV
Remove-Source
Write-Host \"removed \$src\""

# ── 1. ship ──────────────────────────────────────────────────────────────────
echo "==> [1/5] ship $SHA → $HOST:~/$RSRC"
case "$OS" in
  linux)
    SHIP="rm -rf ~/$RSRC && mkdir -p ~/$RSRC ~/$RBASE/target && cd ~/$RSRC && tar -xf - && ln -s ../target target"
    if [[ "$DRY_RUN" == "1" ]]; then
      echo "DRY_RUN: git archive --format=tar $COMMIT | ssh $HOST '$SHIP'"
    else
      # shellcheck disable=SC2029  # $SHIP is meant to expand here; ~ and the glob-free path are for the remote shell
      git archive --format=tar "$COMMIT" | ssh "$HOST" "$SHIP"
    fi
    ;;
  windows)
    TAR="$WORK/$RBASE-$SHA.tar"
    run git archive --format=tar -o "$TAR" "$COMMIT"
    run scp -q "$TAR" "$HOST:$RBASE-$SHA.tar"
    run_pwsh "$WIN_EXTRACT"
    ;;
esac

# ── 2. check ─────────────────────────────────────────────────────────────────
echo "==> [2/5] prerequisites on $HOST"
case "$OS" in
  linux) step run_bash "$LINUX_CHECK" ;;
  windows) step run_pwsh "$WIN_CHECK" ;;
esac

# ── 3. build ─────────────────────────────────────────────────────────────────
echo "==> [3/5] build on $HOST"
case "$OS" in
  linux) run_bash "$LINUX_BUILD" ;;
  windows) run_pwsh "$WIN_BUILD" ;;
esac

# ── 4. pull ──────────────────────────────────────────────────────────────────
echo "==> [4/5] pull $HOST:~/$RDIST/$PATTERN → $DIST/"
run scp -q "$HOST:$RDIST/$PATTERN" "$DIST/"
case "$OS" in
  windows) EXPECTED=("theuth_${VERSION}_windows-x86_64-setup.exe" "theuth_${VERSION}_windows-x86_64.zip") ;;
  linux) EXPECTED=("theuth_${VERSION}_linux-*.AppImage" "theuth_${VERSION}_linux-*.deb" "theuth_${VERSION}_linux-*.rpm" "theuth_${VERSION}_linux-*.tar.gz") ;;
esac
if [[ "$DRY_RUN" == "1" ]]; then
  echo "DRY_RUN: would require in $DIST/: ${EXPECTED[*]}"
else
  shopt -s nullglob
  # shellcheck disable=SC2206  # $PATTERN is a glob on purpose
  GOT=("$DIST"/$PATTERN)
  shopt -u nullglob
  for f in ${GOT[@]+"${GOT[@]}"}; do
    printf '    %-48s %12s  %s\n' "$(basename "$f")" "$(wc -c <"$f" | tr -d ' ')" "$(shasum -a 256 "$f" | awk '{print $1}')"
  done
  MISSING=()
  for pat in "${EXPECTED[@]}"; do
    compgen -G "$DIST/$pat" >/dev/null || MISSING+=("$pat")
  done
  if [[ ${#MISSING[@]} -gt 0 ]]; then
    echo "error: expected in $DIST/ but not produced by $HOST: ${MISSING[*]}" >&2
    exit 1
  fi
fi

# ── 5. clean ─────────────────────────────────────────────────────────────────
if [[ "$KEEP" == "1" ]]; then
  echo "==> [5/5] --keep: $HOST:~/$RSRC left in place"
else
  echo "==> [5/5] remove $HOST:~/$RSRC (target/ stays)"
  case "$OS" in
    linux) run ssh "$HOST" "rm -rf ~/$RSRC" ;;
    windows) run_pwsh "$WIN_CLEAN" ;;
  esac
fi

echo
echo "done: theuth $VERSION $OS artifacts from $HOST in $DIST/ — next: just release (release-s3.sh reads dist/ first)"
