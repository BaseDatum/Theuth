# 153-C as built — contract / PIN (worktree `theuth-153-c`, branch `feat/153-c`)

PIN: see `theuth-153-briefs/PIN.txt` (first PIN `a2002179`; updated to `bbdd03c4` when `VoiceStatus` gained `input_devices`).
Behaviour is not in this wave slice: every new body is a no-op marked `// 153-C stub — <worker> fills`.

## Files
| File | What |
|---|---|
| `crates/voice/src/lib.rs` | `Surface`, `Destination`, `DictationOpts`, `AppProfile`, `DictationTimings`, `DeviceState`, `InputDevice`, `Duck`, `HistoryConfig`; `VoiceEvent::DictationText` + `shaped/command/actions` (serde default); `VoiceEvent::{DictationDevice, DictionarySuggestion}`; `VoiceStatus.input_devices`; `VoiceConfig` + `shape, end_commands, input_devices, standby_secs (8), duck_media, polish_model, polish_profiles, rewrite_template, history`; `pub mod shape/dictionary/history` + re-exports |
| `crates/voice/src/dictation.rs` | `DictationState` + `input_device`, `last` (serde default); `Progress::Text` → event fills `shaped = text`, `command: None`, `actions: []`. **A4's file** — only the two §14.1 hunks, nothing else |
| `crates/voice/src/shape/{mod,end}.rs` | §4.1 verbatim: `ShapeConfig`, `Shaped`, `EndCommand`, `Action`, `Replaced`, `RuleId(String)`, `EndPhrases` (+ §7.2 defaults), `Shaper<'a>::{new, shape}` (returns raw); `Preset`, `ShapePresets` (+ `get(surface)`), `From<Preset> for ShapeConfig`; `end::EndParser::{new, feed, reset}` |
| `crates/voice/src/dictionary/{mod,matcher}.rs` | §5.3: `Entry`, `EntryId(String)`, `Kind`, `Source`, `Scope { Global, Surface{surface}, App{app} }`, `Variant`, `Negative`, `Match { entry, canonical, score, tokens }`; `Dictionary::{load, snapshot, lookup, put, delete, list, negative, packaged_version, counts}`; `ACCEPT_KNOWN 0.70`, `ACCEPT_NEW 0.85`, `EDIT_MAX 2`; `matcher::score → 0.0` |
| `crates/voice/src/history/{mod,learn}.rs` | §6.5: `HistoryRow`, `Correction`, `SuggestionAnswer`, `SuggestionId(Uuid v7)`, `Store::{open, record, attach_audio, list, forget, audio, correction, answer}` (async; `db: Option<SqlitePool>` = None); `learn::{candidates, due}` |
| `crates/voice/Cargo.toml`, `Cargo.toml` | `sqlx` (workspace, artifact's features), `rphonetic = "4"`, `strsim = "0.11"`, `fst = "0.4"` (optional in voice). No `cargo update`; the lock gains only the new crates |
| `crates/runtime/src/side_call.rs` | `CallKind::Polish` ("polish") in `as_str`/`from_str`/round-trip test; `ModelRole::Polish` |
| `crates/agent/src/loop_/side.rs` | `ModelRole::Polish` arm — resolves like `Title` for now (A5: `polish_model → title_model → session`) |
| `crates/agent/src/lib.rs` | `SetPolishModel`, `GetPolishModel`, `Polish { text, destination, session }`, `Rewrite { instruction, selection, app, thread }`, `PolishReply` (ts) |
| `crates/agent/src/supervisor.rs` | `Agents.polish_model` slot (read from `voice.polish_model` via `rt.section::<voice::VoiceConfig>("voice")` at spawn; test ctor `Arc::default()`), `SetPolishModel`/`GetPolishModel` handlers mirroring title (setter persists `write_config_value(.., "voice", "polish_model", ..)`), `Polish`/`Rewrite` handlers → `Agents::polish` stub `{ refused: Some("not built"), .. }` |
| `crates/rpc/src/api.rs` | `dictation_start(sample_rate, opts)`; `polish_model`/`set_polish_model` beside `title_model`; region "153 dictation history / dictionary" (`dictation_correction`, `dictation_history`, `dictation_history_forget`, `dictation_audio_get`, `dictionary_list/put/delete`, `dictionary_suggestion_answer`, `list_input_devices`, `set_input_devices`, `set_shape`, `set_end_phrases`) and region "153 polish / rewrite" (`polish`, `rewrite` → `ask::<Agents,_>`) |
| `crates/rpc/src/export.rs` | every new ts type listed (§14.5 + `Kind`, `Source`, `Variant`, `Negative`, `Scope`, `EntryId`, `SuggestionId`, `DeviceState`, `InputDevice`, `Duck`, `EndPhrases`, `Preset`, `ShapePresets`) |
| `crates/tui/src/{lib,model}.rs` | **shared file, not in the wave-A table**: `dictation_start(None, None)`; `DictationText { .., }` in the match; the two new variants in the ignore arm; the four test constructors gain the three fields. No behaviour change |
| `crates/desktop/ui/src/rpc/bindings/` | re-exported |

## Exhaustive-match sites touched
`CallKind`: `side_call.rs` `as_str`, `from_str`, round-trip test (the only matches). `ModelRole`: `agent/src/loop_/side.rs:48`.
`VoiceEvent`: `tui/src/model.rs:5790` (`apply_voice`). `DictationText` constructors: `voice/src/dictation.rs:525`, `tui/src/model.rs` ×4 (tests).
`DictationState` constructor: `voice/src/dictation.rs:466`. `VoiceStatus` constructor: `voice/src/lib.rs:1078`.

## Design said vs built (deviations from frozen shapes — none to §14.1/§14.2 field names; choices where the design left a hole)
- `Match` fields are not in §5.3: built `{ entry: EntryId, canonical: String, score: f32, tokens: u8 }`.
- `EntryId` is a `String` newtype (packaged entries need stable ids without uuids); `RuleId` likewise. `SuggestionId` is `Uuid` v7.
- `Scope` is `#[serde(tag = "scope")]` with named fields (`Surface { surface }`, `App { app }`) so ts-rs/Swift/C# mirrors are plain objects; §5.3 wrote tuple variants.
- `Correction` has no `dictation_id` (the brief's `learn::candidates(heard, typed)` has none to give); `{ utterance, heard, typed, at_ms }`.
- `Store` methods are `async` (sqlx); `open(data_dir)` only — `HistoryConfig` is passed by A3 when it opens the pool.
- `Dictionary::delete(&EntryId)` / `list(Option<&Scope>)` take references; `put` takes `Entry` by value and returns the stored copy.
- `Agents::polish` is sync (stub); A5 makes it async when the side call lands. `Rewrite` is a separate message (the design listed only `Polish`).
- `HistoryRow.has_audio: bool` instead of `audio_path`; the path never leaves the engine.
- `ShapePresets::get(surface)` and `Dictionary::{packaged_version, counts}` are additions for A4's `voice_status.dictionary` line.

- `CallKind.ts` (§14.5) does not exist today — `CallKind` is not in the export list; unchanged. Bindings dirs (`/bindings`, `crates/desktop/ui/src/rpc/bindings/`) are gitignored, so "commit the bindings" is a no-op: regenerated + copied, 838 files, `methods.json` carries the 17 new names.

## Gates (all green, first run)
- `build-lock mbx check -p voice -p rpc -p agent -p runtime` → `Finished dev profile in 4m 33s`, 0 warnings, 0 errors.
- `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings` → `test result: ok. 8 passed; 0 failed`; `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/` done.
- `build-lock mbx test -p voice --no-run` → `Finished test profile in 1m 46s`.
- `MBX_SCHEDULER_TESTS=0 mbx test -p voice` bare → `test result: ok. 27 passed; 0 failed; 0 ignored`.
- No new tests (stubs only). `cargo fmt -p voice|rpc|agent|runtime|tui` before each commit.

## Hand-offs
- A1: `shape/mod.rs` `Shaper::shape` + `end.rs`; `Shaped`/`Replaced` are not ts-exported (engine-internal); `EndCommand`/`Action` are.
- A2: `dictionary/mod.rs` bodies + `matcher.rs`; `Dictionary` is `Default` and in-memory; swap in the §5.5 storage behind the same methods.
- A3: `history/mod.rs` `Store.db: Option<SqlitePool>`; add `migrations/`, make `open` build the pool. `Correction` fields are yours to extend (not ts-exported).
- A4: `dictation.rs:466` (`input_device`, `last`), `:528` (shaper output); `api.rs` region "153 dictation history / dictionary"; `lib.rs` may gain `pub mod`/re-exports only.
- A5: `side.rs` `ModelRole::Polish` chain; `supervisor.rs` `Agents::polish` (+ `Rewrite` handler) and `POLISH_SYSTEM`; `api.rs` region "153 polish / rewrite" already routes through `ask`.
- Integrator: `crates/tui/src/{lib,model}.rs` touched (additive adapt only); `crates/voice/src/dictation.rs` touched for the two frozen §14.1 hunks.
