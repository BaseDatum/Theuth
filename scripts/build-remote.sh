#!/usr/bin/env bash
# Build the Windows or Linux release artifacts on a remote ssh host and pull them into dist/ (58 step 15).
#
#   scripts/build-remote.sh <user@host> <windows|linux> [--ref <git ref>] [--dist <dir>] [--keep]
#                           [--no-wait] [--resume] [--attach <remote dir>]
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
# 3. start    the build is started DETACHED — it survives a dropped ssh connection — and never runs in
#             the ssh session's foreground. In <sha>/: build.sh|build.ps1 (the commands), build.log
#             (+ build.err on Windows), build.pid, and build.exit (the exit code, written last).
#             linux:   nohup setsid bash -c 'bash build.sh; echo $? > build.exit' > build.log 2>&1 < /dev/null &
#                      build.sh: THEUTH_SKIP_UPLOAD=1 scripts/package-linux.sh            → AppImage, deb, rpm
#                                THEUTH_LINUX_NATIVE=1 THEUTH_SKIP_UPLOAD=1 scripts/stage-linux.sh  → CLI tar.gz
#             windows: Start-Process pwsh -File build.ps1 -RedirectStandardOutput build.log -RedirectStandardError build.err -WindowStyle Hidden
#                      build.ps1: pwsh -NoProfile -File scripts\package-windows.ps1; $LASTEXITCODE > build.exit
#                      (Start-Process children are outside the sshd session's job object; verified only on
#                      the real host — see docs/design/cross-platform/step-15-remote-release-builds-as-built.md)
# 4. poll     every $THEUTH_REMOTE_POLL s (default 20): one ssh prints the log lines after the ones already
#             shown (line count kept locally) and build.exit if present. An ssh failure is retried with
#             backoff 5, 10, 20, 40, 60, 60 … s — the build keeps running. build.exit != 0 → the last 60 log
#             lines, exit with that code (the remote dir is kept for inspection).
# 5. pull     only after build.exit == 0: scp remote target/release/dist/theuth_<ver>_<os>-* → --dist
#             (default target/release/dist, what release-s3.sh reads); print name / size / sha256; exit 1
#             if an expected artifact is missing (windows: -setup.exe + .zip; linux: .AppImage + .deb + .rpm + .tar.gz).
# 6. clean    rm the remote source dir (--keep leaves it); ~/theuth-build/target stays for the next build.
#
# --no-wait          stop after 3: print the remote dir and pid; `--resume` (or `--attach`) later.
# --resume           skip 1-3 and poll the build of <ref> (dir = theuth-build/<short sha>, deterministic).
# --attach <dir>     like --resume for an explicit remote dir (relative to the remote home, e.g. theuth-build/1a81f57657b7).
# A dirty working tree is refused unless --ref names a commit/tag (HEAD is the default ref).
# Env: THEUTH_REMOTE_DRY_RUN=1  print every ssh/scp command and remote script instead of running them.
#      THEUTH_REMOTE_POLL       seconds between polls (default 20).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

usage() {
  echo "usage: scripts/build-remote.sh <user@host> <windows|linux> [--ref <git ref>] [--dist <dir>] [--keep] [--no-wait] [--resume] [--attach <remote dir>]" >&2
  exit 64
}
HOST=""; OS=""; REF=""; DIST="target/release/dist"; KEEP=0; NO_WAIT=0; RESUME=0; ATTACH=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ref) [[ $# -ge 2 ]] || usage; REF="$2"; shift 2 ;;
    --dist) [[ $# -ge 2 ]] || usage; DIST="$2"; shift 2 ;;
    --attach) [[ $# -ge 2 ]] || usage; ATTACH="${2#\~/}"; RESUME=1; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --no-wait) NO_WAIT=1; shift ;;
    --resume) RESUME=1; shift ;;
    -h | --help) usage ;;
    -*) echo "error: unknown option $1" >&2; usage ;;
    *) if [[ -z "$HOST" ]]; then HOST="$1"; elif [[ -z "$OS" ]]; then OS="$1"; else usage; fi; shift ;;
  esac
done
[[ -n "$HOST" && -n "$OS" ]] || usage
case "$OS" in windows | linux) ;; *) echo "error: os must be windows|linux (got $OS)" >&2; usage ;; esac
[[ "$NO_WAIT$RESUME" != "11" ]] || { echo "error: --no-wait and --resume/--attach exclude each other" >&2; usage; }
DRY_RUN="${THEUTH_REMOTE_DRY_RUN:-0}"
POLL="${THEUTH_REMOTE_POLL:-20}"

# shellcheck source=lib/common.sh
source "$ROOT/scripts/lib/common.sh"   # need
need ssh; need scp; need git; need jq; need shasum
[[ "$OS" != "windows" ]] || { need iconv; need base64; }

# ── the commit that ships ────────────────────────────────────────────────────
if [[ -z "$REF" ]]; then
  [[ "$RESUME" == "1" || -z "$(git status --porcelain --untracked-files=no)" ]]   # untracked files never ship (git archive) — only tracked changes count as dirty || { echo "error: working tree is dirty — commit first, or build a commit/tag with --ref" >&2; exit 1; }
  REF=HEAD
fi
COMMIT="$(git rev-parse --verify --quiet "$REF^{commit}")" || { echo "error: $REF is not a commit" >&2; exit 1; }
SHA="$(git rev-parse --short=12 "$COMMIT")"
VERSION="$(git show "$COMMIT:crates/desktop/tauri.conf.json" | jq -r .version)"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]] || { echo "error: no version in crates/desktop/tauri.conf.json at $SHA (got '$VERSION')" >&2; exit 1; }
RBASE="theuth-build"          # relative to the remote home (ssh, scp and sftp all resolve it)
RSRC="${ATTACH:-$RBASE/$SHA}"
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
LINUX_START="$LINUX_ENV
cat >build.sh <<'BUILD'
$LINUX_ENV
rm -f target/release/dist/theuth_${VERSION}_*
THEUTH_SKIP_UPLOAD=1 scripts/package-linux.sh
THEUTH_LINUX_NATIVE=1 THEUTH_SKIP_UPLOAD=1 scripts/stage-linux.sh
ls -l target/release/dist/
BUILD
rm -f build.exit build.log build.pid
nohup setsid bash -c 'echo \$\$ >build.pid; bash build.sh; echo \$? >build.exit' >build.log 2>&1 </dev/null &
echo \"started: \$HOME/$RSRC pid \$! (build.log, build.exit)\""
linux_poll() { # <lines already shown> — prints new log lines, then __EXIT__=<code|empty>
  # shellcheck disable=SC2029  # the remote shell expands ~; $1 is a number
  ssh "$HOST" "cd ~/$RSRC && { tail -n +$(( $1 + 1 )) build.log 2>/dev/null; echo __EXIT__=\$(cat build.exit 2>/dev/null); }"
}
LINUX_TAIL="cd ~/$RSRC && echo '--- build.log (last 60) ---' && tail -n 60 build.log"

# Windows: PowerShell 7. Native exit codes are checked by hand (PSNativeCommandUseErrorActionPreference
# differs between 7.3 and 7.4), the junction is removed with `rmdir` so a recursive delete never
# follows it into the shared target dir.
WIN_ENV="\$ErrorActionPreference = 'Stop'
\$PSNativeCommandUseErrorActionPreference = \$false
\$env:Path = \"\$env:USERPROFILE\\.cargo\\bin;\$env:USERPROFILE\\.bun\\bin;\$env:Path\"
\$base = Join-Path \$env:USERPROFILE '$RBASE'
\$src = Join-Path \$env:USERPROFILE '$RSRC'
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
# build.ps1 is the detached command; Start-Process puts it outside the ssh session so a dropped
# connection does not kill it. build.exit is written by the wrapper, never by package-windows.ps1.
WIN_START="$WIN_ENV
Set-Location \$src
Remove-Item -Force -ErrorAction SilentlyContinue -Path build.exit, build.log, build.err, build.pid
@'
\$ErrorActionPreference = 'Continue'
\$PSNativeCommandUseErrorActionPreference = \$false
\$env:Path = \"\$env:USERPROFILE\\.cargo\\bin;\$env:USERPROFILE\\.bun\\bin;\$env:Path\"
\$env:THEUTH_SKIP_UPLOAD = '1'
Set-Location \$PSScriptRoot
Remove-Item -Force -ErrorAction SilentlyContinue -Path (Join-Path \$env:USERPROFILE '$RBASE\\target\\release\\dist\\theuth_${VERSION}_*')
pwsh -NoProfile -File scripts\\package-windows.ps1
\$code = if (\$null -eq \$LASTEXITCODE) { 1 } else { \$LASTEXITCODE }
Get-ChildItem -Path (Join-Path \$env:USERPROFILE '$RBASE\\target\\release\\dist') -ErrorAction SilentlyContinue | Format-Table Name, Length -AutoSize | Out-String -Width 200
\$code | Out-File -Encoding ascii -FilePath (Join-Path \$PSScriptRoot 'build.exit')
exit \$code
'@ | Set-Content -Encoding utf8 -Path build.ps1
\$p = Start-Process pwsh -ArgumentList '-NoProfile', '-File', 'build.ps1' -WorkingDirectory \$src -RedirectStandardOutput build.log -RedirectStandardError build.err -WindowStyle Hidden -PassThru
\$p.Id | Out-File -Encoding ascii -FilePath build.pid
Write-Host \"started: \$src pid \$(\$p.Id) (build.log, build.err, build.exit)\""
win_poll() { # <lines already shown> — prints new log lines, then __EXIT__=<code|empty>
  run_pwsh "$WIN_ENV
Set-Location \$src
if (Test-Path build.log) { Get-Content build.log -ErrorAction SilentlyContinue | Select-Object -Skip $1 }
\$e = if (Test-Path build.exit) { (Get-Content build.exit -Raw).Trim() } else { '' }
\"__EXIT__=\$e\""
}
WIN_TAIL="$WIN_ENV
Set-Location \$src
'--- build.log (last 60) ---'; Get-Content build.log -Tail 60 -ErrorAction SilentlyContinue
'--- build.err (last 60) ---'; Get-Content build.err -Tail 60 -ErrorAction SilentlyContinue"
WIN_CLEAN="$WIN_ENV
Remove-Source
Write-Host \"removed \$src\""

if [[ "$RESUME" == "1" ]]; then
  echo "==> [1-3/6] --resume: attaching to $HOST:~/$RSRC"
else
  # ── 1. ship ────────────────────────────────────────────────────────────────
  echo "==> [1/6] ship $SHA → $HOST:~/$RSRC"
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

  # ── 2. check ───────────────────────────────────────────────────────────────
  echo "==> [2/6] prerequisites on $HOST"
  case "$OS" in
    linux) step run_bash "$LINUX_CHECK" ;;
    windows) step run_pwsh "$WIN_CHECK" ;;
  esac

  # ── 3. start (detached) ────────────────────────────────────────────────────
  echo "==> [3/6] start detached build on $HOST"
  case "$OS" in
    linux) run_bash "$LINUX_START" ;;
    windows) run_pwsh "$WIN_START" ;;
  esac
  if [[ "$NO_WAIT" == "1" ]]; then
    echo
    echo "started: --no-wait. Attach later with:"
    echo "  scripts/build-remote.sh $HOST $OS --ref $COMMIT --resume    # or: --attach $RSRC"
    exit 0
  fi
fi

# ── 4. poll ──────────────────────────────────────────────────────────────────
echo "==> [4/6] poll $HOST:~/$RSRC/build.log every ${POLL}s (build.exit ends it; ssh drops are retried)"
if [[ "$DRY_RUN" == "1" ]]; then
  case "$OS" in
    linux) echo "DRY_RUN: ssh $HOST 'cd ~/$RSRC && { tail -n +<shown+1> build.log 2>/dev/null; echo __EXIT__=\$(cat build.exit 2>/dev/null); }'" ;;
    windows) win_poll "<shown>" ;;
  esac
  echo "DRY_RUN: repeat every ${POLL}s until __EXIT__=<code>; assuming 0"
  CODE=0
else
  SHOWN=0; BACKOFF=5; CODE=""
  while :; do
    rc=0
    case "$OS" in
      linux) OUT="$(linux_poll "$SHOWN" 2>&1)" || rc=$? ;;
      windows) OUT="$(win_poll "$SHOWN" 2>&1)" || rc=$? ;;
    esac
    if [[ $rc -ne 0 || "$OUT" != *__EXIT__=* ]]; then
      echo "    ssh to $HOST failed (exit $rc): ${OUT##*$'\n'} — retry in ${BACKOFF}s (the build keeps running)" >&2
      sleep "$BACKOFF"; BACKOFF=$(( BACKOFF * 2 > 60 ? 60 : BACKOFF * 2 )); continue
    fi
    BACKOFF=5
    LOG="${OUT%__EXIT__=*}"
    CODE="${OUT##*__EXIT__=}"; CODE="${CODE//[$'\r\n ']/}"
    if [[ -n "$LOG" ]]; then
      # Complete lines only: a partial last line (no newline yet) is neither printed nor counted, so it
      # is shown whole on a later poll instead of being cut.
      printf '%s' "$LOG" | tr -d '\r' | while IFS= read -r l; do printf '    %s\n' "$l"; done
      SHOWN=$(( SHOWN + $(printf '%s' "$LOG" | tr -cd '\n' | wc -c) ))
    fi
    [[ -z "$CODE" ]] || break
    sleep "$POLL"
  done
  if [[ "$CODE" != "0" ]]; then
    echo "error: remote build on $HOST exited $CODE — ~/$RSRC kept (build.log, build.sh|build.ps1)" >&2
    # shellcheck disable=SC2029  # $LINUX_TAIL expands here on purpose
    case "$OS" in
      linux) ssh "$HOST" "$LINUX_TAIL" || true ;;
      windows) run_pwsh "$WIN_TAIL" || true ;;
    esac
    exit "$CODE"
  fi
  echo "    build.exit = 0"
fi

# ── 5. pull ──────────────────────────────────────────────────────────────────
echo "==> [5/6] pull $HOST:~/$RDIST/$PATTERN → $DIST/"
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

# ── 6. clean ─────────────────────────────────────────────────────────────────
if [[ "$KEEP" == "1" ]]; then
  echo "==> [6/6] --keep: $HOST:~/$RSRC left in place"
else
  echo "==> [6/6] remove $HOST:~/$RSRC (target/ stays)"
  case "$OS" in
    linux) run ssh "$HOST" "rm -rf ~/$RSRC" ;;
    windows) run_pwsh "$WIN_CLEAN" ;;
  esac
fi

echo
echo "done: theuth $VERSION $OS artifacts from $HOST in $DIST/ — next: just release (release-s3.sh reads dist/ first)"
