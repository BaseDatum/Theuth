# 153 as-built — B1 (desktop UI: Composer + phone webview + Settings › Voice)

Worktree `~/Develop/theuth-153-b1`, branch `feat/153-b1`, cut from `integrate/153` `e464a304`. Design: `153-voice-anywhere.md`
§3.1, §4.4–4.5, §6.1/6.4, §7.4, §8.1/8.7, §9.1–9.2, §14.5, §16 rows 12/14/15; engine as-builts A2–A5.

## Files
- `crates/desktop/ui/src/lib/dictation.ts` — `Utt { text, shaped, final, command? }`, `shown(u)`, `Active.device`, `applyEvent` stores
  `shaped`/`command`, handles `DictationDevice`; `splice` renders `shown`; `finish(a, discard) → Finish { value, caret, id, send }`;
  `startOpts(commands, pressAgoMs)` + `start(…, commands)` passes `opts`; `correction(api, id, sentText, by)`; the
  `DictionarySuggestion` store (`useSuggestion`, `clearSuggestion`) with the reporting pane and the dictation id attached.
- `crates/desktop/ui/src/rpc/api.ts` — `dictation_start(rate, opts)`, the 17 new methods (`dictation_correction/history/history_forget/
  audio_get`, `dictionary_*`, `list_input_devices`, `set_input_devices`, `set_shape`, `set_end_phrases`, `polish_model`,
  `set_polish_model`, `polish`, `rewrite`) + `METHODS`; `voice_status` typed by the generated `bindings/VoiceStatus` (carries `input_devices`).
- `crates/desktop/ui/src/components/chat/Composer.tsx` — `dictationCommands(harness)`, `spokenSend(auto, text)`; `onFinish` lands the
  text, remembers the id, queues the spoken send; `submit()` reports `dictation_correction` once per dictation; the mic hint under
  the voice bar; `DictatedText` renders `shown(u)`; the `DictionarySuggestion` card under the box for the pane that sent.
- `crates/desktop/ui/src/components/ui/DictionarySuggestion.tsx` — the card (frame D), `pcmSamples` / `playPcm` (base64 i16 16 kHz → WebAudio).
- `crates/desktop/ui/src/components/settings/Voice.tsx` + the `voice` row in `SettingsDialog.tsx` (between Meetings and Voice key).
- Tests: `lib/dictation.test.ts` (9), `components/chat/Composer.test.ts` (4), `components/settings/Voice.test.ts` (4).

## Design vs built
| Design / brief | Built |
|---|---|
| §4.5 `surface: isMobile ? phone : composer` | `touch()` (coarse pointer, `lib/platform`) decides: `start()` is not a hook, and the phone webview is the coarse-pointer client. |
| §14.1 `commands` | theuth's `COMMANDS` names + the harness's manifest names, each with a leading slash (`skill:x` → `/skill:x`). |
| §3.1 `shaped` shown, fallback `text` | `shown(u) = shaped || (command ? '' : text)`: the shaper returns `""` + `ScratchThat` for "scratch that", so the raw text is shown only when no command consumed it (an older engine). |
| §7.4 "send it" → `submit()` | At `DictationFinished`, `send` = a **final** utterance carries `command: send`; the Composer lands the text, then `spokenSend` fires one `submit()` when the box's text is the landed value (same path as ⏎ — a steer mid-turn). A partial that only armed never sends. |
| §7.4 "scratch that" | Nothing extra: the engine's empty final for the earlier utterance is the same write; the splice re-renders. |
| §6.1 correction at send | `submit()` with no `spoken` note → `dictation_correction(id, line)` fire-and-forget; discard (Esc) hands `id: ''` → no report; voice notes never report. |
| §6.4 card copy | "Dictionary · N times this week / You changed ‹heard› → ‹typed› after dictating. Heard the same way N times. Add it as a replacement? / Add · Not now · Never for this · ▶ play what you said". Auto-dismiss 20 s = local clear only (A3 counts the ignore engine-side). |
| §6.4 play `dictation_audio_get(id, utterance)` | The event has no dictation id; the store attaches the id of the last `correction` report → `dictation_audio_get(thatId, null)` (A3: whole dictation). |
| Brief: `Shaped.replaced` underline in `components/ui/dictation.tsx` | **Not built**: `replaced` is not on the wire (`DictationText` carries `shaped/command/actions` only; C: `Shaped`/`Replaced` not ts-exported). `dictation.tsx` unchanged — its fields already show `shaped` through `splice`. |
| §8.2 device hint | `dict.active.device` → "mic changed → ‹name›" / "mic lost — reconnecting…" under the voice bar. |
| §8.1 reorderable device list | A `Select`: picked name → `set_input_devices([name])`, "System default" → `[]`; a preferred name the host no longer lists shows as "(not connected)". |
| §4.5 / §7.2 getters | None in the frozen contract: the shape preset per surface and the end phrases are written through `set_shape` / `set_end_phrases` and the last written value is remembered in `localStorage` (`ponytail:` comment). Hand-off below. |
| §8.7 "Copy last 20 as CSV" | Not built (not in the brief); the debug row prints the five numbers + device from `dictation_status().last`. |
| §9.2 polish picker | `ModelCombobox`, none = "Chain (title model, then session)"; help text says agent prompts are never polished. |
| Brief `Voice.test.tsx` | `Voice.test.ts`: vitest's include is `src/**/*.test.ts`; static-markup render as `Memory.test.ts`. |
| Bindings | `integrate/153` `voice` does not compile (`dictation.rs:943` `Vec<Suggestion>` vs `Vec<Correction>`, `:988` `answer` missing `&mut Dictionary` — the A3/A4 seam), so the export failed; the bindings were copied from `~/Develop/theuth-153-a4/bindings` (19:01), whose ts-exported sources (`voice/lib.rs`, `shape/mod.rs`, `history/mod.rs`, `dictionary/mod.rs`, `rpc/export.rs`, `agent/lib.rs`, `api.rs`) are byte-identical to this tree's. `methods.json` parity test green. |

## Tests
`bunx vitest run src/lib/dictation.test.ts src/components/chat/Composer.test.ts src/components/settings/Voice.test.ts src/rpc/api.test.ts
src/components/ui/dictation.test.ts` → `Test Files 5 passed (5) · Tests 20 passed (20)`.

## Gates (through `BUILD_LABEL=153-B1 build-lock …`)
- tsc baseline (after `bun install` in `crates/desktop/ui` and `packages/apps`): 3 errors, all `lib/dictation.test.ts` (events missing the
  new fields). After: `bunx tsc --noEmit -p crates/desktop/ui` → exit 0, **0 errors** (0 new).
- `bun run build` → see the DONE message.
- Not run: a live Composer (§16 rows 14/15) — no engine with the 153 voice actor builds on this branch tonight.

## Hand-offs
- Windows / companion sessions — the exact `opts` the UI sends: `dictation_start [rate|null, {"surface":"composer"|"phone",
  "destination":{"kind":"agent_prompt"},"app":null,"commands":["/model","/title",…,"/skill:<name>",…],"press_ago_ms":<u32>}]`;
  at send `dictation_correction [id, <box text>]`; the card answers `dictionary_suggestion_answer [id, "add"|"not_now"|"never"]`.
- Integrator: `crates/desktop/ui/src/rpc/extra.ts:244-250` hand-written `VoiceStatus` is now unused by `api.ts` — delete it (not my file).
- Engine (§18 amendment): a getter for `voice.shape.*` and `voice.end_commands` (e.g. on `voice_status`) so Settings › Voice reads instead of remembering.
- Engine: `DictionarySuggestion` could carry the dictation id; the UI then plays without the reporter lookup.
- B2 / `VoicePanel.tsx` (voice key, not mine): `Words` still renders `u.text`; use `dictation.shown(u)` for the shaped form.
