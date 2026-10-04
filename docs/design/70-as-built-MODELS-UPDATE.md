# 70 as built — MODELS-UPDATE: the desktop updates changed model files, not only missing ones

Branch `fix/models-update-by-sha` off `origin/main` 6797885f, 2026-10-04.

## Problem

`crates/desktop/src/models.rs` `present()` was `is_file && len == e.size`. `cklxx/laya-browser` v17s → v19s
keeps every size (`model.safetensors` 643,835,524 B), so an installed machine never saw v19s. Confirmed on an
APFS clone of this Mac's `~/.theuth/models` against the live `models.json`: `decision-browser/model.safetensors`
and `rl_agent_config.json` are same-size/other-sha, `encoder/config.json` differs in size (1939 vs 1968 B) and
was the only one the old code would have fetched.

## What changed

- **`crates/platform/src/sha256.rs`** (new; `platform::sha256`): the `<file>.sha256` sidecar — `<hex> <size> <mtime_secs>\n` —
  moved out of `decision::candle::sha256_cached` so both crates share one format. `sidecar()` :35 (read, `None` when
  absent/stale), `remember()` :47 (write from a hex the caller has; best effort, debug log), `cached()` :56 (sidecar or
  hash-and-remember). Test `sidecar_format_and_short_circuit` pins the exact string and the size/mtime short-circuit.
  `platform` gains `sha2` + `tracing`; `decision` gains `platform` (`candle/mod.rs:232`, `gliner/candle.rs:166` call it).
- **`crates/desktop/src/models.rs`** (module doc rewritten):
  - `status()` :293 replaces `present()`: `Keep` (size + remembered hash, or size only when the manifest row has no
    `sha256` — debug log), `Hash` (size matches, no current sidecar), `Fetch` (absent / other size / sidecar hash ≠ manifest).
    `Entry.sha256` is `#[serde(default)]` so old manifests parse.
  - `sync()` :198 is the testable core (manifest → keep/hash/fetch → download → land or defer). Hashing runs in
    `spawn_blocking` and emits `Progress::Verifying { file }` :98 first; the JSON of the three existing states is unchanged
    (`progress_json` test).
  - `download()` :316 returns the verified `.part`; `land()` :358 renames and writes the sidecar from the streamed hash, so a
    fresh install never hashes twice.
  - Deferred swap: `Entry::checkpoint()` :78 maps `decision/` → `default`, `decision-<name>/` → `<name>`. Before replacing an
    *existing* file of a checkpoint, `loaded_checkpoints()` :382 asks the daemon; if listed, the `.part` goes to `Pending` :117
    and `start()` :126 retries `apply_pending()` :365 every `PENDING_RETRY` (60 s) until `decision_loaded` no longer lists it
    (info log "… updated on disk, applied when the checkpoint unloads" / "… applied"). Missing files and non-decision files land
    at once (`.part` + rename is atomic; the daemon opens lazily).
  - `decision_loaded()` :401: the desktop had no Rust RPC client; the daemon's jsonrpsee server accepts plain HTTP POST as well
    as WebSocket and `?token=` authorises both, so this is one `reqwest` POST to `conn.url` with `ws://` → `http://`,
    5 s timeout. `launch::Launch::peek()` (`launch.rs:114`) gives the cached connection or a probe — never spawns a daemon;
    unreachable = `[]` = nothing loaded.
  - `start()` emits `Ready` when anything was verified or downloaded (a verify-only first pass would otherwise leave the
    `verifying` toast spinning).
  - Pruning is not done; the module doc says so.
- **UI** `ui/src/lib/tauri.ts:69` adds `{ state: 'verifying'; file }`; `ui/src/App.tsx:243` renders it as the same loading toast
  ("Verifying models…", file as description).
- **Docs**: 57 §12.4 line after the v19s note; `70-as-built-MODELS-FRESH.md:38` ceiling struck through.

## Verification

- `cargo test -p platform sha256`: 1 passed.
- `cargo test -p desktop models::`: 9 passed, 0 failed — (a) `same_size_replacement_is_fetched_once_and_remembered`,
  (b) same test: sidecar answers after a same-size/same-mtime content change (the accepted cost), (c)
  `manifest_without_sha256_compares_size_only`, (d) `loaded_checkpoint_replacement_is_deferred_until_unload`, (e) `verifying`
  asserted once on pass 1 and absent on passes 2–3, plus `checkpoint_by_directory`, `progress_json`,
  `decision_loaded_names_checkpoints_and_refuses_nothing_reachable`, and the two pre-existing manifest tests.
- `cargo clippy -p desktop --tests -- -D warnings`: clean. `cargo check -p desktop --tests`: builds on this Mac without the bundle.
- `cargo check -p decision --tests`, `cargo test -p decision`, `cargo check --workspace --tests`: see the report
  (`/tmp/69/report-MODELS-UPDATE.md`) for the numbers.
- RPC smoke: `~/Develop/theuth/target/release/t --headless --data-dir /tmp/69/data` with `THEUTH_RPC__LISTEN=127.0.0.1:47771`;
  `curl -X POST 'http://127.0.0.1:47771/?token=…' -d '{"jsonrpc":"2.0","id":1,"method":"decision_loaded","params":[]}'` →
  `{"jsonrpc":"2.0","id":1,"result":[]}`; without the token → 401.
- First-launch hashing time: APFS clone of `~/.theuth/models` (8.0 GB, 71 files, not the 3.5 GB the brief expected — it also holds
  `decision-fp32`, `decision-onnx-ref`, nemotron), `platform::sha256::cached` over every file, release build, machine under a
  concurrent `cargo test -p decision`: **pass 1 22.0 s (8.58 GB, ~390 MB/s); pass 2 13 ms** (sidecars). The manifest's 25 rows
  for this platform total 5.76 GB on disk → ~15 s on the first launch, 0 after. A debug (`cargo tauri dev`) build hashes
  several times slower; the `verifying` toast covers it.
- `bunx tsc --noEmit` in `crates/desktop/ui`: 0 errors in the touched files. The worktree has no generated `src/rpc/bindings/`;
  with the main worktree's bindings copied in, 7 errors remain, all in `src/store/reduce.ts` / `reduce.test.ts` about
  `ReasoningDone` / `thinking_ms` (bindings older than this branch's `rpc`), none in `App.tsx` / `tauri.ts`.

## Deviations / ceilings

- Shared place is `platform::sha256`, not a `pub` in `decision`: `desktop` does not (and should not — candle/Metal) depend on
  `decision`; `platform` is a leaf both already reach.
- `Entry::checkpoint()` follows the directory convention only; a `decision.checkpoints.<name>.dir` override pointing elsewhere
  is not read by the desktop (it would land the file at once, as before this change).
- `loaded_checkpoints` is asked after each download, right before the swap, not once per pass: at `setup()` the daemon is
  normally not up yet, by the time a 600 MB file lands it is.
- A `Pending` list lives in the `start()` task only; quitting the app before the checkpoint unloads leaves `<dest>.part`, which the
  next launch re-downloads (sidecar ≠ manifest → `Fetch`) and overwrites. Not reused: YAGNI.
- A hashing I/O error on one file fails the pass (as a download error did before); the next launch retries.
