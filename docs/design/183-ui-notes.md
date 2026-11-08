# 183 — Phase 1 UI notes (U183)

Built on the contract commit `7c815e00` and checked against the engine worker's "Contract changes after C" (`36387802`): no TS shape changed; the bindings in this worktree are byte-identical to theirs.

## What was built (by brief item)

| # | Where | What |
|---|---|---|
| 1 | `components/sidebar/SessionItem.tsx` `EngineChip`, `components/chat/Header.tsx`, `lib/owner.ts` `ownerState` | `on cloud-1 · live` (green dot) while the owner is connected; `on cloud-1 · offline` when it is not, or a routed call just answered `-32010`; bare `on cloud-1` before the roster knows the owner. The compact (phone) header's Owner row reads `cloud-1 · live · epoch N` |
| 2 | `components/chat/Composer.tsx` `placeholderFor`; `lib/owner.ts` `useOwnerOffline` / `routed` | `Message — runs on cloud-1` (idle and steering, per mock panel 3); `Runs on cloud-1` on a touch screen so the name never wraps. Every routed action parks behind the existing `OwnerOfflineCard` on `-32010`: send/steer/interject (as before), stop, flush, the ⋯ setters and the ⋯ slash commands (compact, undo, …). Continue here takes the session, then runs the parked call; Wait drops it |
| 3 | `components/chat/SessionMenu.tsx` `settingsRows` | In order: header (remote only), the offline line, Approvals ▸ On / Off / Machine default (<on/off>), Agent messaging, Model › (opens the session's picker), separator, Continue here (move to <this machine>), Move to ▸. Readers `get_approvals` / `get_communication` while open; setters `set_approvals(s, on)`, `set_approvals(s, global, clear=true)`, `set_communication`. Same block for a local session, without the header and with Continue here greyed. Owner offline → setters grey, Move to says "cloud-1 is offline — Continue here first" |
| 4 | `components/chat/ApprovalCard.tsx` | Owner ≠ this engine → `cloud-1 asks: shell` and `mutating · relayed from cloud-1 · denies at 11:47`. Allow/Deny unchanged (`resolve_approval` forwards) |
| 5 | `lib/injected.ts` (`moved`, `engine_restart`, `parseMoved`, `parseRestart`, `movedFromRecord`, `tagAttrs`), `lib/time.ts` `splitRestart`, `components/chat/HarnessCards.tsx` `MovedCard` / `EngineRestartCard`, `Message.tsx`, `ToolRunCard.tsx` | `HistoryEntry::Moved` renders from its record; a `<session_moved/>` input classifies to the same card. A tool result opening with `<engine_restart/>` (live or stored, either side of `<time/>`) loses the note, which becomes a card above the call; such a call splits a tool run so the card stays visible. "What the agent was told" shows the note's fields as a list, never the tag |
| 5 | `lib/injected-wrappers.test.ts` | The scan also finds self-closing openers (`<engine_restart …/>`, and `<time now="…"` closed by `push_str("/>")`); `time` is listed in `NOT_A_ROW` (stripped by `splitTime`) |
| 6 | `components/chat/NewChatPane.tsx` `MachineBar` / `machineRows`, `lib/drafts.ts` `useDraftTarget` + `openDraftSession`, `ModelMenu.tsx` `useModels` | `New chat on [machine ▾]` (shown only with paired peers): this machine first with ✓, `os_name · class`, `LAN` / `via relay` / `offline 3h` (disabled). Folder ▾: `Folder on <m>`, default, the target's `EngineStats.repos`, `Browse <m>…` (a dialog on `path_complete(null, input, cursor, engine)`). Models: `list_models(engine)` for the draft's machine and for a remote session's owner. The first send's `create_session` carries `engine` / `cwd`; the pick resets to this machine |
| 7 | `SessionItem.tsx` `SessionActivity`, `lib/rpc.ts` | For a row owned elsewhere, `list_sessions.running` gates the activity glyph; `ContextChanged{reason: "replicated"}` triggers the existing debounced sessions refetch |
| 8 | `App.tsx`, `store/reduce.ts`, `routes/c.$session.tsx` | Focus is re-sent on `SessionOwnerChanged` so the engine re-follows the new owner. The reducer never drops a `replicated` / `moved` `ContextChanged` on a same revision (the owner's relayed event can carry it before the rows land). A remote chat settles on `TurnFinished`. A `peer` disconnect withdraws that owner's relayed approvals and asks (no `*Resolved` follows a dropped stream) |
| 9 | phone, 430 px | Menus are `max-w-[calc(100vw-1rem)]` / `min(21rem, 100vw-2rem)`; screenshots at 430 px below |

## Calls made (simplest option that keeps the design)

- **Chip text offline**: the brief's `on cloud-1 · offline`, not §12.1's `cloud-1 offline`. "Live" = the owner is connected (the open chat is in focus, so the engine follows it); the UI cannot see the follow stream itself.
- **Placeholder while steering**: mock panel 3 shows `Message — runs on cloud-1` during a running turn, so a remote chat uses it while steering too; `Send privately…` (exports) and talk/skill keep theirs.
- **Continue here label**: `Continue here (move to <this machine's mesh name>)`, e.g. "(move to Sky's MacBook)"; the mock's "this Mac" has no source field.
- **Approvals tri-state**: value from `ApprovalsState.pin` only (E183: `session` is now effective, ancestors included); no pin → "Machine default (<tools.approvals>)". An ancestor's pin also reads as Machine default.
- **Relayed approval deadline**: the event has no deadline, so the card shows first-seen + 300 s (the owner's default `approval_timeout_secs`). Marked `ponytail:`; an owner with another timeout shows the wrong time.
- **Agent messaging** shows `CommunicationState.effective`.
- **Private + machine**: a private chat starts here (the Machine button disables while Private is on, and turning Private on resets the pick).
- **Repos** show without the mock's `git · main` chip: `EngineStats.repos` has no branch.
- **Machine class words**: `laptop`, `desktop`, `server`, `container`, `VM` (the mock's "cloud node" has no class).
- **Approval answers** on an offline owner toast as before (not parked): the owner's own timeout decides them (§4.6).
- **Sessions refetch debounce** reuses the list's 500 ms, not §5.2's 1 s.
- **Not built**: the sidebar's "New chat on ▸" submenu and the ⌘K entry (§12.5), Effort › in ⋯ (the brief's list has none; the session picker carries effort), the draft's default model from the target (`info.default_model` is this engine's until the first reply). *Z183 built the first three; see `183-notes.md` "Phase 1 as built + live proof".*

## Screenshots (`/tmp/rs183u/shots/`, a throwaway fixture with a fake Api, deleted)

`3-remote-chat.png`, `3-menu-approvals.png`, `3-offline-menu.png`, `4-moved-restart.png`, `5-machine-picker.png`, `5-folder-on-xeus.png`, `phone-3-remote-chat.png`, `phone-3-menu.png`, `phone-4-moved.png`, `phone-5-machine-picker.png`.
