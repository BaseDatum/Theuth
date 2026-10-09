# 153 as-built — wave A (closer Z): the merged engine on `integrate/153`

Workers C (contract/PIN), A1 shape, A2 dictionary, A3 history, A4 actor + RPC, A5 polish; their per-worker notes are
`153-as-built-{C,A1,A2,A3,A4,A5}.md`. This file is the merged tree, what Z reconciled, every §14 deviation in one table, the gates on
the merged tree, the probe, and the hand-offs for waves B (surfaces) and C (companion).

## The merged tree by file
| File | Owner | What |
|---|---|---|
| `crates/voice/src/lib.rs` | C (+A4 re-exports) | §14.1 types + `VoiceConfig` (`shape`, `end_commands`, `input_devices`, `standby_secs`, `duck_media`, `polish_model`, `polish_profiles`, `rewrite_template`, `history`); `pub mod shape/dictionary/history`; `pub use stt::{…, Stream, WINDOW}` kept beside the new re-exports (main `6aadda9a` fold verified) |
| `crates/voice/src/shape/{mod,tokens,punctuation,literals,numbers,dictionary_stage,end,tests}.rs` | A1 | §4 stages over one token list; `EndParser` (§7.1) owned by the actor; `Shaper::{new, with_surface, with_commands, with_mentions, shape, shape_with}`; `SETTLE_MS`/`SILENCE_MS` |
| `crates/voice/src/dictionary/{mod,matcher,packaged,store,tests}.rs`, `data/dictionary.v1.json.gz`, `data/dictionary.json`, `build-dictionary/build.py` | A2 | three layers; 40 641 packaged terms (290 KiB gz, `include_bytes!` + sha256); user layer `<data_dir>/voice/dictionary.json`; `lookup/lookup_scoped/put/delete/list/negative/get/pairs/counts/packaged_version` |
| `crates/voice/src/history/{mod,learn}.rs`, `migrations/0001_voice.sql` | A3 | sqlx `voice.db` (WAL, migrate on open), per-dictation `.pcm16` + `utterance_ends`, budget, `correction → Vec<Suggestion>`, `answer(id, answer, &mut Dictionary)`, `recording_allowed` |
| `crates/voice/src/dictation.rs`, `mic.rs` | A4 (Z reconciled) | the actor: opts, shaper per piece with the per-utterance parser, §7.3 settle, §7.4 scratch, §8.1–8.4 devices/standby, §8.7 timings, history rows + audio, dictionary snapshot, `Correction`/`AnswerSuggestion`/`DictionaryOp`/`Setting`/`HistoryStore` messages |
| `crates/rpc/src/api.rs` | C/A4/A5 | 17 new methods: `dictation_start(sample_rate, opts)`, `dictation_correction/history/history_forget/audio_get`, `dictionary_list/put/delete/suggestion_answer`, `list_input_devices`, `set_input_devices`, `set_shape`, `set_end_phrases`, `voice_status`, `polish_model`, `set_polish_model`, `polish`, `rewrite` |
| `crates/agent/src/{polish.rs, loop_/side.rs, loop_.rs, supervisor.rs, lib.rs}`, `crates/runtime/src/side_call.rs` | C/A5 | `CallKind::Polish`, `ModelRole::Polish` = `polish_model → title_model → session`, `Polish`/`Rewrite` side calls on the destination's session or theuth's desk, fail-closed reply check |
| `crates/tui/src/{lib,model}.rs` | C | additive adapt only (`dictation_start(None, None)`, new event variants ignored) |

## What Z reconciled (commits `153-Z: …`)
Order of events: Z first rewired items 1–2 in `dictation.rs` (`3e2864ed`); the orchestrator then superseded A4's DONE three times
(`9080aae5` → `c3bd3f3e` → final `02cefd6f`), A4 having done the same two items itself and being the authority for `dictation.rs`,
`mic.rs` and its `api.rs` regions. Z merged each (`4a7e1145`, `0c4997af`, `bef55869`), taking A4's `dictation.rs` wholesale (A4 dropped
Z's `recording()` helper and duplicate `parser` field on its side). The behaviour below is A4's final file, verified by Z against the
merged tree (clippy, tests, workspace check re-run after the last merge).
1. **Shaper wiring (A1 ↔ A4).** `ShaperOwned` carries `surface` + `commands`; per piece `Shaper::new(cfg, &phrases, &dict, app)
   .with_surface(surface).with_commands(opts.commands).shape_with(&mut parser, &prev, &t.text, t.is_final)`. One `EndParser` per task,
   `reset()` when the utterance index changes. A4's `armed` stickiness and the `#[cfg(test)]` exact-phrase shim are gone — the parser is
   sticky itself and strips `consumed()` words inside `Ctx::run`. `SETTLE`/`SILENCE` = `Duration::from_millis(shape::end::{SETTLE_MS,
   SILENCE_MS})` (the `voice::{SETTLE, SILENCE}` re-exports went — no caller). `with_mentions` stays empty: `DictationOpts` (frozen)
   carries no agent names.
2. **Suggestions (A3 ↔ A4).** `Correction` → `Store::correction(id, sent)` → each `Suggestion` published verbatim as
   `DictionarySuggestion { id, heard, typed, occurrences, surface, has_audio }`; A4's minted ids / `pending` map / `recent` ring /
   `occurrences: 2` / `has_audio: false` deleted. `AnswerSuggestion` → `Store::answer(id, answer, &mut self.dictionary)` (A3 writes the
   learned entry or negative), then the dictation snapshot is rebuilt for `Add`/`Never`. Skipped when `!history.learn`. The store is
   opened only when `Store::recording_allowed(&cfg.history, false)` (`open_stores`), so rows, audio and corrections are all off together;
   `Store::open(..).with_budget(rows, audio_mib << 20)` applies `voice.history`.
3. **Dictionary (A2 ↔ A4).** A4 loads once **off the mailbox**: `open_stores` spawns `spawn_blocking(Dictionary::load)` and the result
   lands as `DictionaryLoaded` (≈ 4 s debug, [INFERENCE] ≈ 0.2 s release); until then `DictionaryOp` answers `DictionaryReply::Loading`,
   which the RPC bodies map to the error "dictionary loading"; `snapshot()` per dictation; `dictionary_list/put/delete` → `DictionaryOp`
   → `list/put/delete`. `shape/mod.rs`: `Shaper::new` reads `dict.pairs(Kind::Path)`
   tagged `tld`/`ext` (leading `.` trimmed) **on top of** the built-in lists — not fallback-only as the brief asked: the packaged `Path`
   pairs are the talon lists (6 TLDs: `.co.uk .com .edu .net .org .us`; 35 extensions, no `rs`/`ts`/`dev`/`io`), so fallback-only
   would break table rows 5 (`theuth.dev`) and 7 / `main.rs`. The built-ins stay as the floor; the test `Ctx` keeps `tlds: &[]`.
4. **History row.** Confirmed in `Store::record`: `audio_bytes`/`audio_path`/`utterance_ends` come from the store's open `AudioFile`
   for that id, never from the row A4 passes (`audio_bytes: 0, has_audio: false` is ignored). Nothing to change.
5. **Fold check.** `crates/voice/src/stt.rs` identical to main `6aadda9a`; `pub use stt::{Engine, PIECE, Piece, Stream, …, WINDOW, …}` present.
6. Actor test harness: `stores_opened = true` under the fake engine (no `voice.db` in `$TMPDIR`, no 4 s packaged index per test) — Z's
   one line kept over A4's harness.
7. **Pre-existing issue fixed (orchestrator item d):** `live::tests::utterer_returns_the_spoken_sentence_punctuated` loaded the Nemotron
   streaming model whenever `~/.theuth/models/nemotron-streaming-en` and `say` were present; it is now
   `#[ignore = "loads the Nemotron streaming model (~0.9 GB) and runs `say`; run with --ignored"]`.

## Deviations from §14 (frozen) across C / A1–A5, consolidated
| Frozen shape | Shipped | Why |
|---|---|---|
| §14.1 message `SuggestionAnswer` (actor) | `AnswerSuggestion { id, answer }` | `history::SuggestionAnswer` is the enum (A4) |
| §14.1 `VoiceStatus.dictionary { … }` (§5.6) | no field; `dictionary_list` reports user + learned | `VoiceStatus` frozen at the PIN (A4) |
| §14.1 `Shaper::new/shape` only | + `with_surface`, `with_commands`, `with_mentions`, `shape_with(&mut EndParser, …)`; `Shaper::shape` = stateless (exact phrases only) | the parser is per utterance and the actor's (A1) |
| §14.1 `DictationOpts.commands` "carried, not applied" (A4) | applied through `with_commands` | Z |
| §6.3 `Store::correction → Vec<Correction>` | `Vec<Suggestion { id, heard, typed, occurrences, surface, has_audio }>` | `Correction` has no id to publish/answer (A3) |
| §6.3 `Store::answer(id, answer)` | `answer(id, answer, &mut Dictionary)` | the store writes the learned entry / negative; the actor owns both (A3) |
| §6.5 schema | + `dictations.utterance_ends TEXT NOT NULL DEFAULT ''` | per-utterance audio slices (A3) |
| §6.5 private session gate | `recording_allowed(&cfg.history, false)` | `DictationOpts` frozen without a session/private flag — wave B must add one (Z) |
| §5.2 `PackagedSet { terms, symbols, formatters, abbreviations, tlds, extensions }` | one `terms` list; `kind` + tags `tld`/`ext`/`number`; read via `pairs(kind)` | A2 |
| §5.4 fuzzy/phonetic at `ACCEPT_NEW` for packaged | packaged = normalised-form match only; fuzzy kept for user 0.70 / learned 0.85 | 108/300 prose words rewritten otherwise (A2 `prose_is_left_alone`) |
| §5.5 user layer in `voice.db` | `<data_dir>/voice/dictionary.json` (atomic rename) | self-contained store (A2) |
| §5.3 `Match` fields unspecified; `Scope` tuple variants | `Match { entry, canonical, score, tokens }`; `Scope` `#[serde(tag = "scope")]` named fields | ts/Swift mirrors (C) |
| §7.3 `SETTLE`/`SILENCE` in the actor | `voice::shape::{SETTLE_MS, SILENCE_MS}` | A1/Z |
| §8.2 retry 300 ms × 3 / 5 s | every 500 ms for 5 s | brief (A4) |
| §8.3 readiness + silent-device gate for every source | mic only | a remote feed is the client's (A4) |
| §8.4 resampler priming | skipped (sub-ms) | A4 |
| §8.6 duck/pause | not built; companion-side | design (A4) |
| §9 `Polish` with no session | charged to theuth's desk (resumed from the store if idle); no desk → `refused: "no session"` | A5 |
| §9.4 output rule | + length 0.3×–2× (+8 chars), one fence / quote pair stripped, `{`-led line refused | fail-closed (A5) |
| §9 `Message<Polish>` sync stub | `DelegatedReply<Result<PolishReply, String>>`; `Rewrite` a separate message | A5/C |
| §14.5 `CallKind.ts` | not exported (never was) | C |

## Gates (merged tree, `BUILD_LABEL=153-Z build-lock mbx …`, CARGO_BUILD_JOBS=4; logs under /tmp/153-z/)
- Pre-Z `mbx check -p voice -p rpc -p agent --tests` (orchestrator, /tmp/153-int-check.log): 2 errors, both `dictation.rs` —
  `suggest(…)` expected `Vec<Correction>`, found `Vec<Suggestion>`; `Store::answer` takes 3 arguments. Nothing else failed to compile.
- `mbx check -p voice -p rpc -p agent -p runtime -p tui --tests` → `Finished dev profile … in 2m 27s`, 0 errors, 0 warnings (check1.log).
- `mbx clippy -p voice -p rpc -p agent -p runtime --tests` → `Finished dev profile … in 2m 55s`, 0 warnings (clippy.log).
- `mbx test -p voice -p rpc -p agent -p runtime --no-run` → `Finished test profile … in 10m 28s`; bare `MBX_SCHEDULER_TESTS=0 mbx test
  -p voice -p rpc -p agent -p runtime` (test-run.log):
  agent `test result: ok. 592 passed; 0 failed; 2 ignored` (165.85 s; the two known load flakes `sensor::monty_budgets_and_allowlist`
  and `supervisor::human_input_releases_parked_items_as_steers` passed first time), `notebooks_binding` 2, `notebooks_block` 6;
  rpc `56 passed`, `prompts` 1; runtime `2 passed`; voice `test result: ok. 73 passed; 0 failed; 0 ignored` (6.87 s) = dictation 11
  (`send_it_settle_then_silence_stops`, `scratch_that_empties_previous_utterance` against A1's real parser), shape 24, dictionary 8,
  history 7, tests 8, vad 5, out 4, stt 3, clip 2, live 1; four doc-test crates `0 passed`.
- Bindings: `BUILD_SLOTS=1 … mbx test -p rpc --features ts --lib --quiet export_bindings` → `test result: ok. 8 passed; 0 failed; 56
  filtered out`; `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/`; both dirs gitignored (`git status` clean) — `bindings/methods.json`
  lists 448 methods including all 17 new names + `voice_status`.
- `BUILD_SLOTS=1 … mbx check --workspace --exclude desktop` → `Finished dev profile … in 3m 10s`, 0 errors, 0 warnings (workspace.log).
- `cargo fmt -p voice -p rpc -p agent -p runtime -p tui -- --check` → clean (exit 0, empty output).
- After the final A4 merge `02cefd6f` (`bef55869`), re-run on the final tree: `mbx check -p voice -p rpc -p agent -p runtime -p tui
  --tests` → `Finished dev profile … in 3m 17s`, 0 errors, 0 warnings (check3.log); `mbx clippy -p voice -p rpc -p agent -p runtime --tests`
  → `Finished dev profile … in 2m 21s`, 0 warnings (clippy3.log); `mbx test … --no-run` → `Finished test profile … in 5m 49s`; bare
  `MBX_SCHEDULER_TESTS=0 mbx test -p voice -p rpc -p agent -p runtime` (test-run3.log): agent `test result: ok. 592 passed; 0 failed;
  2 ignored` (173 s; the two known load flakes passed first time in all three runs), `notebooks_binding` 2, `notebooks_block` 6; rpc `56
  passed`, `prompts` 1; runtime `2 passed`; voice `test result: ok. 72 passed; 0 failed; 1 ignored` (7.84 s; the ignored one is the newly
  gated `live::tests::utterer_returns_the_spoken_sentence_punctuated`); `BUILD_SLOTS=1 … mbx check --workspace --exclude desktop` →
  `Finished dev profile … in 3m 21s`, 0 errors, 0 warnings (workspace3.log); `cargo fmt … -- --check` clean. Bindings not re-exported
  after the merges: no `ts(export)` type changed (`DictionaryReply` is engine-internal). Intermediate runs after `9080aae5`
  (clippy2/test-run2/workspace2) were equally clean.

## Live probe (§16 row 11)
**Skipped** — the brief's condition failed at gate end (19:42): `uptime` 1-min load 23.57 (< 25 ok) but `vm_stat` free + speculative
pages 2.45 GB (needs > 4 GB; 15-min load 39.6, the machine is shared with other waves' builds). The model dir
`/Users/Sky.Moore/.theuth/models/nemotron-streaming-en` exists. Exact recipe for whoever runs it:
1. `BUILD_SLOTS=1 BUILD_LABEL=153-Z build-lock mbx build -p theuth` (≈ 15 min), `cp target/debug/t /tmp/153-z/t`.
2. `/tmp/153-z/data/config.yaml` (BLOCK style): `agent: {model: anthropic/claude-haiku-4-5}`, `secrets: {key_store: file}`,
   `tools: {approvals: false}`, `rpc: {listen: 127.0.0.1:7799}`, `mesh: {relay: none, discovery: false, bind: 127.0.0.1:0}`, every other
   plugin `{enabled: false}` except `voice: {enabled: true, dictation_model_dir: /Users/Sky.Moore/.theuth/models/nemotron-streaming-en}`.
   Never copy `secrets.*`; never touch `~/.theuth`.
3. `perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' -- env -u THEUTH_RPC__LISTEN -u THEUTH_LAUNCH /tmp/153-z/t --data-dir /tmp/153-z/data
   --headless > /tmp/153-z/engine.out 2>&1 < /dev/null &`
4. jsonrpsee over WebSocket (address in `<data>/engine.json`, bearer token in `<data>/token`; a bun script with native `WebSocket`):
   `subscribe_events`; `dictation_start [16000, {"surface":"tui"}]`; 30 × `dictation_audio [id, <base64 i16 16 kHz, 100 ms of a 1 kHz
   tone then silence>]`; `dictation_stop [id]`. Expect `DictationText` events carrying `shaped` (empty text is fine for a tone),
   `DictationFinished`, then `dictation_status.last` with `press_to_first_pcm_ms` / `press_to_first_partial_ms` /
   `press_to_first_final_ms` / `stop_to_final_ms`; `dictation_start [16000, null]` → the same event shape with `Composer` presets.
5. `pkill -f '153-z/t'`.
What the unit tests already prove on the same path (fake engine, real shaper/parser): `none_opts_is_today` (opts `None` → `shaped ==
text`, no command, `Finished { Done }`, `last.surface == Composer`), `timings_recorded_with_synthetic_feed` (the four numbers),
`send_it_settle_then_silence_stops` (1.5 s + 0.35 s), `scratch_that_empties_previous_utterance`.

## Hand-offs
**Wave B (desktop / TUI surfaces)**
- Send `dictation_start(rate, opts)` with `surface`, `destination`, `commands`, `press_ago_ms`; read `shaped`, act on `command: send`
  after `DictationFinished`; a `DictationText { final: true, text: "" }` for an earlier utterance = scratch (empty that splice).
- Terminal spacing: on a final ending in `/cmd` or `@name` skip the splice separator (`crates/tui/src/dictation.rs::trail`).
- `dictation_correction(id, sent_text)` at send; render `DictionarySuggestion` (ids from A3; auto-dismiss 20 s = ignored, 3 per
  session), answer with `dictionary_suggestion_answer`; the card plays `dictation_audio_get(id, None)` (whole dictation —
  `Correction.utterance` is always 0).
- Privacy: the engine cannot see a private session; add the flag to `DictationOpts` (frozen → integrator) before a private surface dictates.
- `polish(text, destination, session)` once per finished utterance, never on partials, never `AgentPrompt`; `refused` → deliver shaped;
  `polish_model()` / `set_polish_model(id | null)` for Settings › Voice; `rewrite(instruction, selection, app, thread)` per §13.6.
- Settings: `list_input_devices` / `set_input_devices` (`voice_status.input_devices` is live), `set_shape`, `set_end_phrases`,
  `dictionary_list/put/delete`; the dictionary header line reads `packaged_version()` + `counts()` through a future `VoiceStatus` field.
- `@mention` shaping — **wave-B / design follow-up, not wired in wave A:** `Shaper::with_mentions` exists but `DictationOpts` (frozen
  §14.1) carries no agent names and the engine has no other channel for them; the stage still applies the dictionary's `mention`-tagged
  user/learned entries. Design decision needed: a `mentions: Vec<String>` on `DictationOpts`, or the actor asking `agents_list` itself.
**Wave C (companion)**
- `DictationDevice { lost | changed, device }` in the level slot; `dictation_status.input_device` / `.last` (§8.7 numbers) for the debug row.
- Duck/pause (§8.6) is the companion's; `voice.duck_media` is already read into config.
- `AppProfile.polish_prompt` is not appended by `polish` (no `AppProfile` on the RPC); put per-app text under `voice.polish_profiles.<bundle id>`.
- Rebuilding the dataset: `python3 crates/voice/build-dictionary/build.py` (network; `--offline` reuses `cache/`), commit `data/*`.

## F1 (2026-10-09 23:05) — follow-up fix: ts-rs name collision + `voice_settings` getter
Branch `fix/153-f1` off main `f4834cf3`; three commits `153-F1: …`.
- **Finding 1 — bindings collision.** `crates/voice/src/dictionary/mod.rs`: `Kind` / `Source` carry `ts(export, rename = "EntryKind")` /
  `ts(rename = "EntrySource")` (Rust names unchanged; Swift mirrors were already `EntryKind`/`EntrySource`). Regenerated bindings:
  `EntryKind.ts` / `EntrySource.ts` appear, `Entry.ts` imports them, `Kind.ts` is the artifact kind again and `Source.ts` the knowledge
  source; `ls bindings | sort | uniq -d` is empty (847 files). No UI import of the voice `Kind`/`Source` existed (`extra.ts` imports the artifact `Kind`).
- **Finding 2 — `voice_settings()`.** `crates/voice/src/lib.rs`: `VoiceSettings { shape, end_commands, input_devices, standby_secs, duck_media,
  polish_model, polish_profiles, history }` + `From<&VoiceConfig>`; `HistoryConfig` now ts-exported. `dictation.rs`: `CurrentSettings` message —
  the actor's live keys (shape / phrases / devices / standby / history, as the `Setting` handler left them) over `rt.section("voice")` for the
  config-only keys. `rpc/src/api.rs`: `voice_settings()` in the trait + the "153 dictation history / dictionary" region; `polish_model` is overlaid
  from `Agents`' `GetPolishModel` slot (the `voice_status` ↔ `InputDevicePrefs` pattern). `export.rs` lists `HistoryConfig`, `VoiceSettings`.
- **Readers.** Desktop `components/settings/Voice.tsx`: `useQuery(voice_settings)`; `set_shape` / `set_end_phrases` / `set_input_devices` /
  `set_polish_model` invalidate it; the `useRemembered` localStorage memory is deleted; shaping selects stay disabled until the getter answers;
  phrase rows are keyed on the server value. `rpc/api.ts` + `METHODS` (sorted). TUI `lib.rs`/`model.rs`: `/voice` adds
  `shape: tui … · composer … · companion … · phone … · any app …` from `voice_settings` (a failed read drops the row, like the device list).
  Companion untouched (DictationSettings still points at the desktop).
- **Tests.** voice `voice_settings_round_trip_defaults` (field-for-field mirror + serde round trip); tui `voice_cmd_parses` extended (no settings →
  no `shape:` row; tui `plain` prints); `Voice.test.ts` asserts `voice_settings` is called once and the end-phrase inputs show its values.
- **Gates** (`/tmp/153-f1/`): `mbx check -p voice -p rpc -p tui --tests` 0 err/warn · `mbx clippy …` 0 warn · `mbx test … --no-run` + bare
  `MBX_SCHEDULER_TESTS=0 mbx test -p voice -p rpc -p tui` → 298 passed, 0 failed (rpc 56, tui 166, voice 73 + 1 ignored) · bindings export ok,
  `methods.json` has `voice_settings` · `tsc --noEmit` 0 errors · `vitest run api.test.ts Voice.test.ts dictation.test.ts` 3 files / 15 tests pass ·
  `cargo fmt -p voice -p rpc -p tui -- --check` clean.
- **Deviations.** First bare test run hit `rpc engine::tests::second_acquire_in_one_process_is_running` ("engine.lock is held but engine.json is
  missing") — untouched file, passed on rerun; flaky under the shared machine. `standby_secs` is `bigint` in TS (u64). Radix `Select` renders no
  value text under `renderToStaticMarkup`, so the preset itself is proven by the Rust tests, the UI test by the enabled trigger + phrase inputs.

## F2 (2026-10-10 00:25) — tail flush: words spoken just before `dictation_stop` were dropped
Branch `fix/153-f2` off main `bfafefeb`; commits `153-F2: …`. Finding: Windows live round (engine 2.14.0+f4834cf3) lost "send it" with no
tail, kept "Send" with 1.2 s, all with 2.5 s. Two causes, both in `crates/voice`.
- **Cause 1 — queued audio dropped at stop** (`dictation.rs`). `Task::run` broke on `cancel.is_cancelled()` before reading `feed.rx`, so the
  20 ms chunks already delivered (and the sub-chunk remainder in `Remote.pending`) never reached the model; `live.close()` flushed only what
  had been fed. Now: `StopDictation` pushes `Remote.pending` zero-padded to a full chunk ahead of the cancel; `PushAudio` after a stop
  answers "not dictating" (`cancel.is_cancelled()`, no new field); on `Ok(Done)` (cancel or the §7.3 settle — not deadline, not error) the
  task runs `Task::drain`: `feed.rx.try_recv()` until empty through `self.chunk(_, false)`, bounded to `DRAIN_MAX` = 5 s of audio at the
  source rate (a mic keeps delivering). `stop_at` stays the instant the stop was seen, so `stop_to_final_ms` carries the drain's cost.
  Mic path uses the same loop → same drain. `StopDictation` doc now says what it does.
- **Cause 2 — one zero window flushed the decoder** (`live.rs`). `Utterer::finish` fed the padded partial window then exactly one zero
  window. Now `keep_flushing(fed, new_text)`: zero windows until one yields no new text, floor `FLUSH_MIN` = 2, ceiling `FLUSH_MAX` = 6
  (1.1–3.4 s of synthetic right context); depth in `tracing::debug!("voice: utterance N closed after D zero windows")`. `Text`/`Done`/
  `DictationTimings` unchanged (no field fits). `piece()`'s stray-lead-in handling kept. VAD `Edge::Close` goes through the same `finish`.
- **Tests.** `dictation::tests::stop_drains_queued_audio_before_close` (deterministic: `Fake` gained `delay` per chunk = a model slower than
  real time; one `PushAudio` of 60 chunks + half a chunk, `StopDictation` at once, final scripted on the padded 61st chunk; asserts final
  text, `command: Send`, `Done`, audio after the stop refused). Unfixed tree: `panicked … "the final before the stop"` (finished in 0.01 s, no
  text at all). Fixed: ok. `live::tests::flush_floor_then_until_silent_then_ceiling` (bookkeeping). Model-backed, `#[ignore]` like the existing
  one: `close_right_after_the_last_word_keeps_send_it` (no tail) and `close_after_a_short_tail_keeps_send_it` (1.2 s); both print
  `close()` cost and one zero-window step cost (`tail Xs: close() …, one zero window …`).
- **Model-backed tests NOT run** — `vm_stat` free + speculative 2.60 GB at start, 2.61 GB at gate end (needs ≥ 4 GB; shared machine). So the
  cause-2 before/after and the per-step encoder cost are unmeasured here. Exact commands (worktree `~/Develop/theuth-153-f2`, PATH line
  from WORKING-RULES-BC, test binary already built):
  `MBX_SCHEDULER_TESTS=0 mbx test -p voice -- --ignored --nocapture live::tests::close_` (before: `git stash`/checkout `92f02ea8` for the
  unfixed `finish`, expect the no-tail test to fail). If one zero window prints > 150 ms, the ceiling of 6 costs > 0.9 s per close — then
  lower `FLUSH_MAX`.
- **Gates** (`/tmp/153-f2/`): `mbx check -p voice --tests` 0 err/warn (check.log) · `mbx clippy -p voice --tests` 0 warn (clippy.log) ·
  `mbx test -p voice --no-run` + bare `MBX_SCHEDULER_TESTS=0 mbx test -p voice` → 75 passed, 0 failed, 3 ignored (test.log) ·
  `cargo fmt -p voice -- --check` clean.
- **Deviations.** The brief's test 0 named model-backed harness tests in `dictation.rs`; that module is fake-engine only, so the reproduction
  uses the fake with a per-chunk delay (the slow-model condition the finding describes) and a scripted final, not a synthesized clip.
