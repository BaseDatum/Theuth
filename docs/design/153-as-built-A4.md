# 153 as-built — A4 (actor + mic + RPC bodies)

Worktree `~/Develop/theuth-153-a4`, branch `feat/153-a4`, cut from the PIN `cbaf4259`. Design: `153-voice-anywhere.md` §4.4, §4.6,
§5.6, §6.1/6.3, §7.3–7.4, §8.1–8.4/8.7, §14, §16 rows 5–8, 10–11.

## Files
- `crates/voice/src/dictation.rs` — rewritten around the PIN's actor: `StartDictation { source, opts }`, the shaper per piece, sticky
  end commands, "scratch that", the §7.3 settle, §8.2 re-arm, §8.3 readiness + silent-device gate, §8.4 standby, §8.7 timings,
  history rows + per-utterance audio, the dictionary snapshot, new messages `Correction`, `AnswerSuggestion`, `DictionaryOp`/`DictionaryReply`,
  `Setting`, `InputDevicePrefs`, `HistoryStore`.
- `crates/voice/src/mic.rs` — `list_input_devices()`, `Capture::open_on(prefs)`, named open through `StreamConfig.device_id`, test-only `Capture::fake()`.
- `crates/voice/src/lib.rs` — `pub use` lines only (new messages, `SETTLE`/`SILENCE`).
- `crates/rpc/src/api.rs` — bodies of `dictation_start/correction/history/history_forget/audio_get`, `dictionary_list/put/delete/suggestion_answer`,
  `list_input_devices`, `set_input_devices`, `set_shape`, `set_end_phrases`, `voice_status`; helpers `Server::write_voice_config`, `history_store(ext)`.
- `crates/desktop/ui/src/rpc/bindings/*` — the re-export (see gates).

## Design vs built
| Design | Built |
|---|---|
| §4.6 `Task.shaper: ShaperOwned`, `prev_text` = finals joined | Same. `prev` = shaped finals joined with a space. §7.1 state: one `EndParser` per utterance owned by the task, fed through `Shaper::shape_with`, reset on a new utterance. |
| §4.6 "scratch that" re-sends the previous closed utterance empty | Same: `DictationText { utterance: prev, text: "", shaped: "", final: true }`; the scratch utterance itself is not kept as a final. |
| §7.3 `SETTLE` 1.5 s then `SILENCE` 0.35 s | `SETTLE`/`SILENCE` = `Duration`s of A1's `end::SETTLE_MS/SILENCE_MS`. Settle counts from the last piece carrying `Send`; silence = the utterer's gate closed (`is_open() == false`) for 0.35 s; new speech (gate open, or a piece without `Send`) resets. `Finished { Done }`. |
| §8.1 `open_on(prefs)` walks the list, then default, then first input | Same; `flexaudio::devices()` (cpal names, `is_default`) enumerates and `StreamConfig.device_id = Some(name)` opens by name — no deviation needed. |
| §8.2 retry 300 ms × 3 / 5 s | Brief's numbers: every 500 ms for 5 s. `DictationDevice { Lost }` then `{ Changed }`, the open utterance is closed first, no `Finished`; after 5 s `Error("microphone: device lost")`. Remote sources untouched. |
| §8.3 `Started` after the first PCM; 2 s ready gate; 3 silent 160 ms windows | Mic only (a remote feed is the client's). The level window (8 × 20 ms) doubles as the detector window; after one non-silent window the detector is off. Rejection → next device once (the failed name dropped from the list), else `DictationDevice { Lost }` + `Error("microphone: no sound from ‹name›")` / `"no audio from ‹name›"`. |
| §8.4 standby 8 s, prewarm primes the resampler | Standby built (`voice.standby_secs`, `0` = today; buffered chunks are drained at reuse; a stale device error drops it; a `Lease` or a device-list change drops it). Resampler priming skipped (one FFT block, sub-ms). |
| §8.6 duck/pause | Not touched: companion-side per design; the setting is read into config by C. |
| §8.7 timings | `press = recv − press_ago_ms`; `first_pcm`, `first_partial`, `first_final` on the task's clock; `stop_to_final_ms` from the stop observation (cancel/deadline/settle) to the `Finished` publish. In `DictationState.last` and the `dictations` row (`press_ms` column = press→first PCM). |
| §5.6 `voice_status.dictionary { … }` | `VoiceStatus` (frozen, C's) has no such field; not added. `dictionary_list` reports user + learned as designed. |
| §6.3 suggestions | Post-merge (integrate/153 `bebe75e5`): `Store::correction` returns `Vec<Suggestion>` with id/occurrences/surface/has_audio → published 1:1 as `DictionarySuggestion`, skipped when `!history.learn`; `Store::answer(id, answer, &mut dictionary)` does Add/Never/NotNow and the actor re-snapshots after Add/Never. |
| §6.5 store setup | `Store::open(..).with_budget(rows, audio_mib << 20)`, gated on `Store::recording_allowed(&history, false)` — no private-session signal exists in the frozen `DictationOpts`, so `false`. Audio: `attach_audio` once per closed utterance (the task batches a 16 kHz i16 utterance incl. a 500 ms pre-roll), rows at `Finished`. |
| §14.1 message named `SuggestionAnswer` | Named `AnswerSuggestion` (the enum `history::SuggestionAnswer` already has the name). |
| §14.1 `DictationOpts.commands` | Applied via A1's `Shaper::with_commands` + `with_surface` (post-merge). Per-utterance `EndParser` through `shape_with`. |
| A1's `with_mentions(agent names)` | Not wired: the voice crate has no agent registry and the frozen `DictationOpts` carries no mention list; the dictionary's `mention`-tagged entries still apply. Integrator/wave B: add a `mentions` field or have the RPC layer resolve `agents_list` if wanted. |

## Tests (`crates/voice/src/dictation.rs`, fake engine + injected mic opener; no model, no device)
`none_opts_is_today`, `send_it_settle_then_silence_stops`, `scratch_that_empties_previous_utterance`, `device_lost_rearms_within_5s`,
`silent_device_rejected_in_480ms`, `readiness_gate_2s`, `timings_recorded_with_synthetic_feed`; the four PIN tests unchanged.
The end-command tests run through A1's real `EndParser` (post-merge; the PIN-era test shim is gone).
Post-merge full run: `live::tests::utterer_returns_the_spoken_sentence_punctuated` (a pre-existing model-loading test in `live.rs`, not
`#[ignore]`d, untouched by A4) failed once on the model dropping the trailing `.`; it passed in the pre-merge run — flaky model output, not A4's.

## Gates (all through `BUILD_LABEL=153-A4 build-lock mbx …`, CARGO_BUILD_JOBS=4)
- `mbx check -p voice -p rpc --tests` → exit 0, 0 warnings.
- `mbx test -p voice -p rpc --no-run`, then bare `MBX_SCHEDULER_TESTS=0 mbx test -p voice -p rpc` →
  voice `test result: ok. 34 passed; 0 failed` (11 `dictation::*`), rpc `test result: ok. 56 passed; 0 failed` (+ 1 doc-test crate `ok`).
- `mbx clippy -p voice -p rpc --tests` → `Finished dev … in 1m 12s`, 0 warnings.
- Bindings: `BUILD_SLOTS=1 … mbx test -p rpc --features ts --lib --quiet export_bindings` → `8 passed`; `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/`
  → empty diff (every §14.5 type was exported at the PIN; A4 adds no `ts(export)` type).
- Not run: a model-loading test, the live probe (§16 row 11 — commands below).

## Probe (§16 row 11) — for the integrator
No RPC harness calls `dictation_start` in `crates/rpc/tests` or `crates/app`. Manual: `t --headless`, then over the WS RPC
`dictation_start [16000, {"surface":"tui","press_ago_ms":0}]` → push 20 ms chunks of a tone via `dictation_audio [id, <b64 i16>]` →
`subscribe_events` shows `DictationText` with `shaped` (== `text` until A1), `DictationFinished`, then `dictation_status.last` with the four
numbers; `dictation_start [16000, null]` → identical event shape.

## Hand-offs (wave B surfaces)
- Send `opts` with `surface`, `destination`, `press_ago_ms`; read `shaped`, act on `command: send` after `DictationFinished`; on a
  `DictationText { final: true, text: "" }` for an earlier utterance, empty that splice (scratch).
- Show `DictationDevice { lost | changed, device }` in the level slot; `dictation_status.input_device` / `.last` for the debug row.
- `dictation_correction(id, sent_text)` at send; answer `DictionarySuggestion` with `dictionary_suggestion_answer`.
- Settings: `list_input_devices` / `set_input_devices` (`voice_status.input_devices` is live), `set_shape`, `set_end_phrases`.
