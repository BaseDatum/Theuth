# 70 as built — MODELS-FRESH: `just release-models` follows Hugging Face `main`

Branch `fix/release-models-freshness` off `b1ff179d`. Brief: `/tmp/69/omp-MODELS-FRESH.md`. Sky, 2026-09-27: "make sure just release-models pulls the latest from hugging face if there has been an update and replaces them on s3". Nothing uploaded; `~/.theuth` read only (`--check` writes nothing).

## Before

`scripts/fetch-models.sh` fetched only what was missing ("files already on disk are never re-downloaded"); `release-models.sh` uploaded a file when the object's size differed. A new revision at the origin never reached a machine that already had the old bytes, and therefore never reached the bucket.

## What changed

- **`scripts/fetch-models.sh`** — `fetch_missing` → `fetch_models`: missing → `fetch_one` (origin, else bucket; unchanged), present with an `hf:` origin → `refresh_one`: `probe` (curl `-IL`, headers of every hop) → `origin_rev` / `origin_etag` / `origin_size` prefer the huggingface.co hop's `x-repo-commit` / `x-linked-etag` / `x-linked-size` because the CDN hop's `etag` is a xet hash, not the file's (checked 2026-09-27 against lokkju and onnx-community; the plain-file case — 307 to HF itself, etag = git blob sha1 — against convaiinnovations). Record `$MODELS_DIR/.origin/<path>.json` `{repo, path, revision, etag, size, sha256, fetched_at}`; no record → `local_etag_of` hashes the file in the etag's own form (sha256 for 64 hex, `blob <size>\0` sha1 for 40 hex) and a match writes the record. Stale = etag or size differs (a revision that moved without touching the file just refreshes the record). `fetch_origin`: download to `.part`, size vs the announced size, hash vs the etag, `mv`, record. Flags `--check` / `--force <path|all>` parsed from `$@` (also when sourced by release-models); `MODELS_DRY_RUN=1` joins `THEUTH_S3_DRY_RUN` / `DRY_RUN`; `HF_BASE` for the test's fake origin. `explain` names the failure: gated 401 (`x-error-code: GatedRepo`), proxy 403 (a bounded GET on 403 looks for the Zscaler block page), moved 404. A failed HEAD keeps the file and counts as `not checkable`; a failed download exits 1 (first-failure-stops, as before).
- **`scripts/release-models.sh`** — `publish` decides `new` (no row in the bucket's `models.json`) / `kept` (row has the same size+sha256 **and** `head-object` reports that size) / `updated` (otherwise → `aws s3 cp` to the same key). Model rows gain `revision` (from the record) and `origin` (`origin_of`, omitted for `none:`). Summary table after `models.json`: path · action · old rev → new rev · bytes (old rev from the published manifest's `revision`, `-` until the first run with this script). `--check` returns after the report, no AWS creds needed. **Rule followed:** unversioned `models/<model>/<file>` keys replaced in place, `models.json` last — the design (57 §2, 00 §9, the script itself) has always published flat keys and the size-diff upload already overwrote; 39 §5's `<ver>/` immutability is for app releases. 43 §0.3's early `pocket-tts/<rev>/` idea was never built.
- **`justfile`** — `fetch-models *ARGS` / `release-models *ARGS` pass the flags through; comments rewritten.
- **`crates/desktop/src/models.rs`** — `manifest_ignores_unknown_fields` proves `revision` / `origin` / a nested unknown / a top-level unknown are ignored (serde's default; no `deny_unknown_fields`). Reader otherwise unchanged.
- **`docs/design/57-decision.md` §2** — new "Freshness" bullet after the distribution bullet.

## Test — `scripts/tests/fetch-models-test.sh` (no bats on this Mac; plain bash)

python3 `http.server` fake origin: LFS files answer 302 with `x-repo-commit` / `x-linked-etag` / `x-linked-size` and redirect to a `/cdn/` hop with a foreign etag; `.json` files answer 200 with the blob sha1; `kyutai/pocket-tts` → 401 GatedRepo; `onnx-community/*` → 403 with a Zscaler body; unknown → 404 EntryNotFound. Bucket = a refused port. Cases: no record + matching etag (LFS and plain; records written with the right etag/revision/repo); gated 401, proxy 403, moved 404 messages, `pinned` / `no documented origin` lines, `20 files: 0 stale, 0 missing, 13 not checkable`; origin advances → `--check` prints `STALE rev r1 → r2`, exits 2, touches neither file nor record; `MODELS_DRY_RUN=1` prints `would replace`, leaves no `.part`; plain run → `updated rev r1 → r2 (5000 bytes)`, bytes replaced, record `r2 5000`; `--force <path>` re-downloads an up-to-date file; `release-models.sh` dry run with a fake `aws` on PATH: `would replace`, `DRY_RUN: aws s3 cp …`, manifest rows carry `revision` / `origin`, summary rows for a model and the runtime, **no `s3 cp` reached `aws`**, record untouched; `release-models.sh --check` exits 2 without creds. 46 s. `shellcheck -x` clean on all three scripts (`SC1090` disabled on the one `source <(sed …)` that lifts REQUIRED out of the script under test).

## Real check (this Mac, 2026-09-27, no HF_TOKEN, no proxy block) — `just fetch-models --check`, exit 2

| file | result |
|---|---|
| nemotron-streaming-en/{encoder.onnx, decoder_joint.onnx, tokenizer.model} | up to date, rev `95df6c82` |
| nemotron-diarization/{model_quantized.onnx, .onnx_data} | up to date, rev `353b6f8a` |
| decision/{model.safetensors, encoder/config.json, tokenizer/tokenizer.json, rl_agent_config.json} | up to date, rev `55cf4c4e` |
| pocket-tts/{tts_b6369a24.safetensors, tokenizer.model} | cannot check: 401 GatedRepo (set `HF_TOKEN`) |
| decision-browser/* (4) | cannot check: **404 EntryNotFound — `cklxx/laya-browser` no longer has `v10/`**; `main` (`ac29aefb`) holds `v17s/` only: `model.safetensors` 643,835,524 B, `tokenizer/tokenizer.json` 34,363,188 B, `encoder/config.json` 1,939 B — a different backbone/tokenizer than the 842 MB v10 on disk, not a drop-in. Sky's call whether `origin_of` moves to `v17s` (57 §4.1 records v10). |
| pocket-tts/voices, parakeet-tdt/* | no documented origin — not checked |
| pyannote/wespeaker | pinned https origin — not checked |

**Stale today: none of the 9 checkable files.** Nothing on the bucket needs replacing for those; the first real `just release-models` will still show every model row as `updated`/`new` in the summary only if the bucket's `models.json` sha256s differ from the local files (it compares content, not history).

## Not done / ceilings

- `cargo test -p desktop models::`: 3 passed (`platform_and_key_fields`, `manifest_ignores_unknown_fields`, `verified_file_lands_and_a_bad_one_leaves_nothing`).
- Desktop `present()` is size-only (its own comment: hashing 2 GB per start is not worth it), so a same-size replacement reaches fresh installs only. A `revision` compare against the record would fix it; not asked.
- `kept` needs both the manifest row and a matching `head-object`; when `head-object` fails (creds, network) the file is re-uploaded — the safe direction, same as the old size check failing closed.
- `https:` origins are never re-checked (GitHub release assets are pinned by tag). `--force` covers them.
