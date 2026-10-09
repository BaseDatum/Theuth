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

## Verification (step 3)
| Gate | Result |
|---|---|
| Mac Rpc tests (`dotnet test tests/Theuth.Companion.Rpc.Tests --blame-hang`) | **97/97** (68 before + 29: `Models153Tests` 20, `RpcClient153Tests` 3, `Fixture153Tests` 6 over the 13 cross-platform fixtures) |
| Mac Core tests (`dotnet test tests/Theuth.Companion.Core.Tests`) | CORE_COUNT (310 at the start; + W153C's merged tests; W153B adds `AskVoiceTests` wave-B cases incl. **`SendItSkipsTheHold`**, the early-mic tap race, the speech press, and `DictationSettingsTests`); the 149 `AskVoiceMachine` cases unmodified |
| build-B Debug x64 (`Theuth.Companion.slnx`, `Platform=x64`) | PIN 4fcbcf1e **0 err / 0 warn** (52 s); FINAL_BUILD |
| TS bindings re-check (H-A) | main 5afea171's voice/agent/rpc types are unchanged since b7ec00b9 except behaviour (`"dictionary loading"` → `DictionaryRetry`); the generated bindings (`~/Develop/theuth-153-int/crates/desktop/ui/src/rpc/bindings`, 19:38) match every record — field names, optionality, tags (`kind`, `command`, `scope`), u64 as `ulong`, `metaphone: [string, string]` — except the two flat-dir name collisions under Open items |
| Fixture UI round 1 (19:54, 1e92fa11) | `--ask-fixture all`: shaped partial/final in the field (UIA `Ask theuth` value "Ship it tonight" during send-it), the shaped text in the 3 s hold, then `ask sent to desk (55 chars)`; send-it: status "sending in 1.5 s…", `voice: "send it" — sent without the hold (15 chars)` 19:54:48.226 → `ask sent to desk (15 chars)` 8 ms later (no hold); the suggestion card shown (Mica Base, frame D's words) and auto-dismissed after 20 s with nothing sent; Settings › Dictation against the pre-wave-A engine shows the `-32601` answers as readable lines |
| Fixture UI round 2 (20:33, 88f7eee8) | the six app-rendered PNGs below; the Run key back on the installed companion; desk released |
| Live round (H-ENGINE) | LIVE_ROUND |

Evidence: `~/.theuth/briefs/2026-10-09/153-windows/evidence/W153B/` — `build-*.json`, `fixture-round-{1,2}-*.txt`,
`153-w153b-{1-shaped-live,2-shaped-hold,3-sendit-armed,4-sendit-sent,5-suggestion,6-settings-dictation}.png` (+ `-on-dark` composites),
LIVE_EVIDENCE `README.md`.

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
1. **Live round** — LIVE_OPEN
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
