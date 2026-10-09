# 153 as-built — waves B + C (closer ZB: desktop UI, TUI, macOS companion, engine native tools)

Worktree `~/Develop/theuth-153-bc`, branch `integrate/153-bc`, cut from `integrate/153` `bdc7d7b1` (wave A, closer Z). Design
`153-voice-anywhere.md` §16 rows 12–22; the workers' as-builts `153-as-built-{CC,B1,B2,B3,C1,C2}.md` are the detail — this file is the
merge, the seams, the consolidated deviations and the gates on the merged tree. Gate outputs under `/tmp/153-zb/`.

## Merge order (each `git merge --no-ff`, all six textually clean — no conflict markers)
| # | branch | sha | merge commit |
|---|---|---|---|
| 1 | `feat/153-cc` companion contract | `557eb083` | `a91a125f` |
| 2 | `feat/153-b3` companion fields | `300cd9f0` | `37c56e91` |
| 3 | `feat/153-c1` command mode + card + keys | `6fb03669` | `06285116` |
| 4 | `feat/153-b1` desktop UI | `9b41c07a` | `20417597` |
| 5 | `feat/153-c2` engine native tools | `1fa78c3b` | `bd594e75` |
| 6 | `feat/153-b2` TUI (rebased on `b7ec00b9`) | `f7154ce8` | `a0b90580` |

The expected B3 ↔ C1 overlap (`Store.swift:309` `showAnswerCard`, `SettingsStore.swift:66` `anyProbe`) did not conflict: CC's hook
signature is the one both sides use; C1 replaced the body (`pointShowCard`), B3 only calls it.

## Integration items
- **B3 ↔ C1 seam** (`4f0ed0cb`, `AskBar/Store+Ask.swift`): B3's `commitAsk` called `askCard.begin(question:)`, which takes C1's default
  `kind: .voiceNote` — the Ask card would have shown the `[voice note, N s]` marker on its follow-up field. Now `begin(question:, kind: .ask)`
  and `askCard.session = desk` after `resolveDesk()` (O / typed follow-ups on the card reach the desk that answered). The streaming path
  (`askObserve` → `phase/text/status/turnFinished` → `showAnswerCard` → `PointController.showExternal`, expansion kept while
  `generation` is unchanged) compiles and `AskTests` / `W6Tests` pass unchanged. "send it" on the row mic / card V: `HoldToTalk.onSend`
  exists; C1's voice follow-up is "hold the mode key" and does not call B3's machine — left as built, recorded here.
- **B1 hand-offs** (`209fbd0e`): hand-written `VoiceStatus` deleted from `rpc/extra.ts` (only `api.ts` used it; it now imports the
  generated `bindings/VoiceStatus`, which carries `input_devices`); `voice/VoicePanel.tsx` `Words` filters and renders `dictation.shown(u)`
  (`shaped || (command ? "" : text)`) so the voice key's pill shows the shaped form like the Composer.
- **C2 ↔ C1 `purpose` key**: the engine's `ToolCallStarted` serialises `args: serde_json::Value` and `call_id` (`crates/agent/src/lib.rs:2763`);
  the companion's `PointController.Wire` decodes with `TheuthJSON.decoder` (`convertFromSnakeCase`) and reads `w.args ?? w.arguments`,
  then `args["purpose"] ?? args["reason"]` — the right key. No `ToolCallStarted` fixture exists under `Tests/`; the `Wire` struct is private
  to the Companion executable target (not testable from `CompanionCoreTests`) — confirmed by reading serde ↔ `CodingKeys`, not by a test.
- **Bindings** regenerated from this tree (`mbx test -p rpc --features ts --lib export_bindings` → `8 passed`, 838 files copied to
  `crates/desktop/ui/src/rpc/bindings/`, gitignored) before `tsc`; B1 had built against A4's copy — no type drift (`tsc` 0 errors).
- **no-emoji gate** (`422d0c1a`): B1 used `▶` in three comments (`settings/Voice.tsx`, `ui/DictionarySuggestion.tsx`, `lib/dictation.ts`);
  `no-emoji.test.ts` counts U+25B6 as pictographic. Comments now say "Play" (the buttons were lucide `Play` already).

## Consolidated deviations (frozen shape → shipped → why)
| § | Frozen / design | Shipped | Why | Who |
|---|---|---|---|---|
| §4.1 | `Shaped.replaced` underline in the field | not on the wire; fields show `shaped` only | `DictationText` carries `shaped/command/actions`; `Replaced` not ts-exported | B1 |
| §4.4 | field shows `shaped`, fallback `text` | `shaped \|\| (command ? "" : text)` (UI, TUI, companion `shaped ?? text`) | "send it"/"scratch that" alone shape to `""` on purpose; raw text only for an older engine | B1 B2 B3 |
| §7.4 | Ask "send it" → `setText` then `commitAsk` with undo hold | `.sendNow` → `sendAskNow` → `commitAsk`, hold skipped; TUI/UI `submit()` once at `DictationFinished` with a **final** `command: send` | the actor owns the settle; a partial that only armed never sends | B3 B2 B1 |
| §3.1 | settling copy "sending in 1.2 s · ⎋ keep editing" | "sending…" (companion), header "sending…" + ⏎ send now / typed char keeps editing (TUI) | no countdown source on the surface | B3 B2 |
| §8.5 | mic opens at key-down; 250 ms hold | same; chunks buffered until `started`, a tap (< 250 ms) discards them — no `dictation_start`; setting off → mic at the hold | the 149 `AskVoiceMachine` tests stay unmodified | B3 |
| §6.1 | correction at send on every surface | Ask bar, Composer, TUI; companion `HoldToTalk` callers (`AgentsPanel`, `ReplyField`) get `correctionId` but do not report | not B3's files — hand-off (wave D) | B3 |
| §6.4 | 20 s auto-dismiss = Not now without cooldown; ▶ `dictation_audio_get(id, utterance)` | dismiss sends nothing (A3 counts the ignore); play `(id, null)`; UI attaches the id of the last correction report; TUI `p` says "desktop only" | event carries no dictation id; `has_audio` false today | B1 B2 B3 |
| §8.1 | reorderable device list | one `Select` → `set_input_devices([name])`, default → `[]`; TUI `/voice mic <name>` | one preferred device covers the brief | B1 B2 |
| §8.7 | Settings shows `voice.shape.*` / end phrases / learn flag | write-only (`set_shape`/`set_end_phrases`), last value remembered in `localStorage`; companion row is a read-only pointer; TUI shows no preset | no getter in the frozen §14 contract | B1 B2 B3 |
| §10.3 | `purpose` per tool call | top-level `purpose` key (explicit arg on `applescript`/`shortcuts`/`powershell`, `**extra` kwarg elsewhere), prefixes `checking:`/`executing:`/`verifying:`, ≤ 60 chars, fallback `reason` → tool name | prompt-only change in the engine, as §10.2 says | C2 C1 |
| §10.4 | cap at 12 steps | the 12th `ToolCallStarted` freezes the card and the controller cancels | — | C1 |
| §10.6 | approval listen with `end_commands` off | 8 s companion dictation (`surface: companion, destination: agentPrompt`), reads `text` only, ignores `command` | `DictationOpts` has no per-call end-command switch | C1 |
| §11.2 | risk reclassifier + `computer.native` policy | **not built**; `Risk::Mutating` on all three, default policy `Ask`; skill carries confirm-before-delete rules | registry's only permission input is `spec().risk`; a per-call seam is new work | C2 |
| §12.2 | voice follow-up through B3's machine | hold the mode key (`followUp`); typed follow-up marker `[command, 1 s]` | one controller owns the gesture | C1 |
| §12.4 | `Phase.speaking` | dropped; `speaking` is a parallel glyph flag | the stub listed both | C1 |
| §12.6 | modifier-only / fn chords bindable | settings row + "Grant Input Monitoring…" button only | needs `Chord.keyCode` optional + `flagsChanged` monitor — wave D | C1 |
| §14.3 | `appProfiles: [AppProfile]` | one JSON blob under `appProfiles`; "Apps" placeholder in Dictation settings | wave D fills | CC B3 |
| §14.4 | `DictationWire` gains `shaped/command/actions` | public `TheuthKit.DictationWire`; the private copy in `Store+Ask.swift` deleted | one wire → reducer mapping (`AskDictation.event`) | CC B3 |
| §15 | `Hotkeys.names` registration | per-controller `applyHotkey` (voice key's way); `dictateAnywhere` registers nothing yet | wave D | C1 |
| §9.2 / §16 | `Voice.test.tsx` | `Voice.test.ts` static-markup render | vitest include is `src/**/*.test.ts` | B1 |

## Gates (merged tree; `/tmp/153-zb/*.log`)
- Swift: `build-lock swift build` → `Build complete! (86.19s)` (pre-existing `shared` / `NSEvent Sendable` warning classes only; 4 in 153 files,
  same classes); after the seam fix `Build complete! (9.88s)`. `build-lock ./test.sh` → `✔ Test run with 232 tests in 38 suites passed`
  (= 211 + B3 6 + C1 15), twice (before and after the seam commit).
- Companion probes (render-only, `--shareable`, nothing sent, no TCC prompt), PNGs `/tmp/153-zb/`: `153-command-{heard,working,approval,done}.png`,
  `153-card-{collapsed,expanded,failed}.png` (log `probe-c1.log`, `frame=(1060, -140, 440, …)`); `153-settings-dictation.png`
  (`--probe-settings dictation`, window 13761); B3's `--ask-probe 40 --shareable --synthetic-dictation` against the live engine 2.14.0 on
  127.0.0.1:56525 (no mic): all 6 asserts PASS (`ask-probe-dictation.log`: CANCEL IN FLIGHT, RELEASE BEFORE START, REFUSED, TAP, HOLD,
  SUGGESTION CARD `anchor=(1040, -384, 480, 72)`), card crop `153-suggestion-card.png`.
- UI: `bun install` (ui 622 pkgs, apps 484); `bunx tsc --noEmit -p crates/desktop/ui` → exit 0, **0 errors**; `BUILD_SLOTS=1 bunx vitest run` →
  first run `1 failed | 235 passed (236)` / `1 failed | 1563 passed (1564)` (no-emoji, fixed above; re-run of that file `1 passed`), full suite
  otherwise green, no `pairing.test.ts` flake; `bun run build` → `✓ 14080 modules transformed. ✓ built in 22.07s`.
- Rust: `mbx check -p computer -p tools -p theuth -p tui --tests` → `Finished dev profile in 16m 49s`, 0 warnings/errors;
  `mbx clippy -p computer -p tools -p tui --tests` → `Finished dev profile in 3m 11s`, 0 warnings; `mbx test -p computer -p tui --no-run` →
  `Finished test profile in 4m 20s`, then bare `MBX_SCHEDULER_TESTS=0 mbx test -p computer -p tui` → computer `63 passed; 0 failed`,
  tui `166 passed; 0 failed` (+ `no_emoji` 1 passed); `BUILD_SLOTS=1 mbx check --workspace --exclude desktop` → `Finished dev profile in 1m 14s`,
  0 errors; `cargo fmt -p computer -p tools -p theuth -p tui -- --check` → exit 0, no diff.
- Fold: `git merge --no-ff origin/main` `5afea171` (wave A landed on main as `Merge integrate/153`; 5 files: `voice/dictation.rs`
  `DictionaryLoaded`/`Loading`, `rpc/api.rs` the `Loading` arms, two A as-builts) → `c4b7b759`, clean. Re-gated: `mbx check -p computer -p
  tools -p theuth -p tui --tests` → `Finished dev profile in 2m 10s`, 0 warnings; `swift build` → `Build complete!`; `tsc` → 0 errors.
  No ts-exported type changed (actor-internal messages), bindings not regenerated.

## Hand-offs
- **Wave D (any-app)**: `KeyAction.dictateAnywhere` (⌃⌥D) is in the table and settings but registers no hotkey; `ModeKeySettings.appProfiles`
  JSON blob + the "Apps" placeholder row; `Destination.anyApp` / `Scope.surface(.anyApp)` encodings pinned by `FixtureTests`; modifier-only
  chords need `Chord.keyCode` optional + `KeyTable.check(.needsInputMonitoring)` + a `flagsChanged` monitor (fn 63, ⌘ 55/54, ⌥ 58/61, ⌃ 59/62,
  ⇧ 56/60); `HoldToTalk` callers in `AgentsPanel`/`ReplyField` should send `dictation_correction(correctionId, text)` at commit.
- **Windows twin**: fixtures `Tests/TheuthKitTests/Fixtures/153-*.json` are the C# contract input; constants `holdThreshold 250`, `maxDuration 60 s`,
  `tapToggleMs 400`, `maxSteps 12`, `textCap 60`, `listenSecs 8`, `doneMs 6000`, suggestion auto-dismiss 20 s; `dictation_start` params
  `[16000, {"surface":"companion","destination":{"kind":"agent_prompt"},"commands":[],"press_ago_ms":N}]`; `powershell` tool
  `{script, timeout_secs?, purpose?}` → `pwsh -NoProfile -NonInteractive -Command`, fallback `powershell`, untested on Windows here.
- **Engine (§18 frozen-shape follow-ups)**: a getter for `voice.shape.*` and `voice.end_commands` (e.g. on `voice_status`) so Settings read instead
  of remembering; `DictionarySuggestion` carrying the dictation id (UI plays without the reporter lookup); a private-session flag + agent names
  in `DictationOpts` (B1's `commands` carry slash names only; `@mentions` are not passed); §11.2 per-call risk seam if a policy that allows
  `mutating` but asks for `destructive` native calls is wanted.
- **Not run live** (needs Sky at the keyboard): §16 row 21 ⌃⌥C → approval by voice → ✓ line; mode switch mid-hold; Composer rows 14/15 against a
  153 engine (the engine on this Mac is 2.14.0, pre-153: no `shaped`, no `dictation_status.last`).
