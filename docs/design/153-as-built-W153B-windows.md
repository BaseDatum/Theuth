# 153 as built — W153B (Windows twin of wave B + the shared C# RPC mirrors)

Worktree `~/Develop/theuth-153w-b`, branch `feat/153w-b` (off `basedatum/integrate/153w` = main 6aadda9a; PIN-C merged at 42dceeba).
Design: `docs/design/153-voice-anywhere.md` §3.1, §4.4, §6.1–6.4, §6.7, §7.2–7.4, §8.1, §8.5, §8.7, §9.1–9.2, §14, §16.1 row 13, §16.2.
Hand-offs applied: H-INFO (wave A's Rust types on `basedatum/integrate/153` b7ec00b9), H3-CC (Swift mirror names), H-INFO fixtures
(13 `153-*.json` from `feat/153-cc` 557eb083), H3-B1 (desktop), H3-B2 (TUI), H3-B3 (Mac companion fields), PIN-C 1e9f40b9 (merged, no API used).

## The PIN (step 1)
- `PIN-B 4fcbcf1e` (19:30, 15 min after start): `Theuth.Companion.Rpc/Models.153.cs` + `RpcClient.153.cs` + two lines in `Models.cs`
  (the two new events typed in `WireEvent`) + the `DictationEvent` optionals (`shaped`, `command`, `actions`). No W153C hook line was needed:
  the two new events are typed in `WireEvent` and routed from `StoreModel.EventReceived` (the raw hook) in W153B's own files.
- 571ea44c aligned the names with the Swift mirrors (H3-CC) and added the cross-platform fixtures — types only, the wire never changed.

| Shape (§14.1/§14.4) | C# (`Theuth.Companion.Rpc`) | Wire notes |
|---|---|---|
| `Surface`, `ShapePreset` (Rust `Preset`), `SuggestionAnswer`, `DeviceState` | enums + `WireWords153.Word()` | snake_case words; received enum fields go through `TolerantWordConverter<T>` (unknown word → null, never throws) |
| `Action` | `ShapeAction` | `Action` would shadow `System.Action` in every file; `DictationText.actions` stays raw words (`ValueList<string>`), `WireWords153.ParseAction` types them |
| `Destination` (tag `kind`) | record `{Kind, App?}` + `AgentPrompt`/`Document`/`MessageReply`/`AnyApp(app)` | `{"kind":"agent_prompt"}` … |
| `DictationOpts` (`#[serde(default)]`) | record, null fields omitted | writes `"commands":[]` always (serde reads the same as the Mac's explicit nulls) |
| `AppProfile` | record, every required field always written | `commands` never null |
| `DictationTimings`, `DictationState` (+`input_device`, `last`), `VoiceStatus` (+`input_devices`), `InputDevice` | records | `ModelState` → `VoiceModelState {State, Message}` |
| `EndCommand` (tag `command`) | record `{Command, NearMiss}` + `IsSend`/`IsScratchThat` | an unknown command word decodes as itself |
| `EndPhrases` | record with the engine's defaults | |
| `Entry`, `Scope` (tag `scope`, named fields), `Variant` | records; `Entry.Kind`/`Source` are the wire words (`EntryKind`/`EntrySource` string constants) | unknown words round-trip through `dictionary_list` → edit → `dictionary_put`; `Scope.Kind` is the tag (a member cannot share its type's name) |
| `HistoryRow`, `PolishReply`, `ModelInfo` | records | `ModelInfo` = id/name/provider (as Swift) |
| `VoiceEvent::DictationText` + `shaped/command/actions` | `DictationEvent` gains `Shaped`, `Command`, `Actions` (+ `Device`/`State`, as Swift's `DictationWire`) and `DisplayText` | render rule below |
| `VoiceEvent::DictationDevice`, `::DictionarySuggestion` | `DictationDevice`, `DictionarySuggestion` (in `WireEvent`) | every field optional |

`RpcClient.153.cs`, positional in `api.rs` order: `DictationStartAsync(rate, opts)` (null opts → the one-element `[rate]` an older engine takes),
`DictationStatusAsync`, `VoiceStatusAsync`, `DictationCorrectionAsync(id, sentText)`, `DictationHistoryAsync(limit)`,
`DictationHistoryForgetAsync(id?)`, `DictationAudioGetAsync(id, utterance?)`, `DictionaryListAsync(scope?)`, `DictionaryPutAsync(entry)`,
`DictionaryDeleteAsync(id)`, `DictionarySuggestionAnswerAsync(id, answer)`, `ListInputDevicesAsync`, `SetInputDevicesAsync(names)` (one
positional list), `SetShapeAsync(surface, preset)`, `SetEndPhrasesAsync(phrases)`, `PolishModelAsync`, `SetPolishModelAsync(model?)`,
`PolishAsync(text, destination, session?)`, `RewriteAsync(instruction, selection, app, thread?)` (pairs as two-element arrays).

## Wave B behaviour (step 2) — files
| File | What |
|---|---|
| `Core/AskVoice.cs` | `AskVoiceMachine.OpenMicOnKeyDown` → `OpenMic` at key-down (§8.5); `Dictated(text, SendIt)` → `SendNow` (no hold, §7.4), accepted in `Recording` too (the engine ends the dictation on the "send it" settle while the key is held, §7.3); `AskDictation` keeps the display text, `SendIt` (the latest piece's command) and `LastId` |
| `Core/AskBarModel.cs` | `IDictationCalls` gains default `StartAsync(opts)` / `CorrectionAsync`; opts `{surface: companion, destination: agent_prompt, press_ago_ms}`; the early mic buffers until `dictation_start`, a tap drops it; `SendNow` → `store.SubmitAskNow`; "sending in 1.5 s…" while armed; `dictation_correction` at the commit (`Committing`), never for an undone hold or a cleared field; `DictationDevice` hints "mic → ‹name›" / "mic lost — reconnecting…"; `ProbeFixture` (recorded events through the real reducer) |
| `Core/StoreModel.153B.cs` (new) | `Committing` event, `SubmitAskNow`, `SendInputNow`, `SubmitReplyNow`, `SentText`/`SentSession` |
| `Core/DictationSettings.cs` (new) | companion-local `micOnKeyDown` (default on), last-set `shapePreset.<surface>` / `endPhrases` (labels only), phrase parsing, the "Last dictation" line, `SuggestionCopy` (frame D's words), `MicPreference` (§8.1 order → endpoint) |
| `Core/StoreModel.cs` (unowned, **2 lines**) | :749 the dictation decoder passes `DisplayText` + the send flag; `Commit` raises `Committing` |
| `Rpc/Models.cs` | `DictationEvent` optionals + `DisplayText`; `WireEvent` types the two new events |
| `App/AskBar/DictationCalls.cs` | opts with a `-32602` fallback to the 149 call; correction with a `-32601` no-op |
| `App/AskBar/AskBarInstall.cs` | `micOnKeyDown` from Settings; installs the suggestion card; refreshes `MicPreference` from `voice_status` at every connect |
| `App/AskBar/AskBarWindow.cs` | `CapturePng` (probe evidence) |
| `App/AskBar/AskFixtures.cs` (new), `AskFlags.cs`, `AskProbe.cs` | `--ask-probe N --ask-fixture shaped|send-it|suggestion|all`, `--dictation-shot`, PNGs to `%TEMP%\153-w153b-*.png` |
| `App/Voice/RowTalk.cs` | opts, `SendIt`, correction at the commit of the row's send, the recorder closes when the engine ends the dictation |
| `App/Voice/VoiceRecorder.cs` (unowned, `Preferred()` + 1 call) | opens the first connected endpoint named in `voice.input_devices`, else the default (§8.1 "one choice serves both paths") |
| `App/Windows/AgentsPanelWindow.cs`, `AttentionPanelWindow.cs` (unowned, **1 line each**) | a spoken "send it" skips the hold (`SendInputNow` / `SubmitReplyNow`) |
| `App/Windows/SuggestionCardWindow.cs` (new) | frame D: ToolWindow + card chrome (Mica Base, round corners), under the Ask bar when it is up; Add · Not now · Never for this → `dictionary_suggestion_answer`; ▶ `dictation_audio_get(id, null)` played at 16 kHz when `has_audio`; 20 s auto-dismiss sends nothing |
| `App/Settings/SettingsWindow.153.cs` (new), `SettingsWindow.cs` (page registration + reload on connect), `SettingsModel.cs` | Settings › Dictation: microphone, "Open the mic on key-down", shaping per surface, spoken commands, polish model (+ link), dictionary list/add/delete, history (50 rows, ▶, Forget, Forget all), privacy text, "Last dictation" line; `partial void AnyAppSettings(StackPanel)` = W153D's seam |
| tests | `Rpc.Tests/{Models153Tests, RpcClient153Tests, Fixture153Tests}.cs` + `Fixtures153/` (13 files); `Core.Tests/{AskVoiceTests.153.cs, DictationSettingsTests.cs}`; `AskTests.cs` gains only `partial` on `AskVoiceTests` (the 149 cases unmodified) |

Render rule (H3-B1/B2): shown = `shaped` when non-empty; else "" when the piece carries an end command ("send it" / "scratch that" alone shape
to ""); else the raw `text` (an engine before 153 sends no `shaped`). Send: the latest piece's `command: send` arms; a later piece without it
disarms; the field acts once, on `DictationFinished`.

## H-BC alignment (153-as-built-BC.md deviations rows and §Hand-offs › Windows twin)
- §4.4 render rule — followed the UI/TUI form `shaped || (command ? "" : text)` (H3-B1/B2); the Swift companion's `shaped ?? text` differs only
  for an empty `shaped` without a command.
- §3.1 settling copy — followed: "sending…" while "send it" is armed (no countdown source on the surface).
- §6.1 correction at send — Ask bar as the Mac; the row mics (agents panel, card V) also report at their commit (the Mac leaves its
  `HoldToTalk` callers to wave D) — an addition, same RPC and the same "commit only" rule.
- §8.7 Settings — followed: shape presets and end phrases write-only, last value remembered locally; the learn flag is a read-only pointer.
- §Hand-offs › Windows twin constants — `holdThreshold 250 ms`, `maxDuration 60 s`, suggestion auto-dismiss 20 s and the exact
  `dictation_start` params `[16000, {"surface":"companion","destination":{"kind":"agent_prompt"},"commands":[],"press_ago_ms":N}]` all match
  (pinned by `AskVoiceTests` / `RpcClient153Tests`); the 13 fixtures are decoded by `Fixture153Tests`.
- §10.6 (approval listen), the command card and the `powershell` tool are W153C's; the AnyApp hand-offs are W153D's.

## Verification (step 3)
| Gate | Result |
|---|---|
| Mac Rpc tests (`dotnet test tests/Theuth.Companion.Rpc.Tests --blame-hang`) | **111/111** (68 at the start + `Models153Tests` 20, `RpcClient153Tests` 3, `Fixture153Tests` 6, and `FixtureTests.EveryFixtureDecodes` over the 13 shared `153-*.json` fixtures + 155's `attention_item_155_alert` that main brought) |
| Mac Core tests (`dotnet test tests/Theuth.Companion.Core.Tests`) | **387/387** (310 at the start; + W153C's merged tests; W153B adds `AskVoiceTests` wave-B cases incl. **`SendItSkipsTheHold`**, the early-mic tap race, the speech press, and `DictationSettingsTests`); the 149 `AskVoiceMachine` cases unmodified |
| build-B Debug x64 (`Theuth.Companion.slnx`, `Platform=x64`) | PIN 4fcbcf1e **0 err / 0 warn** (52 s); merged head c3a1f896 (main f4834cf3 + PIN-C) **0 err / 0 warn** (35 s); every intermediate head 0/0 (`build-*.json`) |
| TS bindings re-check (H-A) | main 5afea171's voice/agent/rpc types are unchanged since b7ec00b9 except behaviour (`"dictionary loading"` → `DictionaryRetry`); the generated bindings (`~/Develop/theuth-153-int/crates/desktop/ui/src/rpc/bindings`, 19:38) match every record — field names, optionality, tags (`kind`, `command`, `scope`), u64 as `ulong`, `metaphone: [string, string]` — except the two flat-dir name collisions under Open items |
| Fixture UI round 1 (19:54, 1e92fa11) | `--ask-fixture all`: shaped partial/final in the field (UIA `Ask theuth` value "Ship it tonight" during send-it), the shaped text in the 3 s hold, then `ask sent to desk (55 chars)`; send-it: status "sending in 1.5 s…", `voice: "send it" — sent without the hold (15 chars)` 19:54:48.226 → `ask sent to desk (15 chars)` 8 ms later (no hold); the suggestion card shown (Mica Base, frame D's words) and auto-dismissed after 20 s with nothing sent; Settings › Dictation against the pre-wave-A engine shows the `-32601` answers as readable lines |
| Fixture UI round 2 (20:33, 88f7eee8) | the six app-rendered PNGs below; the Run key back on the installed companion; desk released |
| Live round (H-ENGINE) | run as R1 (see the R1 section): proof 15's Windows half and proof 13 live; the engine drops the tail before `dictation_stop` |

Evidence: `~/.theuth/briefs/2026-10-09/153-windows/evidence/W153B/` — `build-*.json`, `fixture-round-{1,2}-*.txt`,
`153-w153b-{1-shaped-live,2-shaped-hold,3-sendit-armed,4-sendit-sent,5-suggestion,6-settings-dictation}.png` (+ `-on-dark` composites),
`README.md`.

## Deviations (each the closest faithful variant; none changes the wire)
1. `AskVoiceMachine.OpenMicOnKeyDown` defaults **off in the machine** so the 149 `AskVoiceMachine` tests stay unmodified (§16.2); the app
   passes Settings › Dictation "Open the mic on key-down" (default **on**, H3-B3). Swift's tap effect is `.discardMic`; here `Tap` and the
   controller drops the early buffer — same behaviour.
2. Render rule follows H3-B1/H3-B2 (empty `shaped` falls back to `text` only without a command), not H3-B3's shorthand `shaped ?? text`.
3. The suggestion card shows for `surface` = `companion` **and** `any_app`: both are dictated by the companion and no other client shows an
   any-app card (H3-B2 "the sender's surface").
4. Microphone: one choice (`set_input_devices([name])`, `[]` = system default), as the desktop (H3-B1), not §8.1's reorderable list; a longer
   list set elsewhere is shown and honoured (the recorder walks it in order).
5. Shape presets and end phrases are write-only in the contract (no getter): the page shows the last value this companion wrote (H3-B1),
   kept as companion-local labels (`shapePreset.<surface>`, `endPhrases`); the engine stays the only copy that matters.
6. Polish model: shown with a link to theuth's Settings › Voice (§9.2 "no second picker"); `set_polish_model` is wrapped but not offered.
7. History switches (`voice.history.enabled/audio_mib/learn`): no RPC writes them — read-only pointer to theuth's Settings › Voice (as the Mac).
8. Per-destination polish opt-in: the companion's own fields are `agent_prompt` (never polished — said in the row); other apps are W153D's
   `appProfiles` → rendered through the `AnyAppSettings` partial seam (no line in W153B's files changes when W153D implements it).
9. `DictationDevice` hints are wired, but the companion streams its own microphone (a remote source the engine never re-arms, A4), so they
   only fire for an engine-mic dictation.
10. Unowned files touched: `Core/StoreModel.cs` (2 lines), `AgentsPanelWindow.cs` (1), `AttentionPanelWindow.cs` (1), `VoiceRecorder.cs`
    (preferred endpoint), `AskTests.cs` (`partial`). No sibling-owned file was edited.
11. Screenshots: during both UI rounds build-B's desktop did not present WinUI content to screen capture (black windows, the taskbar clock
    frozen at 19:50 — the session was not being presented), so the PNG evidence is rendered by the app itself (RenderTargetBitmap of the
    bar, the card and the Settings page) and backed by UIA reads and the companion log.

## Open items
1. **Live round (R1, after H-ENGINE)** — ready to run in one desk round: `--ask-probe 40 --ask-live sendit --shareable` (built-in "Ship it
   tonight. Send it." through the real engine inside a voice press → expect shaped text and `sent without the hold`), `--ask-probe 60 --ask-live
   learn --shareable` (dictate, undo the hold, correct one word, send — twice → `DictionarySuggestion` → the card, PNG `8-live-suggestion`),
   `--settings dictation --shareable --dictation-shot` (devices, polish model, dictionary, history, last dictation from the 153 engine) plus a
   UIA change of the Companion preset / phrases checked in the companion log and `w6-gate.ps1 settings-json`. Proof 15 (correction loop) and
   the §8.7 timings line need the 153 engine.
2. **TS bindings name collisions (wave A, not C#):** the flat bindings dir has ONE `Kind.ts` (artifact kind) and ONE `Source.ts`
   (knowledge source); `voice::dictionary::Kind` / `Source` are overwritten, so the TS `Entry.kind` / `Entry.source` types are wrong on the
   desktop side. The C# follows the Rust source (`term|symbol|formatter|abbrev|mention|command|path`, `packaged|user|learned`).
3. **No getters** for `voice.shape.*`, `voice.end_commands.*`, `voice.history.*` (and no setter for `voice.history.*`): the page labels
   the last values written here; an engine-side `voice_settings_get` would let every client show the truth (design §14.2 amendment).
4. **W153D seam:** implement `partial void AnyAppSettings(StackPanel page)` in `SettingsWindow.153D.cs` for app profiles / clipboard / paste
   check; the per-app polish opt-in lives there.
5. **Integrator:** the unowned one-liners (`StoreModel.cs` ×2, `AgentsPanelWindow.cs`, `AttentionPanelWindow.cs`, `VoiceRecorder.cs`
   `Preferred()`, `AskTests.cs` `partial`) are additive; a merge with W153C/W153D touched none of them.
6. Multi-display: the suggestion card follows the Ask bar or the display under the pointer; verified on build-B's single display only.

## R1 — 153 F1 `voice_settings` (main bfafefeb) and the live round

**voice_settings.**
- `Models.153.cs` adds three records:
  - `ShapePresets`: preset words kept as strings; `Preset(surface)` parses one, and an unknown word gives null instead of throwing.
  - `HistoryConfig`.
  - `VoiceSettings`: `standby_secs` is a `ulong` (bigint in TS), `duck_media` stays a word, `polish_profiles` is a map.
- `RpcClient.153.cs` adds `VoiceSettingsAsync()`.
- No new shared fixture came with F1, so nothing new is registered in `FixtureTests.Shapes`.

**Settings › Dictation reads it once per open:**
- The shape pickers move to the engine's words. Showing a value is not a choice, so no `set_shape` is sent for it. A word this build does not know is named in the caption.
- The send, scratch and literal fields are replaced unless they were already edited.
- The microphone preference comes from `input_devices`; on an older engine it comes from `voice_status`.
- The polish line uses `polish_model` (an id or the chain); on an older engine it calls `polish_model()`.
- The history caption shows the real rows, MiB, learn and enabled values.

**Fallbacks:**
- When the engine answers -32601, the page keeps the values last written here. The caption says "this engine is older and cannot report its presets".
- Any other error gives "the engine did not answer".

Every open writes a log line for read-back: `settings: voice_settings ok — shape companion=… ; send=[…] … history enabled=… rows=… audio_mib=… learn=…` (or the -32601 line).

**Gates:**

| Check | Result |
|---|---|
| Rpc tests (Mac) | **113/113** (`VoiceSettingsReadsTheLiveKeysAndAnEngineBeforeF1AnswersMethodNotFound`, `VoiceSettingsDefaultsMatchTheEngine`) |
| Core tests (Mac) | **485/485** (`VoiceSettingsCaptionsFollowTheEngineAndFallBackWhenItIsOlder`) |
| build-B Debug build | 87b13d09 **0 errors, 0 warnings** |
| Fallback round on the installed pre-F1 engine (23:17) | log `settings: voice_settings -32601 (engine before 153 F1) — the remembered values stay`; PNG `153-w153b-9-settings-f1-fallback.png` shows the "this engine is older" caption |

Reading the values back from an F1 engine waits for the final package: the package installing tonight predates F1.

**Live round (R1, 23:46–23:56, desk lock W153B; build-B engine 2.14.0+f4834cf3 = 153 waves A–D + the WebGPU run lock, no F1).**
Speech came from the system voice (Microsoft David, 16 kHz) and was pushed through `dictation_start/audio/stop` inside a voice press; the microphone was not used.
Desk sends reach build-B's theuth desk, whose model does not resolve, so nothing was spent. Log: `live-round-r1-log.txt`.

| Round | Engine events | Companion |
|---|---|---|
| learn ×2, "Please restart the zorblax service." | final `Please restart the Zorblack service` both times; after the 2nd correction `DictionarySuggestion{heard:"Zorblack", typed:"Zorblax", occurrences:2, surface:"companion", has_audio:true}` | `dictation_correction(…, 34 chars) ok` at each send; the card was **shown** (PNG `153-w153b-8-live-suggestion.png`: "You changed Zorblack → Zorblax after dictating." · "2 times this week" · Add / Not now / Never for this / ▶) and auto-dismissed after 20 s with nothing sent — **proof 15's Windows half, live** |
| sendit with 2.5 s of silence before the release, "Ship it tonight, send it." | utterance 7 `Ship it tonight` (final), utterance 8 `Send it` final, `shaped:""`, `command:{"command":"send","near_miss":false}` | `voice: "send it" — sent without the hold (15 chars)` → `ask sent to desk` 21 ms later (PNG `153-w153b-7-live-sendit.png`) — **proof 13, live** |
| sendit with release on the last sample, "Ship it tonight. Send it." | final `Ship it tonight` only; "Send it" never arrives | no command, so the 3 s hold, then sent — correct for what the engine said |
| sendit, 1.2 s tail | `Ship it tonight` + utterance `Send` (no "it") | hold, then sent `Ship it tonight Send` |
| sendit-comma, 0.8 s tail | `Ship it tonight` only | hold, then sent |

**Engine finding, not a companion defect: the last words before `dictation_stop` are lost.** The engine finalizes the last utterance only when about 1.5–2.5 s of audio follow the last word. Released at once, "Send it" is gone; with 1.2 s it comes back as "Send"; with 2.5 s it is recognized with the command. A person who releases the key right after "send it" will mostly get the hold instead of the immediate send, on every surface. The engine should flush its pending audio and right context on stop. The learn rounds kept their last word ("service"), so the cut looks like a short tail plus the system voice's short final pause. The new probe flags are `--ask-live-tail MS` and `--ask-live sendit-comma`.

**Settings on this engine:** `voice_settings` answers -32601 (no F1), so the page shows the fallback, the same as `153-w153b-9-settings-f1-fallback.png` from 23:17. Reading the values back from F1 waits for the final package.

**Restored** after the round: the installed companion is running (`--background`), the Run key is present, `companion.json` has `launch_at_login: true`, and the desk is released at 23:56:42.

## R2 — the 153-F2 tail-drop fix on the installed engine, and the F1 readback on the installed companion

**Setup:**
- build-B installed engine 2.14.0+f1a7809c: 153-F2 (main 1480afdd) plus F1, the Windows twin and the quit fix.
- Desk lock W153B 02:13:58–02:21:10; the probe was the Debug companion at a5d9f036 (main plus v2.15.0).
- Synthesized speech went through `dictation_start/audio/stop` inside a voice press, with **0 s of silence before the release** on every row.
- Log: `live-round-r2-log.txt`.

| Row | Phrase | Engine finals | Command | Companion | `stop_to_final_ms` (engine) · stop → Finished (companion) |
|---|---|---|---|---|---|
| A | "Ship it tonight. Send it." | `Ship it tonight` + `Send it` (shaped "") | send | `sent without the hold` → desk 27 ms later | not captured · 4.59 s |
| B | "Please restart the zorblax service." ×2, corrected to Zorblax | `Please restart the Zorblack service` ×2 | — | correction sent at each send; card shown (occurrences 3), auto-dismissed | 1605 / 1586 ms · 2.09 / 2.51 s |
| C | "Ship it tonight. Send it." | `Ship it tonight` + `Send it` | send | `sent without the hold` | 3246 ms · 3.48 s |
| D | "Ship it tonight, send it." | `Ship it tonight` + `Send it` | send | `sent without the hold` | 2438 ms · 3.11 s |
| E | "Ship it tonight, send it." | `Ship it tonight` + `Send it` | send | `sent without the hold` | not captured · 3.40 s |

**Where the timings come from:**
- Engine numbers are `dictation_status().last`, which the probe reads 3–4 s after the release.
- For rows C–E that read came a few hundred ms before the dictation's own `DictationFinished`. Each read therefore returned the previous dictation's numbers, and they are attributed to it.
- The companion column runs from the `dictation_stop` reply to `DictationFinished`.

**F2 works.** In R1, "Send it" was lost with a 0 s tail; now it arrives on all four send-it rows, with `command:send`, and the field is sent without the hold.

**The drain takes time on build-B.**
- The final "Send it" lands 2.4–3.2 s after the stop by the engine's own measure (3.1–4.6 s seen by the companion). The model works through the queued audio slower than real time.
- The 3 s hold is skipped, but the user still waits about as long before the send.
- The learn rows, whose words come earlier, drain in about 1.6 s.

**F1 readback on the installed companion** (`start-installed-args --settings dictation --shareable --dictation-shot`; PNG `153-w153b-10-settings-f1-readback-installed.png`):
- `settings: voice_settings ok — shape companion=full composer=full tui=full phone=full any_app=full; send=[send it] scratch=[scratch that] literal=literal; input_devices=[]; polish_model=chain; history enabled=True rows=200 audio_mib=64 learn=True; standby=8s duck=off`.
- The page shows the caption "Shown: the engine's presets, shared with the desktop, the terminal UI and the phone.", the engine's phrases, "Polish model: Chain (title model, then session)", and the history caption "the last 200 dictations and up to 64 MiB … Corrections are learned."
- There is no "older engine" caption.
- The page first rendered twice before the engine connection opened. Those two reads logged `voice_settings failed (rpc -32300: not connected)`, and the third render, after the connection opened, read OK.

**Restored:** the installed companion was started again, the Run key reads `--background`, `companion.json` has `launch_at_login: true`, and the desk was released at 02:21:10.

**Probe additions:**
- The ask-live round now logs `ask-live … timings: … stop_to_final_ms=…` after each round.
- `w6-gate.ps1 start-installed-args <flags>` starts the installed exe with flags.
