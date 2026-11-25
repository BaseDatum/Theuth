#!/bin/bash
# SDK conformance (design 216): every SDK's fixture and hello-world template against host.py.
# Usage: sdk/conformance/check.sh [py js sh go rb rs raw]   (default: all). Needs python3 and perl.
# A language whose runtime is missing is SKIP; a present runtime that fails is FAIL.
# Each case runs in a staged copy (template/fixture + the SDK file), the way `theuth plugin new` lays
# out a plugin folder. Rust builds honour the machine build lock ($THEUTH_BUILD_LOCK, "" disables).
set -u
here=$(cd "$(dirname "$0")" && pwd)
sdk=$(cd "$here/.." && pwd)
langs=${*:-py js sh go rb rs raw}
tmp=$(mktemp -d "${TMPDIR:-/tmp}/theuth-sdk-conf.XXXXXX")
LOCK=${THEUTH_BUILD_LOCK-/tmp/theuth-build.lock}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/tmp/theuth-216-sdk-target}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-6} CARGO_INCREMENTAL=0 GOFLAGS=${GOFLAGS:--p=4}
held=0 last_lock=0 results=""
cleanup() { [ $held = 1 ] && rmdir "$LOCK" 2>/dev/null; rm -rf "$tmp"; }
trap cleanup EXIT
trap 'exit 130' INT TERM

have() { command -v "$1" >/dev/null 2>&1; }
note() { results="$results$1|"; echo "-- $1"; }

# locked CMD...: run one build command holding the machine build lock, 45 s after the last release.
locked() {
  if [ -n "$LOCK" ]; then
    wait_s=$((last_lock + 45 - $(date +%s)))
    [ "$wait_s" -gt 0 ] && sleep "$wait_s"
    until mkdir "$LOCK" 2>/dev/null; do sleep 10; done
    held=1
  fi
  "$@"; rc=$?
  if [ -n "$LOCK" ]; then rmdir "$LOCK"; held=0; last_lock=$(date +%s); fi
  return $rc
}

# stage KIND LANG -> prints the staged folder (KIND: fixture | template)
stage() {
  d="$tmp/$2-$1"
  rm -rf "$d"; mkdir -p "$d"
  if [ "$1" = fixture ]; then cp -R "$here/plugins/$2/." "$d/"; else cp -R "$sdk/templates/$2/." "$d/"; fi
  case $2 in
    py) cp "$sdk/python/theuth_plugin.py" "$d/" ;;
    js) cp "$sdk/ts/theuth-plugin.js" "$sdk/ts/theuth-plugin.d.ts" "$d/" ;;
    sh) cp "$sdk/sh/theuth-plugin.sh" "$d/" ;;
    go) cp "$sdk/go/theuth.go" "$d/" ;;
    rb) cp "$sdk/ruby/theuth_plugin.rb" "$d/" ;;
    rs) mkdir -p "$d/src"; cp "$sdk/rust/src/lib.rs" "$d/src/theuth.rs" ;;
  esac
  echo "$d"
}

host() { perl -e 'alarm 180; exec @ARGV' python3 "$here/host.py" "$@"; }
rs_build() { cargo build --quiet --manifest-path "$1/Cargo.toml" --offline 2>/dev/null ||
  cargo build --quiet --manifest-path "$1/Cargo.toml"; }

# suite LABEL KIND DIR CMD...: fixture -> full + manifest; template -> two runs (state persists), eof,
# manifest; raw (no SDK, no --theuth-manifest) -> two runs, eof; status (status-only shape) -> status.
suite() {
  label=$1 kind=$2 d=$3; shift 3
  echo "== $label"
  ok=PASS data="$tmp/data-$label"
  case $kind in
    fixture)
      (cd "$d" && host --mode full --data "$data" -- "$@") || ok=FAIL
      (cd "$d" && host --mode manifest-full -- "$@") || ok=FAIL ;;
    template|raw)
      (cd "$d" && host --mode template --data "$data" --start 0 -- "$@") || ok=FAIL
      echo "  (second run, state persisted)"
      (cd "$d" && host --mode template --data "$data" --start 1 -- "$@") || ok=FAIL
      (cd "$d" && host --mode eof --data "$tmp/eof-$label" -- "$@") || ok=FAIL
      if [ "$kind" = template ]; then (cd "$d" && host --mode manifest-template -- "$@") || ok=FAIL; fi ;;
    status)
      (cd "$d" && host --mode status --data "$data" -- "$@") || ok=FAIL ;;
  esac
  note "$ok $label"
}

for lang in $langs; do
  for kind in fixture template; do
    case $lang in
      raw) [ $kind = template ] || continue
           cp -R "$sdk/templates/raw-py" "$tmp/raw-py"; cp -R "$sdk/templates/raw-sh" "$tmp/raw-sh"
           if have python3; then suite "raw-py template (python3)" raw "$tmp/raw-py" python3 main.py; fi
           for b in /bin/bash $(command -v bash); do
             suite "raw-sh template (bash $("$b" -c 'echo $BASH_VERSION'))" status "$tmp/raw-sh" "$b" run.sh
           done ;;
      py) if have python3; then d=$(stage $kind py); suite "py $kind (python3)" $kind "$d" python3 main.py
          else note "SKIP py (no python3)"; fi ;;
      js) found=0
          for rt in node bun deno; do
            have $rt || { note "SKIP js $kind ($rt missing)"; continue; }
            found=1; d=$(stage $kind js)
            case $rt in
              deno) suite "js $kind (deno)" $kind "$d" deno run --quiet --allow-env --allow-read --allow-write main.mjs ;;
              *) suite "js $kind ($rt)" $kind "$d" $rt main.mjs ;;
            esac
          done ;;
      sh) bashes="/bin/bash"
          for b in /opt/homebrew/bin/bash /usr/local/bin/bash $(command -v bash); do
            case " $bashes " in *" $b "*) ;; *) [ -x "$b" ] && bashes="$bashes $b" ;; esac
          done
          for b in ${BASHES:-$bashes}; do
            v=$("$b" -c 'echo $BASH_VERSION')
            for json in jq python3; do
              have $json || { note "SKIP sh $kind bash $v ($json missing)"; continue; }
              d=$(stage $kind sh)
              THEUTH_JSON=$json suite "sh $kind (bash $v, $json)" $kind "$d" "$b" run.sh
            done
          done ;;
      go) if have go; then
            d=$(stage $kind go)
            if (cd "$d" && go build -o plugin .); then suite "go $kind" $kind "$d" ./plugin
            else note "FAIL go $kind (build)"; fi
          else note "SKIP go (no go)"; fi ;;
      rb) if have ruby; then d=$(stage $kind rb); suite "rb $kind (ruby $(ruby -e 'print RUBY_VERSION'))" $kind "$d" ruby main.rb
          else note "SKIP rb (no ruby)"; fi ;;
      rs) if have cargo; then
            d=$(stage $kind rs)
            bin=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$d/Cargo.toml" | head -1)
            if locked rs_build "$d"; then
              suite "rs $kind" $kind "$d" "$CARGO_TARGET_DIR/debug/$bin"
            else note "FAIL rs $kind (build)"; fi
          else note "SKIP rs (no cargo)"; fi ;;
      *) note "FAIL unknown language $lang" ;;
    esac
  done
done

echo
echo "== summary"
fail=0
old_ifs=$IFS; IFS='|'
for r in $results; do
  [ -n "$r" ] || continue
  echo "  $r"
  case $r in FAIL*) fail=1 ;; esac
done
IFS=$old_ifs
echo "conformance: $([ $fail = 0 ] && echo PASS || echo FAIL)"
exit $fail
