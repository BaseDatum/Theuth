# 70 — As built: BOARDS step 7a — `codeindex::Locate` and the `code_locate` RPC

Design 71 §5 row 7, engine half (§2.5 "Live", M11/M12). Branch `71/step-7a-code-locate` off `feature/71-boards`. The reader half (chips, Verify, live window) is step 7b.

## What was built

- `crates/codeindex/src/locate.rs` (new): `Locate { root, anchors }` → `Result<Vec<Located>, String>`; `LocateAnchor { path, start?, end?, symbol?, text_sha? }`; `Located { status, start, end, text? }`; `LocateStatus = same | changed | moved | missing` (serde lowercase); `text_sha(text)` (lowercase hex SHA-256) and `locate(ix, msg)`.
- `crates/codeindex/src/lib.rs`: `pub mod locate`, re-exports, `impl Message<Locate> for CodeindexActor`.
- `crates/rpc/src/api.rs`: `code_locate(root: String, anchors: Vec<LocateAnchor>) -> Vec<Located>`, `send(&codeindex()?, Locate{..})` beside `codeindex_stats`. `export.rs`: `LocateAnchor`, `Located` in the `ts` export list.
- Bindings: `bindings/{LocateAnchor,Located,LocateStatus}.ts`, `code_locate` in `methods.json` (`just ts`); the three files mirrored into the tracked `crates/rpc/bindings/`.
- `crates/desktop/ui/src/rpc/api.ts`: `code_locate` in the `Api` interface and `METHODS` (kept `api.test.ts` "METHODS equals the server method list" green). No reader code.
- `docs/design/60-codeindex.md` §10: amendment paragraph (71 §6.4).
- Tests: `crates/codeindex/tests/locate.rs` (one scratch git repo through every state and rejection); `locate.rs` unit tests for the lexical path check and the hash/slice helpers.

## Semantics (71 §2.5, as implemented)

1. `root` → `root::discover(Some(root))` (canonical, must exist, must be a directory), then must equal the work tree `gix::discover::upwards` finds — otherwise the whole call errors `root … is not a git top-level`.
2. Every `path` is checked lexically before any file is read: empty, absolute, containing `://` or any non-`Normal` component (`..`, `.`) → `path … is not root-relative`, whole batch fails. An existing file that canonicalises outside `root` (symlink escape) → `… is outside …`, whole batch fails.
3. File read from the working tree with `std::fs::read` + `from_utf8_lossy` + `str::lines()` (CRLF-tolerant); never `symbols.body`.
4. Range: `start` absent → whole file; `end` absent → `start` alone. `text = lines[start-1..=end-1].join("\n")`.
5. `same`: the range exists and (`text_sha` absent, or `sha256(text)` equals it case-insensitively).
6. Else with no `symbol`: `changed` if the range exists, `missing` if it runs past EOF.
7. Else with `symbol`: the index is opened once per call and reconciled (`reconcile(ri, false)`, the same front half every `code_*` tool runs); `definitions_named(symbol)` filtered to `path`, nearest `line_start` to `start`. Different `line_start` → `moved` with `start = line_start`, `end = start + (requested length − 1)` clamped to EOF, and that range's text. Same `line_start` → `changed`. No definition: `changed` if the reconcile answered `stale` and the range exists (the symbol may simply not be indexed yet), otherwise `missing`.
8. `missing` echoes the requested `start`/`end` (0 when absent) with `text: None`; every other status carries the current text.
9. Handler code path contains no write: `std::fs::read`, `canonicalize`, `is_file`, sqlite `SELECT`s through `store::definitions_named`; the only write-capable call is `Codeindex::reconcile`, which writes the derived index under `<data_dir>/codeindex/`, never the tree.

## Done-when, item by item

| Item | Evidence |
|---|---|
| `cargo test -p codeindex -p rpc` green | codeindex: 24 unit (incl. `locate::tests::{lexical_rel_rejects_escapes, sha_is_lowercase_hex_of_joined_lines}`), 17 `tests/index.rs`, 1 `tests/locate.rs`; rpc: 30 unit + doc tests. All ok, 2026-09-26. |
| Tests for identical / changed / moved / missing | `tests/locate.rs::locate_states_and_rejections`: scratch repo `src/util.rs` with `helper`/`unrelated`. Identical: L1–3 with sha → `same`, text = the three lines; bare link L5–7 (no sha) → `same`; whole file → `same` 1–7. Changed: body `1` → `1 + 0`, with and without `symbol` → `changed` L1–3 with the new text. Moved: two comment lines inserted above → with `symbol: helper` → `moved` start 3 end 5, text identical to the original; the same anchor without `symbol` → `changed`. Missing: `helper` renamed to `assistant` (file present) → `missing`; range L40–42 past EOF → `missing` echoing 40–42; `src/lib.rs` deleted → `missing`; nonexistent file with symbol → `missing`; all with `text: None`. |
| Out-of-root rejection | Same test: `../secret.rs`, `/etc/hosts`, `../../nope.rs`, and `src/link/secret.rs` where `src/link` is a symlink to a sibling temp dir holding a real file — each fails the *whole* batch (a good anchor sits beside it) with `outside` / `root-relative` in the error. Unit test: `""`, `/etc/passwd`, `../x.rs`, `a/../../x.rs`, `file:///x` rejected lexically. |
| `root` rejection | Same test: `<repo>/src` (inside a repo but not its top) → `is not a git top-level`; a plain temp dir with no `.git` → same; `<repo>/gone` → `does not exist`. |
| RPC exported in bindings | `bindings/methods.json` line 38 `"code_locate"`; `bindings/LocateAnchor.ts`, `Located.ts`, `LocateStatus.ts` generated by `just ts` (`export_bindings` 5 tests ok). Desktop `api.test.ts` "METHODS equals the server method list" passes against it (vitest 2 files / 3 tests ok with `no-emoji.test.ts`). |
| No write path in the handler | `locate.rs` has no `fs::write`, `create`, `remove`, `rename`, `OpenOptions` (grep in §"Semantics" 9); `api.rs` impl is one `send`. |
| Live wire | See "Wire smoke" below. |

## Wire smoke (2026-09-26)

`target/debug/t --headless` from this build, `THEUTH_DATA_DIR=/tmp/69/71-dd-theuth-71-s7a` (`secrets.key_store: file`), `THEUTH_RPC__LISTEN=127.0.0.1:7871`; bun client `scripts/apps/rpc.ts`; scratch `git init` repo with `src/lib.rs` = `helper` (L1–3) + `other`; `sha = sha256("pub fn helper() -> u32 {\n    1\n}")`. Positional params `[root, anchors]`:

| Call | Reply |
|---|---|
| L1–3, `symbol: helper`, sha | `[{"status":"same","start":1,"end":3,"text":"pub fn helper() -> u32 {\n    1\n}"}]` |
| two comment lines inserted above; same anchor, and the same anchor without `symbol` | `[{"status":"moved","start":3,"end":5,"text":"pub fn helper() -> u32 {\n    1\n}"},{"status":"changed","start":1,"end":3,"text":"// moved\n// down\npub fn helper() -> u32 {"}]` |
| body `1` → `1 + 0`; L1–3 with `symbol` | `[{"status":"changed","start":1,"end":3,"text":"pub fn helper() -> u32 {\n    1 + 0\n}"}]` |
| `src/gone.rs` L1 | `[{"status":"missing","start":1,"end":1,"text":null}]` |
| `../etc/passwd` | error `-32000` `path "../etc/passwd" is not root-relative` |
| `/etc/hosts` | error `-32000` `path "/etc/hosts" is not root-relative` |
| `root = /tmp`, no anchors | error `-32000` `root /private/tmp is not a git top-level` |

The `moved` reply came from the index the call itself reconciled (first open of that root, 3 files, synchronous).

## Deviations from 71 §2.5, and why

1. **No `text_sha` = `same` when the range exists.** §2.5 defines `Same` only through the hash. A `code:` link or unit `ref` has no embedded text; for it the question is "does this range still exist" and the answer carries the live text the M5 live window needs. `missing` past EOF.
2. **`moved` keeps the requested length**, not the definition's full span: `end = new_start + (end − start)`. An `Excerpt` may quote the first 60 lines of a longer definition (71 §4.2 `n = min(…, 60)`); the chip's `moved → L88–L101` should name the same-sized window. Clamped to EOF.
3. **Stale index + no definition → `changed`, not `missing`.** On the first open of a large root the reconcile runs in the background and `definitions_named` is empty; the text differing is the one thing known to be true. Once the index is warm the same anchor resolves to `moved`/`missing` properly.
4. **`text` is returned for every non-`missing` status**, including `same` with a hash. One rule; a board of hundreds of anchors is still ≤ a few hundred KiB, and the reader caches per `(path, mtime)`.
5. **The whole batch fails on one bad path or a bad root** (not a per-anchor error status). Anchors reach the RPC from `surface::check`-validated specs, so an escape is malformed or hostile input, and 60 §11 treats path escape as an error, not a result.
6. **Several same-name definitions in one file** (impl methods) pick the one whose `line_start` is nearest the requested `start`; `Definition` has no owner column to disambiguate by.
7. **`crates/rpc/bindings` mirror.** `just ts` writes to the gitignored `/bindings`; the tracked `crates/rpc/bindings/` is a partial mirror earlier work committed. The three new files were copied there (additive); `methods.json` has never lived there and was not added.

## Not built here (step 7b)

Reader batch on open, Verify, chips, "show current", `N anchors · M stale`, live window in the side pane, Ask to refresh — all UI; `components/board/*` untouched.
