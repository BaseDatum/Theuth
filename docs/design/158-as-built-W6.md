# 158 as-built — W6 (gated quality): the echo counter, the gate measurements, one flaky test

Worker W6E, worktree `theuth-158-w6e`, branch `feat/158-w6e` from main `0ac16fa3` (W1–W5 on main). Design §13, §14, §16 W6 row.
Commits: `6ee0be38` (A + B), `1c6ff8db` (C), + this file. Nothing gated was built; §14 items are proposals below.

## A. Echo counter (W-o) — no behaviour change
- `crates/meeting/src/post/echo.rs` (new): `count(&[Segment]) -> Stats` — a mic segment is an echo duplicate when a system
  segment overlapping it within ±2 s (`WINDOW_MS`) has Jaccard ≥ 0.8 (`THRESHOLD`) over lower-cased alphanumeric word sets;
  `tokens` / `jaccard` / `similar` helpers; nothing is dropped. Called once in `post/mod.rs::run` before `state = Review`
  (`stats.speakers` survives a re-run). Unit tests: exact duplicate within 2 s counts (case-insensitive), outside ±2 s does not
  (edge at exactly 2 s counts), below 0.8 does not, system/empty segments never count.
- `Manifest.stats: Option<Stats>` (contract stub, was always `None`) now filled; `Stats` gains `speakers: Option<bool>`
  (`#[serde(default)]`; TS `MeetingStats.speakers: boolean | null`). `store.rs` test: a manifest without `stats` and a `stats`
  without `speakers` parse; a counted manifest round-trips.
- RPC `meeting_set_speakers(id, speakers: bool) -> Manifest` (new, smallest; `meeting_edit` only touches the transcript) —
  `api.rs` beside `meeting_set_template`, body `post::echo::rpc_set_speakers` → actor `SetSpeakers` (same shape as
  `SetTemplate`); a manifest without `stats` gets zeros + the answer. `rpc/api.ts` `Api` + `METHODS`; bindings re-exported.
- UI: `settings/Meetings.tsx` "Echo" section (hidden on macOS via `isMacDesktop()`): `meeting_list(10)` → `meeting_get` each →
  `echoSummary()` = "N % of your microphone segments repeated the other side (d of m) over the last k meetings, s on speakers."
  `meeting/Review.tsx` asks once per meeting outside macOS "Did you use speakers (not headphones) in this meeting?" (Speakers /
  Headphones → `meeting_set_speakers`, manifest cache replaced); hidden once `stats.speakers` is set, shown in read-only states too.

## B. Measurements — `crates/meeting/tests/measure_158.rs` (`#[ignore]`, counts only)
`THEUTH_MEETINGS_DIR=~/.theuth/meetings THEUTH_SPEAKERS_DIR=~/.theuth/meetings/speakers mbx test -p meeting --test measure_158 -- --ignored --nocapture`
Sky's disk at 18:50: 5 meetings with manifest + transcript (ids `01a12243`, `01a0e8f9`, `01a0c93b`, `01a0c47f`, `01a0c444`), 3 Done.

| # | Item (§14 / §13) | Measured | Gate | Tripped? | What would ship |
|---|---|---|---|---|---|
| 1 | Echo (§13) | mic segments 40 / 115 / 0 / 57 / 33; echo dupes 0 / 1 / 0 / 0 / 0 → ratios 0.0 / 0.9 / 0.0 / 0.0 / 0.0 %; median 0.0 %; speakers answer `None` ×5 (macOS, VoiceProcessingIO) | median ≥ 5 % over ≥ 5 speaker meetings | No (and no Windows/Linux sample yet) | nothing; on a trip: bleed check (drop a duplicate from the transcript, not the audio) first, AEC3 (`webrtc-audio-processing`) only after a two-host build |
| 2 | Post-STT repetition guard | last 20 (= 5) meetings, 754 segments: 0 runs of ≥ 3 consecutive same-track segments at ≥ 0.8, 0.00 % of segments | measured on Parakeet over 20 meetings | No (Parakeet, not Whisper) | nothing |
| 3 | Window-read names | last 10 (= 5): 49 non-me labels, 0 named by hand, suggestions 0 / 0 / 0 / 2 / 1 → 46 unnamed after `suggest` (94 %); no meeting has an event, 0 enrolled profiles | ≥ 20 % unnamed over 10 meetings | **Yes, but on an empty input**: no calendar event and no voiceprint on any of the 5 — the §10 sources are absent, not failing | re-measure after ≥ 10 meetings with calendar on; if still ≥ 20 %, macOS AX read-only (§14) |
| 4 | Decisions schema | Done 3; projects with Done meetings 0; max Done in one project 0; Done without project 3 | ≥ 10 Done meetings in one project | No | nothing |
| 5 | Multilingual tier | manifests with a language: 0 of 5 (no per-meeting field); `meeting.language` not set in config → `en` | a non-English meeting, or Sky asks | No | nothing; `Manifest.language` would be the first step if asked |
| 6 | Detection poll cost | 100 ticks of `detect::sensor::mic_holders()` = 568.5 ms → **5.69 ms/tick** (0 holders, 0 errors), i.e. ≈ 0.3 % of a core at the 2 s poll | the tick shows in `top` | No | nothing; Core Audio listener if it ever does |
| 7 | Hybrid search | not measurable without Sky: needs a 50-query set (query → expected meeting/segment) written against his meetings; FTS-only `meeting_search` gives the baseline miss rate | FTS baseline ≥ 20 % misses | Unmeasured | collect 50 queries (10 per meeting type, mix of exact phrase / paraphrase / name) → run `meeting_search` → count misses; ≥ 10 misses → `segments_vec` + RRF (144's `EmbedActor`) |

Observation (no content read): `~/.theuth/meetings/` holds stray top-level `manifest.json`, `transcript.json`, `live.jsonl`,
`briefs.jsonl`, `embeddings.json`, `summary.md`, `*.opus` beside the id dirs — one meeting's files written to the parent dir.
Not touched; worth a look by whoever owns `Store`.

## C. Flaky `rpc::engine::tests::second_acquire_in_one_process_is_running` — a real ordering race in `acquire`
- Reproduced in-process only: 0/20 alone and 0/40 under external load, but **7/12** when the whole `rpc` unit binary runs. Diag
  (temporary `lsof` + retry loop, removed): after `drop(lease)` the third `acquire` saw `WouldBlock` with no fd of ours open on
  `engine.lock`; it recovered within the next attempt. A `ps` sampler showed `/bin/zsh` children of the test process during
  `search::tests` (`tools::Shell` login-shell PATH probe, `platform::env::login_path`). Mechanism: fork/spawn copies the fd table,
  so the child's reference keeps the `flock` alive past our `close()` until it execs; `acquire` polled only `engine.json` (already
  unlinked by the drop) and never retried the lock → "engine.lock is held but engine.json is missing".
- Real-world analogue: an engine exiting while spawning anything, with a second `t` starting in that window → the same refusal.
- Fix (code, `crates/rpc/src/engine.rs::acquire`, 16 lines): the 10 × 20 ms grace loop retries `try_lock` and reads the record on
  each miss (record → `Running`; lock → proceed; neither after 200 ms → the same `Io` error). The test is unchanged.
- Proof: full `rpc` unit binary × 20 → 56 passed every run; the single test × 20 while a full suite ran in parallel → 20/20 green.

## Libraries
- Jaccard over word sets: `strsim` (workspace dep) has char-level Jaro/Sørensen–Dice, not word-set Jaccard; `autoresearch::brief::jaccard`
  is `pub(crate)` in another crate. 12 lines of stdlib `HashSet` instead. No new dependency.

## Deviations
- `meeting_set_speakers` is a new RPC (not in §15); `meeting_edit` edits the transcript only.
- The Settings row reads 10 manifests client-side (`meeting_get` ×10) rather than adding `stats` to `MeetingSummary` (frozen index row).
- The speakers question is shown in Done/Summarizing too (not Review only): it is a manifest flag, not a transcript edit.
- Measurement is an integration test (`tests/measure_158.rs`), not an example binary.

## Hand-offs / open items
- Live UI proof of the Echo row and the Review question needs a Windows/Linux host (both hidden on this Mac by design).
- Item 3 needs re-measuring once calendar-matched meetings exist; item 7 needs Sky's 50 queries.
- Stray meeting files at `~/.theuth/meetings/` top level (above).

## Gates (each its own command, `BUILD_LABEL=158w6e`, `CARGO_BUILD_JOBS=4`)
- `cargo fmt -p meeting -p rpc` (bare) → clean.
- `build-lock mbx test -p meeting -p rpc --no-run` → 0, no warnings.
- `mbx test -p meeting -p rpc` → meeting 125 passed (2 ignored) + calendar 15 + note 1 + oracle 1; rpc 56 + alerts 1 + prompts 1; 0 failed.
- `build-lock mbx clippy -p meeting -p rpc` → see below.
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → 8 passed; copied to
  `crates/desktop/ui/src/rpc/bindings/` (gitignored); `MeetingStats.speakers`, `methods.json` + `meeting_set_speakers`.
- `build-lock bun install` (ui 623, packages/apps 484) → ok. `build-lock bunx tsc --noEmit -p .` → 0.
- `build-lock bunx vitest run` Review, settings/Meetings, rpc/api → 3 files, 12 tests passed.
- Measurement run above (1 passed, `--ignored`).
- `BUILD_SLOTS=1 build-lock mbx check --workspace --tests` → see below.
