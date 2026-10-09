# 153 as-built — B2 (TUI: shaped splice, send it / scratch that, suggestion line, `/voice`)

Worktree `~/Develop/theuth-153-b2`, branch `feat/153-b2`, rebased onto `integrate/153` `b7ec00b9` (the brief's base `e464a304`
had a non-compiling `voice`; 153-Z's three commits fixed it and touch none of B2's files). Design: `153-voice-anywhere.md` §3.1 (TUI
column), §4.4, §6.4, §7.4, §8.1/8.7, §14.5–14.6, §16 row 13; engine contract as A4 built it (`153-as-built-A4.md` hand-offs).

## Files
- `crates/tui/src/dictation.rs` — `shown(text, shaped, has_command)` (the region text per piece), `Dictating::text_event`, `send`
  + `mic` fields, `Suggestion { id, heard, typed, occurrences, shown_ms }` + `SUGGESTION_MS = 20 s` + `line()`/`expired()`.
- `crates/tui/src/model.rs` (dictation/voice parts) — `Command::{DictationStart { pressed }, DictationCorrection, SuggestionAnswer,
  VoiceStatus, SetInputDevices, SetShape}`; fields `pending_send`, `dictated`, `suggestion`; `apply_voice` folds `shaped`/`command`,
  `DictationDevice`, `DictionarySuggestion { surface: Tui }`; `stop_dictation(send)`, `answer_suggestion`, `suggestion_wake`,
  `expire_suggestion`; `send_text` emits the correction; `/voice` row in `COMMANDS`; `voice_cmd(arg)`, `voice_line(status, dictation,
  devices)`.
- `crates/tui/src/ui.rs` — the loop drains `pending_send` through `m.submit` (the send key's path) and expires the suggestion on its
  wake; keys: ⏎ during the settle = send now, a typed char = keep editing, `y`/`n`/`N`/`p`/Esc on the suggestion line; header shows
  "sending…", the mic note; the one-line suggestion under the composer; an emptied editor clears `dictated`.
- `crates/tui/src/lib.rs` (shared file, dictation arms only) — `dictation_start(None, Some(DictationOpts { surface: Tui, destination:
  AgentPrompt, commands: COMMANDS, press_ago_ms, app: None }))`; the five new command arms (`UiEvent::Info` replies, no new events).

## Design vs built
| Design | Built |
|---|---|
| §4.4 field shows `shaped`; brief: fallback `text` | `shown`: `shaped`, or `text` when `shaped` is empty **and** no end command rides the piece — "send it" / "scratch that" alone shape to `""` on purpose, and the scratched utterance's empty re-send has none either. |
| §7.4 "send it" → splice then the send key's path | `Finished` with `send` on the latest piece → `pending_send = splice`; the UI loop calls `m.submit` (steer mid-turn, send otherwise) and clears the editor. An empty splice lands as text instead. `take_if` on the id makes it once. |
| §3.1 settling: "⎋ keep editing · ⏎ send now"; any key → keep editing | ⏎ → `stop_dictation(true)` (keeps the submit); a typed char → `stop_dictation(false)` (drops it, text lands); Ctrl+D likewise drops it; Esc discards as before. The TUI cannot un-settle without stopping (§7.3: surfaces call `dictation_stop`). |
| §7.4 "scratch that" | An earlier utterance's `final` with empty text empties its slot (`set_utt` + `body` skips empties); no TUI logic beyond `shown`. |
| §8.2 `DictationDevice` → level slot | Header note after the clock: "mic lost — reconnecting…" / "mic → ‹name›" (ochre). |
| §6.1 `dictation_correction` at send | `dictated = Some(id)` at a non-empty `Finished`; `send_text` sends `DictationCorrection { id, text: line }` first (fire-and-forget: a failure is a `tracing::debug!`); cleared when the editor empties. |
| §6.4 card `a`/`n`/`v`/`p`; brief `y`/`n`/`N` | Brief's keys: `y` add · `n` not now · `N` never · Esc = not now · `p` prints "play what you said: in the desktop's suggestion card". Only `surface: Tui` suggestions show. Auto-dismiss 20 s = drop locally, no RPC (no cooldown, as §6.4). The line takes the editor's focus while up. |
| §8.1/8.7 `/voice` status: devices, preset per surface, last timings | `voice_status.input_devices` (preference, "›"-joined), `list_input_devices` rows (● preferred / ○, "(system default)"), `dictation_status.input_device` + `.last` as "last dictation: Tui · mic 42 ms · first words 310 ms · final 890 ms · stop → final 420 ms · ‹device›". **Preset per surface not shown**: no RPC reads `voice.shape` (`set_shape` is write-only); `/voice shape <full|plain|off>` sets the TUI's. |
| §8.1 `/voice mic <name>` → `set_input_devices` | `set_input_devices([name])`; `/voice mic default` → `[]`. Reply "mic → ‹name›". |
| §14.1 `commands` | The `COMMANDS` table's names (`/model`, `/voice`, …). |
| `Action`s ("new line", "tab") | Not applied separately: A1's `shaped` already carries the newline (row 21); `actions` ignored. |
| A1 hand-off: skip `trail()` after `/cmd` / `@name` | Nothing to do: `trail` is `""` when `after` is empty; otherwise the space before `after` is correct. |

## Tests (`crates/tui`)
`dictation::tests::splice_over_shaped_table` (10 rows: shaped wins, no-shaped fallback, partial→final re-shape, "send it" alone,
phrase removed + armed, scratch empties earlier utterance, continue after scratch, scratch the only utterance, new utterance disarms,
newline verbatim); `model::tests::{send_it_submits_once, suggestion_keys, dictation_device_note, voice_cmd_parses}`;
`dictation_lifecycle` adapted to `DictationStart { .. }`. No model, no mic: fed events.

## Gates (`BUILD_LABEL=153-B2 build-lock mbx …`, CARGO_BUILD_JOBS=4)
- `mbx check -p tui --tests` → `Finished dev profile in 5m 08s`, exit 0, 0 warnings.
- `mbx clippy -p tui --tests` → `Finished dev profile in 12.80s` (after two test-only fixes: `type Row`, `const SEND`), exit 0, 0 warnings.
- `mbx test -p tui --no-run`, then bare `MBX_SCHEDULER_TESTS=0 mbx test -p tui` → `Finished test profile in 13m 02s`; lib `test result: ok. 166 passed; 0 failed` (+ 1 integration `ok`, 0 doc-tests), no flake.

## Hand-offs
- Integrator: `crates/tui/src/lib.rs` touched (the `DictationStart` arm + five new command arms; no new `UiEvent`, so the headless
  printer is untouched). Branch is on `integrate/153` `b7ec00b9`.
- A4/engine: the TUI treats `DictationText.command == Send` on the **latest** piece as armed; a later piece without it disarms (the
  actor resets `armed` per utterance — matches). `DictionarySuggestion.surface` must be `Tui` for the TUI to show it.
- Desktop (B1): `p`lay is desktop-only by design; the TUI says so.
