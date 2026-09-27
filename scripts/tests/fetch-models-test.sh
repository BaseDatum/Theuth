#!/usr/bin/env bash
# fetch-models.sh against a fake Hugging Face (python3 http.server answering with HF's headers:
# `x-repo-commit`, `x-linked-etag`, `x-linked-size`, a redirect to a "CDN" hop whose etag is not the
# file's) and a bucket that refuses connections. Cases: no record + matching etag (LFS sha256 and
# plain-file blob sha1), stale revision → replaced (after --check reports it, exit 2), gated 401,
# proxy 403 (Zscaler block page), dry runs of fetch-models and release-models (fake `aws`: no cp).
#
#   scripts/tests/fetch-models-test.sh        prints one `ok` per case; any failure exits 1
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"; kill "${SERVER:-0}" 2>/dev/null || true' EXIT
ORIGIN="$T/origin"; MODELS="$T/models"; OUT="$T/out"
mkdir -p "$ORIGIN" "$MODELS"

fail() { echo "FAIL: $*" >&2; echo "--- last output:" >&2; cat "$OUT" >&2; exit 1; }
has()  { grep -qF -- "$1" "$OUT" || fail "expected in output: $1"; }
lacks() { ! grep -qF -- "$1" "$OUT" || fail "unexpected in output: $1"; }

# --- fake origin -------------------------------------------------------------------------------------
cat >"$T/origin.py" <<'PY'
import hashlib, http.server, os, sys, threading
ROOT = sys.argv[1]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, headers, body=b""):
        self.send_response(code)
        for k, v in headers: self.send_header(k, v)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD": self.wfile.write(body)
    def do_HEAD(self): self.do_GET()
    def do_GET(self):
        p = self.path.split("?")[0]
        if p.startswith("/cdn/"):                      # the CDN hop: etag is a xet hash, not the file's
            data = open(os.path.join(ROOT, ".cdn", p[5:]), "rb").read()
            return self.send(200, [("ETag", '"xet-%s"' % p[5:][:8])], data)
        parts = p.lstrip("/").split("/resolve/main/")
        if len(parts) != 2: return self.send(404, [("x-error-code", "RepoNotFound")], b"Not Found")
        repo, path = parts
        if repo == "kyutai/pocket-tts":
            return self.send(401, [("x-error-code", "GatedRepo"), ("ETag", 'W/"7d-x"')], b"Access to model is restricted")
        if repo.startswith("onnx-community/"):
            return self.send(403, [("Server", "ZscalerProxy")], b"<html>Zscaler: this site is blocked by your organisation</html>")
        f = os.path.join(ROOT, repo, path)
        if not os.path.isfile(f): return self.send(404, [("x-error-code", "EntryNotFound")], b"Entry not found")
        data = open(f, "rb").read()
        rev = open(os.path.join(ROOT, repo, "REV")).read().strip()
        if path.endswith(".json"):                     # plain git file: etag = blob sha1, served by HF itself
            blob = hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()
            return self.send(200, [("x-repo-commit", rev), ("x-linked-etag", '"%s"' % blob), ("ETag", '"%s"' % blob)], data)
        sha = hashlib.sha256(data).hexdigest()         # LFS file: redirect carries the identity
        os.makedirs(os.path.join(ROOT, ".cdn"), exist_ok=True)
        open(os.path.join(ROOT, ".cdn", sha), "wb").write(data)
        return self.send(302, [("x-repo-commit", rev), ("x-linked-etag", '"%s"' % sha), ("x-linked-size", str(len(data))),
                               ("Location", "/cdn/" + sha)], b"redirect")
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
open(os.path.join(ROOT, "PORT"), "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY
python3 "$T/origin.py" "$ORIGIN" & SERVER=$!
for _ in $(seq 1 50); do [[ -f "$ORIGIN/PORT" ]] && break; sleep 0.1; done
PORT="$(cat "$ORIGIN/PORT")"
export HF_BASE="http://127.0.0.1:$PORT" THEUTH_MODELS_DIR="$MODELS" THEUTH_S3_PUBLIC_BASE_URL="http://127.0.0.1:1/x"
unset HF_TOKEN DRY_RUN MODELS_DRY_RUN THEUTH_S3_DRY_RUN

# --- fixtures: every REQUIRED file on disk (so nothing is fetched); two of them at the fake origin ---
# shellcheck disable=SC1090  # REQUIRED array lifted out of the script under test
source <(sed -n '/^REQUIRED=(/,/^)/p' "$ROOT/scripts/fetch-models.sh")
for f in "${REQUIRED[@]}"; do mkdir -p "$MODELS/$(dirname "$f")"; echo "placeholder $f" >"$MODELS/$f"; done
LFS=nemotron-streaming-en/tokenizer.model; LFS_REPO=lokkju/nemotron-speech-streaming-en-0.6b-int8
PLAIN=decision/rl_agent_config.json;        PLAIN_REPO=convaiinnovations/laya
mkdir -p "$ORIGIN/$LFS_REPO" "$ORIGIN/$PLAIN_REPO"
head -c 4000 /dev/urandom >"$ORIGIN/$LFS_REPO/tokenizer.model";     echo r1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa >"$ORIGIN/$LFS_REPO/REV"
echo '{"agent":1}' >"$ORIGIN/$PLAIN_REPO/rl_agent_config.json";     echo r1bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb >"$ORIGIN/$PLAIN_REPO/REV"
cp "$ORIGIN/$LFS_REPO/tokenizer.model" "$MODELS/$LFS"; cp "$ORIGIN/$PLAIN_REPO/rl_agent_config.json" "$MODELS/$PLAIN"

run() { set +e; "$@" >"$OUT" 2>&1; RC=$?; set -e; }

# 1. no record + matching etag → up to date, record written (sha256 for LFS, blob sha1 for plain)
run "$ROOT/scripts/fetch-models.sh"
[[ $RC -eq 0 ]] || fail "exit $RC"
has "$LFS: up to date (rev r1aaaa"; has "$PLAIN: up to date (rev r1bbbb"
[[ "$(jq -r .etag "$MODELS/.origin/$LFS.json")" == "$(shasum -a 256 "$MODELS/$LFS" | cut -d' ' -f1)" ]] || fail "LFS record etag"
[[ "$(jq -r .revision "$MODELS/.origin/$PLAIN.json")" == r1bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ]] || fail "plain record revision"
[[ "$(jq -r .repo "$MODELS/.origin/$PLAIN.json")" == "$PLAIN_REPO" ]] || fail "plain record repo"
# 3. gated 401 and 4. proxy 403 → clear message, file kept, run continues
has "HTTP 401 from $HF_BASE/kyutai/pocket-tts/resolve/main/tokenizer.model — gated repo (set HF_TOKEN) or a proxy blocking the host (x-error-code: GatedRepo)"
has "pocket-tts/tokenizer.model: cannot check freshness (kept)"
has "HTTP 403 from $HF_BASE/onnx-community/Nemotron-3-Diarization-ONNX/resolve/main/onnx/model_quantized.onnx — blocked by the Zscaler proxy on this network"
has "HTTP 404 from $HF_BASE/cklxx/laya-browser/resolve/main/v10/model.safetensors — the file moved or was renamed at the origin; fix origin_of in scripts/fetch-models.sh (x-error-code: EntryNotFound)"
has "pyannote/wespeaker_en_voxceleb_CAM++.onnx: pinned (https origin, no freshness check)"
has "parakeet-tdt/vocab.txt: no documented origin — not checked"
has "20 files: 0 stale, 0 missing, 13 not checkable"
echo "ok: no record + matching etag; gated 401; proxy 403; moved 404"

# 2. the origin moves on: --check reports and exits 2 without touching anything; a plain run replaces
head -c 5000 /dev/urandom >"$ORIGIN/$LFS_REPO/tokenizer.model"; echo r2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa >"$ORIGIN/$LFS_REPO/REV"
run "$ROOT/scripts/fetch-models.sh" --check
[[ $RC -eq 2 ]] || fail "--check exit $RC, want 2"
has "$LFS: STALE rev r1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa → r2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"; has "$PLAIN: up to date"
has "20 files: 1 stale, 0 missing, 13 not checkable"
[[ "$(jq -r .revision "$MODELS/.origin/$LFS.json")" == r1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ]] || fail "--check wrote the record"
cmp -s "$MODELS/$LFS" "$ORIGIN/.cdn/$(shasum -a 256 "$MODELS/$LFS" | cut -d' ' -f1)" || fail "--check replaced the file"
# 5a. dry run: plan only
MODELS_DRY_RUN=1 run "$ROOT/scripts/fetch-models.sh"
[[ $RC -eq 0 ]] || fail "dry run exit $RC"
has "$LFS: DRY_RUN: would replace, rev r1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa → r2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ← $HF_BASE/$LFS_REPO/resolve/main/tokenizer.model"
[[ "$(jq -r .revision "$MODELS/.origin/$LFS.json")" == r1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ]] || fail "dry run wrote the record"
ls "$MODELS/$LFS".part "$MODELS/$LFS".headers 2>/dev/null && fail "dry run left temp files"
echo "ok: --check exit 2, dry run plans only"

run "$ROOT/scripts/fetch-models.sh"
[[ $RC -eq 0 ]] || fail "exit $RC"
has "$LFS: updated rev r1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa → r2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa (5000 bytes)"
cmp -s "$MODELS/$LFS" "$ORIGIN/$LFS_REPO/tokenizer.model" || fail "file not replaced"
[[ "$(jq -r '.revision + " " + (.size|tostring)' "$MODELS/.origin/$LFS.json")" == "r2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 5000" ]] || fail "record not updated"
run "$ROOT/scripts/fetch-models.sh" --check
[[ $RC -eq 2 ]] || fail "post-update --check exit $RC (the 401/403/404 files stay unverifiable)"
has "20 files: 0 stale, 0 missing, 13 not checkable"
# --force re-downloads an up-to-date file
run "$ROOT/scripts/fetch-models.sh" --force "$PLAIN"
[[ $RC -eq 0 ]] || fail "--force exit $RC"
has "$PLAIN: updated rev r1bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb → r1bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb (12 bytes)"
echo "ok: stale revision replaced, record updated, --force"

# 5b. release-models dry run: fake aws records every call; no `s3 cp`, summary table printed
mkdir -p "$T/bin"; cat >"$T/bin/aws" <<SH
#!/usr/bin/env bash
echo "\$*" >>"$T/aws.log"; exit 1
SH
chmod +x "$T/bin/aws"; : >"$T/aws.log"
head -c 6000 /dev/urandom >"$ORIGIN/$LFS_REPO/tokenizer.model"; echo r3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa >"$ORIGIN/$LFS_REPO/REV"
PATH="$T/bin:$PATH" AWS_ACCESS_KEY_ID=dry AWS_SECRET_ACCESS_KEY=run MODELS_DRY_RUN=1 run "$ROOT/scripts/release-models.sh"
[[ $RC -eq 0 ]] || fail "release-models dry run exit $RC"
has "$LFS: DRY_RUN: would replace, rev r2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa → r3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
has "DRY_RUN: aws s3 cp $MODELS/$LFS s3://theuth/releases/models/$LFS"
has "DRY_RUN: aws s3 cp $T/"; has "models.json"
has '"revision": "r2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"'; has '"origin": "hf:lokkju/nemotron-speech-streaming-en-0.6b-int8/tokenizer.model"'
grep -E "^    $LFS +new +- → r2aaaaa +5000$" "$OUT" >/dev/null || fail "summary row for $LFS"
grep -E "^    onnxruntime/aarch64-apple-darwin/VERSION +new " "$OUT" >/dev/null || fail "summary row for the runtime"
! grep -q " s3 cp " "$T/aws.log" || fail "dry run invoked aws s3 cp"
[[ "$(jq -r .revision "$MODELS/.origin/$LFS.json")" == r2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ]] || fail "release dry run wrote the record"
run "$ROOT/scripts/release-models.sh" --check
[[ $RC -eq 2 ]] || fail "release-models --check exit $RC, want 2"
lacks "aws s3 cp"
echo "ok: release-models dry run uploads nothing, prints the summary; --check publishes nothing"
echo "all ok"
