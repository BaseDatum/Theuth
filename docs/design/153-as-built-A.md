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
Order of events: Z first rewired items 1–2 in `dictation.rs` (`3e2864ed`); the orchestrator's 19:10 message then superseded A4's DONE
with `feat/153-a4` `9080aae5`, which does the same two items itself. Z merged it (`4a7e1145`) resolving `dictation.rs` toward A4's
version, then A4's final `c3bd3f3e` (`0c4997af`; as-built line only, no crate change); the behaviour below is A4's final file, verified
by Z against the merged tree (re-ran clippy + the test gate after the merge).
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
3. **Dictionary (A2 ↔ A4).** A4 already loads once (`spawn_blocking(Dictionary::load)` inside `open_stores`, awaited on `Prewarm` /
   first start — ≈ 4 s debug, [INFERENCE] ≈ 0.2 s release, the actor mailbox waits that long once) and snapshots per dictation;
   `dictionary_list/put/delete` → `DictionaryOp` → `list/put/delete`. `shape/mod.rs`: `Shaper::new` reads `dict.pairs(Kind::Path)`
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
- After the `9080aae5` merge (`4a7e1145`), re-run on the final tree: clippy same set → `Finished dev profile … in 3m 11s`, 0 warnings
  (clippy2.log); `test --no-run` → `Finished test profile … in 5m 13s`; bare run (test-run2.log): agent `592 passed; 0 failed; 2
  ignored`, notebooks 2 + 6, rpc `56 passed` + prompts 1, runtime `2 passed`, voice `test result: ok. 72 passed; 0 failed; 1 ignored`
  (the ignored one is the newly gated `live::tests::utterer_returns_the_spoken_sentence_punctuated`); workspace check `--exclude desktop`
  → `Finished dev profile … in 1m 29s`, 0 errors (workspace2.log); `fmt -- --check` clean. Bindings not re-exported: no ts type changed.
  The later `c3bd3f3e` merge (`0c4997af`) touched only `153-as-built-A4.md`, so these lines are the final tree's.

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
