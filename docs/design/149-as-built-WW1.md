# 149 WW1 — as built (the shared C# contract: Rpc + DTOs + events + E6 + key table + icons + Store plumbing)

Branch `feat/149w-ww1` (basedatum), worktree `~/Develop/theuth-149w-ww1`, off `integrate/149` `77fa225f`, with the contract C `0280c0a7`
(H1) merged at `8a8ed6d3`. Scope: design 149 §11 "The Windows waves" row WW1, §7.4 #6 (same key table, icons, RPCs), §6 E6, §8.
`WIN` = `crates/desktop/companion-windows`, `COMP` = `crates/desktop/companion/Sources`.

## What exists (file → members)

### Rpc (`WIN/src/Theuth.Companion.Rpc`)
- `Models.149.cs` (new) — hand-written mirrors of the Rust wire shapes, each naming its source: `ItemContext { request, last_reply }`
  (Revision 1 #5); `SessionSummary` (`agent/src/lib.rs:2232`: id · created_at · updated_at · title · tags · model · parent · name · effort ·
  agent · cwd · harness_model · desk · pinned · archived · project · running · owner · here — every `#[serde(default)]` field optional);
  `SessionsStatus` (`sessionsync/types.rs:251`); `CancelScope` (`root|tree`) and `Delivery` (`steer|next_turn`) with `WireWords149.Word()`;
  `PointerSample { t_ms, x, y, pen }`, `ScreenContextReq`, `ScreenContextReply` (`computer/src/lib.rs:133-147`, `pointer.rs:12`);
  meeting (`meeting/src/lib.rs`): `MeetingState` (string-backed struct + converter: bare word or `{"Post": stage}`, `IsLive`),
  `Degradation` (word or `{"SttDown": track}` / `{"PermissionDenied": p}`), `MeetingSupport` (object `{kind, reason}`, bare word, or
  null → `Unknown`), `Topic`, `Brief`, `Utterance` (`Who`), `MeetingStatus` (`IsLive`), `MeetingPermissions`, `CachedBrief`
  (`cache.rs:27`), `ModeInfo` (`decision/src/lib.rs:584`, `Mode` kept as `JsonValue`); `MeetingEvent` — ONE tolerant record for all
  15 variants keyed by `Type` (`Types` set; helpers `TopicAdded`, `TopicRemovedId`, `AsUtterance`; nothing required, so a newer field or
  variant never throws); `SubagentSpawned` (`agent/src/lib.rs:2706`); `ActivityAppended { entry: JsonValue }` (`tasks/activity.rs:131`).
- `Models.cs` — `AttentionItem.Context: ItemContext?`, `Priority: string?`, `Rank: uint = 5` (C's additions); `Packet.IfWrong`;
  `WireEvent.Typed` routes `SubagentSpawned`, `ActivityAppended`, `Dictation{Started,Level,Text,Finished}` (→ the existing
  `DictationEvent`) and the 15 meeting words (→ `MeetingEvent`), so `WireEvent.Decode` returns typed payloads for them.
- `RpcClient.149.cs` (new partial) — positional params in `api.rs` order: `ListSessionsAsync` (:485), `ListAgentsAsync(session)` (:489),
  `SessionsStatusAsync` (:549), `AgentPauseAsync(name, paused)` (:2088), `CancelAsync(session, CancelScope?)` (:255),
  `SendInputAsync(session?, text, files, delivery?, id?)` (:194), `SteerAsync(session, text, files, delivery?, id?)` (:218),
  `ScreenContextAsync(req)` (:335), `PutAttachmentAsync(base64, ext)` (:1426), `MeetingStatus/Live/Briefs/Topics/Cursor/Pause/Resume/
  Stop/Tap/Dismiss/Deeper/Request/Ptt/OracleMode/PermissionsAsync` (:1558-1609, :1855, :1580), `BriefGetAsync` (:1912),
  `BriefSearchAsync(query, limit?)` (:1895), `SpeakAsync(session, text)` (:1467), `StopSpeakingAsync(id?)` (:1470). Error mapping,
  cancellation and reconnect are the existing client's (an engine error keeps its code; a lost socket fails the call with -32300).
  Reused, not re-added: `ListActivityAsync`, `AgentsListAsync`, `SendInputAsync(session, text)`, Core `AskRpc` (`MeshStatusAsync`,
  `DeskOpenAsync`, `SteerAsync(2)`, `ActivityListAsync`, `SetEventFocusAsync`, `Dictation*`).

### Core (`WIN/src/Theuth.Companion.Core`)
- `AttentionContext.cs` (new) — `Resolve(item, sessions, activity) → (Request, LastReply, FromEngine)`: E6 when the item carries a
  non-empty `context`, else the §6 interim join (`SessionSummary.title` / `AgentInfo.last` by the item's `session`); `Clip` = first line,
  ≤ 160 text elements, ellipsis.
- `HarnessIcon.cs` (new) — `Family(kind)` (omp · opencode · claude · codex · theuth · unknown; null/empty = theuth; prefixes such as
  `opencode2`, `claude-code` fold), `For(kind[, size])` → `<family>-<16|20|24|32>.png` (24 default), `Path()`, `Folder = Assets\harness`.
- `KeyBindings.cs` — `KeyScope` += `Agents`, `Island`, `PointAndSay` (titles "Agents panel", "Meeting island", "Point and say");
  `KeyAction` += `OpenAgents`, `VoiceKey`, `MeetingPtt`, `MeetingEye`, `TalkReply`, `PointAttach`, `StopAgent`, `AskPoint`,
  `Agents{Next,Previous,Open,OpenSession,Talk,Stop,Escape}`, `Island{Mute,Stop,Dismiss,Deeper,Send,Escape}`, `PointCancel`; the §8 rows
  with Windows defaults: Ctrl+Shift+A · **Ctrl+Alt+Space** (D16 / 7.4 #3) · Alt+` · Alt+Shift+E; panel V · P · X; askBar P; agents
  J · K · Enter · Ctrl+O · V · X · Esc; island M · Ctrl+M · E · D · Enter · Esc; pointAndSay Esc (fixed). Aliases: Space → `Reply`
  (Revision 1 #18), Ctrl+P → `AskPoint`, ↓/↑ → agents rows. `VirtualKeys.Oem3Backtick`. Collision policy unchanged
  (`IsUniquePerScope` and `Check` now span six scopes). Settings › Keys and the legend enumerate `KeyScope`, so the new rows list
  themselves.
- `StoreEngine.cs` — `IStoreEngine` gains default members for every new call (W1–W6 fakes compile); `RpcStoreEngine` forwards them.
- `StoreModel.cs` — now `partial`; three marked lines: `Observe149(e)` in `HandleEvent` (before `AskObserve`), the raw hook raised last
  (also for a typed item that failed to decode), `_ = Refetch149Async()` after each `Refetch`.
- `StoreModel.WW1.cs` (new partial; every member XML-documented; `PropertyChanged` on the owner thread):
  - agents/sessions: `Sessions`, `SessionsStatus`, `Mesh`, `SelfEngineId`, `HostNameOf(session)`, `SessionOf(info)`, `ChildrenOf(info)`,
    `ContextFor(item)`; events `SubagentSpawnedReceived`, `ActivityAppendedReceived`, `SessionsRefetched`;
  - meeting: `Meeting`, `Topics`, `Briefs`, `MeetingCursor`, `Live` (final lines, cap 200), `MeetingLive`, `MeetingMicLevel`,
    `MeetingSystemLevel`, `MeetingListening`; events `MeetingEventReceived`, `MeetingRefetched`; the Store applies TopicAdded/Removed,
    Brief, Level, Listening, ApprovalWaiting, State, final Utterances itself and refetches on Started/Deleted/ReviewReady/Summarized/
    PostProgress;
  - dictation: `DictationEventReceived` (typed, before the Ask bar's reducer);
  - raw: `EventReceived(type, utf8)` — WW3's hook, raised at the END of `HandleEvent` on the UI thread;
  - keys: `GlobalKeyPressed(KeyAction)` / `GlobalKeyReleased(KeyAction, held)` + `GlobalKeyDown/Up` for the app to call;
  - actions: `CancelTreeAsync`, `SendInputAsync(session, text, files)`, `SteerAsync(session, text, files)`, `ScreenContextAsync`,
    `PutAttachmentAsync`, `Meeting*Async`, `BriefGet/SearchAsync`, `SpeakAsync`, `StopSpeakingAsync`, `AgentPauseAsync`,
    `Refetch149Async` (list_sessions + sessions_status + mesh_status, each tolerated alone), `RefetchMeetingAsync` (meeting_status,
    then meeting_briefs + meeting_live when a meeting exists; on connect = every `PendingPrompts`, and on meeting events).

### App (`WIN/src/Theuth.Companion`)
- `Store.WW1.cs` (new partial) — `InstallHotkeys(Hotkeys)`: forwards `Pressed`/`Released` to the Core events; sets `Hotkeys.Gate` so
  `meetingPtt` / `meetingEye` are registered only while `MeetingLive` (§8) and re-applies the table on every change.
- `Hotkeys.cs` — `Gate` (rows skipped by `Apply`) and `Applied` (the last table); two small additions to W2's file.
- `Windows/Surfaces.cs` — one marked line after `Hotkeys.Pressed += OnHotkey;`.
- `Probe149.cs` (new) + one marked line in `App.xaml.cs` — `--probe N` also logs `probe149 …` lines: list_sessions · sessions_status ·
  mesh_status · meeting_status · one list_agents(first session), and the harness-icon count beside the exe.
- `Theuth.Companion.csproj` — a `<!-- 149 WW1 -->` ItemGroup linking `..\..\..\companion-assets\harness\*.png` as `Assets\harness\*`
  (Content, CopyToOutput/PublishDirectory), so `build.ps1`'s `dotnet publish` keeps the 24 PNGs.

### Tests
- Rpc (`WIN/tests/Theuth.Companion.Rpc.Tests`): `Models149Tests` (tolerance: unknown type, extra fields, every meeting variant,
  dictation typed, context optional, old/new SessionSummary, status/brief/perms/mode shapes, params encoding); `Fixture149Tests` — C's
  15 shared `*149*.json` fixtures (linked, not copied: the csproj glob already links the whole Swift fixture dir) decode, the port of
  `FixtureTests.swift:147-264`; `FixtureTests.Shapes` rows for them; `RpcClient149Tests` — every new call against the loopback Kestrel
  engine (params in api.rs order, results, -32601 from an older engine, a null result = `JsonException` and the loop survives, the new
  stream kinds reach the callback). `RpcClientTests` and `RpcClient149Tests` share one xunit collection (see Deviations).
- Core (`WIN/tests/Theuth.Companion.Core.Tests`): `WW1Tests` — `KeyTable149Tests` (Windows defaults per scope, six-scope uniqueness,
  titles, fixed rows, aliases, per-scope collisions + Move + JSON round trip, the voice key's resolution), `AttentionContext149Tests`
  (E6 wins, interim join, clip on a text-element boundary), `HarnessIcon149Tests` (lookup; every named file ships in the shared set);
  `StoreModel149Tests` — PendingPrompts → meeting fetch + refetch → sessions/status/mesh with host names, older engine tolerated call by
  call, meeting events update the island state and reach typed-then-raw hooks in order, the raw hook sees unknown and undecodable items
  but never typeless ones, Subagent/Activity/Dictation typed, joins and thin actions, global keys forwarded.

## Verification
- Mac (`~/.dotnet`, `nice -n 10`): `dotnet build Theuth.Companion.Portable.slnf -warnaserror` clean; `dotnet test Theuth.Companion.Portable.slnf`
  — **Core 169 / 169, Rpc 68 / 68** (was 146 / 23 before WW1). R1: three consecutive Rpc runs under `--blame-hang 90s` at load ~36:
  4 s · 5 s · 4 s, all 68/68; Core 169/169 (see Deviations 10 for the hang's root cause).
- build-B Debug build (`Theuth.Companion.slnx`, Platform=x64, slot ww1): skeleton `2c965119` → exit 0, 0 errors, 0 warnings, 46 s
  (id 20261009T010102-3); `b9022543` → 0 / 0, 41 s (20261009T013133-39); `7ed29d98` → 0 / 0, 37 s (20261009T013451-53).
- Live read-only smoke on build-B's installed engine: see §Live smoke below.

## Live smoke (build-B, installed engine)
Round at 02:45–02:47 under the desk lock (owner ww1): `w6-gate.ps1 quit-installed` → `probe.ps1 -Seconds 20 -From debug` (the slot's Debug
exe at `7ed29d98`) → `w6-gate.ps1 start-installed` → `w5-gate.ps1 reg` → release. Engine: the INSTALLED theuth on build-B (2.12.3 at the
time; `ws://127.0.0.1:54937`, 64-char token), no model credential, nothing seeded. `PROBE OK`, exit 0. What the live engine answered
(`evidence/ww1/probe149-7ed29d98.log`):

| call | answer |
|---|---|
| `meeting_status` | `state=Idle meeting=none topics=0 supported=Partial degraded=[]` (briefs 0, live 0 — no meeting on a box with one virtual display) |
| `list_sessions` | 4 sessions, `running=0`, `remote=0` (every row `here=true`) |
| `sessions_status` | `engine=69ceb456…7751e5 here=4 elsewhere=0` |
| `mesh_status` | `self=P50 peers=0 reachable=0` |
| `list_agents(01a11896…)` | `0 []` (the first session has no subagents) |
| W1 lines, unchanged | `PendingPrompts approvals=0 asks=0 secrets=0` · `attention_list=0 agents_list=1 list_activity=4 live=0` |

Also in that log: the four new global rows registered (`openAgents = Ctrl+Shift+A`, `voiceKey = Ctrl+Alt+Space`, `meetingPtt = Alt+\``,
`meetingEye = Alt+Shift+E`) and, after `Store.WW1` applied the gate, `meetingPtt` / `meetingEye` "not registered now (gated)" with
`surfaces up: … hotkeys=[openPanel=Ctrl+Shift+Space, openAgents=Ctrl+Shift+A, voiceKey=Ctrl+Alt+Space]`; and `harness icons beside the
exe: 24 png in Assets\harness (sample omp-24.png exists=true)`. After the round the installed companion was restarted (pid 13672) and
`HKCU\…\Run\TheuthCompanion` points at `%LOCALAPPDATA%\theuth\companion\Theuth.Companion.exe --background` (`evidence/ww1/w5-reg.log`).
Not exercised live (no agent turn possible tonight): E6 bubbles from the engine, a live meeting (Started … Deleted), `cancel`, `send_input`
with files, `screen_context`, `speak` — all covered by the loopback-engine and fixture tests above. One display only.

## Deviations from the design (and why)
1. **`MeetingEvent` is one tolerant record, not 15 C# types.** WW3 asked for the raw stream hook and decodes meeting events itself;
   the typed record is for the Store's own island state and tests. Nothing in it is `[JsonRequired]`, so (unlike the Swift
   `MeetingEvent.decode`, which throws on a missing field) a malformed item decodes with defaults — the raw hook still carries the bytes.
2. **Meeting enums are string-backed structs/strings, not C# enums.** The shared `JsonStringEnumConverter` is snake_case and refuses
   unknown words; the meeting crate writes PascalCase (`Recording`, `Mic`, `Granted`, `Auto`) and `MeetingState` / `Degradation` / `Support`
   are externally or adjacently tagged. `MeetingState`, `Degradation`, `MeetingSupport` have converters; `Track`, `BriefSource`,
   `PermissionState`, `Priority` are the wire word as `string`; `ModeInfo.Mode` is a `JsonValue`.
3. **`MeetingSupport` also reads a bare word and null.** The Rust shape is `{"kind":"Full"}`; C's fixtures write `"Full"` / `null`
   (`meeting_status_149_idle/_post`). Both are accepted, as the Swift model does.
4. **Island stop is Ctrl+M, not Ctrl+Shift+M.** Local-scope chords are stored without Shift (`Chord.NormalizedLocal`), so a §8 ⌃⇧M row
   could never match a typed chord; C made the same call for the Mac table. A typed Ctrl+Shift+M resolves to it.
5. **Agents-scope "⏎ open session · ⌘O" became two rows** (`AgentsOpen` Enter "Open the row's session", `AgentsOpenSession` Ctrl+O
   "Open the session in theuth"); both map to `Intent.OpenSession`. WW2 decides what each opens.
6. **No per-platform default table in C#.** C added `KeyTable.Platform` with `platformDefaults[.voiceKey]`; the C# table is Windows-form
   only by design (134 D6), so its one column IS the Windows column of §8 (`Ctrl+Alt+Space`). A `Platform` enum would have nothing to switch.
7. **`HarnessIcon.For(kind)` names the 24 px file**, not a bare `<name>.png` (the brief's wording): the shared set ships only sized PNGs
   (`<name>-{16,20,24,32}.png`) plus SVGs; `unknown` → `unknown-24.png`.
8. **Meeting hotkeys are gated in the app, not the table.** §8 says PTT/eye are "registered only while a meeting is live"; `KeyTable`
   stays data, `Hotkeys.Gate` + `Store.WW1` re-apply on `MeetingLive` changes. A debug companion with no meeting shows
   `hotkey meetingPtt: not registered now (gated)` in its log.
9. **`Refetch149Async` runs after every W1 refetch** (sessions + status + mesh, ~3 local calls per 250 ms debounce), not on its own timer;
   `meeting_status` is fetched only on connect and on meeting events, so an idle engine is not polled.
10. **The Rpc test host hang — root cause and fix (R1).** Under load the test host hung at the first loopback engine start; the
    hang dump (`dotnet-dump analyze … clrstack -all`, 03:22) shows the test thread inside `WebApplication.CreateSlimBuilder()` →
    `ApplyDefaultAppConfigurationSlim` → `JsonConfigurationSource.Build` → `FileConfigurationProvider..ctor` → `ChangeToken.OnChange` →
    `OnChangeTokenFired` → `RegisterChangeTokenCallback` → … recursing without end: the slim builder adds `appsettings*.json` with
    `reloadOnChange: true`, and on macOS under load the `PhysicalFileProvider` watch token fires at once, forever (the 51 % CPU seen on the
    spinning host). Fix: `FakeEngine` builds with `WebApplication.CreateEmptyBuilder()` + `UseKestrelCore()` — no file configuration, no
    watcher — plus `xunit.runner.json` `parallelizeTestCollections: false` for the Rpc assembly (one Kestrel host at a time) and a 15 s
    bound (`FakeEngine.Bound`) on every server/client await, `StartAsync` and `DisposeAsync`, so a regression fails instead of hanging.
    Proof at load ~36: three consecutive full Rpc runs under `--blame-hang --blame-hang-timeout 90s` — 68/68 in 4 s, 5 s, 4 s — and
    Core 169/169 in 0.5 s. (The earlier "one collection" change was a symptom patch; it stays, harmlessly.) The pre-149 7-minute Rpc runs
    were the same mechanism in its slow form.
11. **E6 is decoded but not yet produced by any engine** (W0 / H2 pending at the time of writing); every card bubble on build-B comes
    from the interim join. `Fixture149Tests.AttentionItemContextPriorityRank` pins the wire shape.

## Open items
- H2 (W0's `AttentionItem.context` on the engine): nothing to change on the C# side unless the field names move; re-run
  `Fixture149Tests` against the regenerated fixture when W0 lands.
- H3 (the Mac W1/W2/W3 constants and tests): not received while WW1 ran; WW2/WW3 port what touches their models. WW1-side candidates
  are `PointMachine` constants (if C's `holdMs 200 · doubleTapMs 300 · minNoteMs 800 · sampleHz 60 · doneMs 6000` change) — none of them
  live in WW1's files.
- E7 (mesh-wide `list_activity`) is absent as briefed; `HostNameOf` + `SessionSummary.owner/here` give WW2 the hollow-dot fallback.
- `meeting_oracle_mode` / `decision_mode` are callable; no Store state is kept for them (no surface reads them yet).
- `ActivityAppended.entry` is raw (`JsonValue`): Core's `ActivityEntry` (ReceiptModel.cs) is the decoder when a surface needs the row;
  the Store does not keep an activity feed.
- The `SubagentSpawned.cap` word and `AttentionItem.priority` are strings, not enums, on purpose (tolerance); add enums when a surface
  switches on them.