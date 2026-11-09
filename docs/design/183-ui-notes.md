# 183 — UI notes (U183 Phase 1, U2 Phase 2)

## Phase 2 (U2): Settings › Network, Nearby, pair dialog, Unpair with rotation

Built on the contract commit `5eb03940` (C2). Before finishing, `theuth-183`'s `183-notes.md` had no "Phase 2 contract changes after C2" section and its bindings and `api.ts` were byte-identical to this worktree's, so nothing was adapted.

| # | Where | What |
|---|---|---|
| 1 | `components/settings/Network.tsx` (was `Mesh.tsx`, `git mv`), `SettingsDialog.tsx` tab `network`, `routes/settings.$tab.tsx` | Sections in order: rotation banner (when reported), **This machine** (OS tile, editable name `mesh_rename(self_id)`, 8-char id + copy, chips `LAN · mDNS ✓/off` and `Relay · <host> · 38 ms ✓` or `Relay · <host> · <error>`, switch "Let nearby machines find this one while pairing" → `mesh_set_nearby`), **Nearby** (`mesh_nearby`, patched by `MeshEvent.nearby`; rows with Pair…; "Only shown while this page is open."; `mesh_nearby_watch(true)` on mount, `(false)` on unmount), **Machines in this mesh** (OS tile, name + `hub`, `os_name · class`, path chip `LAN 2 ms` / `Relay 41 ms` / `offline 3h`, "Agents can message" → `mesh_set_messaging(id, on)`, off shows `agent messaging between <self> and <peer> is off on <self>`, ⋯ Rename (inline) · Unpair…; this machine: `this machine` chip, no switch), **Contacts** (muted, caption, disabled Add contact… with a `Phase 3` tag), **Agent messaging** (`get_communication(null)`: On/Off and why), **Advanced** (`<details>`, collapsed: mesh name, hub preference, sessions, replication log, Rotate vault key, pairing string + QR + paste, SSH add-node), Sandbox VMs, other meshes. `/settings/mesh` redirects to `/settings/network` |
| 2 | `components/settings/PairDialog.tsx` `PairBody`, `pairView`, `usePairFlow` | `Pair with <m>` + `os · addr`; `482 913`; "Check that <m> shows the same code"; the vault warning; [Pair] [Cancel]. States: connecting, code, waiting for the other side (Pair greyed), paired, rejected ("pressed Cancel, or the codes did not match"), expired, RPC refusal. Pair → `mesh_pair_answer(id, true)`; Cancel/close of an open ceremony → `(id, false)`. A stranger's `waiting` ceremony (not expired, not seen) opens the same dialog. No messaging checkbox (W-17, D3) |
| 3 | `components/settings/UnpairDialog.tsx` `UnpairBody`, `RotationProgress`, `RotationBanner`, `UnpairDialog` | "Unpair <m>?", the §12.9 body, "Replace these at their providers if <m> is lost" with names in mono and "Sign in again: github, google"; "<m> had no secrets."; offline / older-build / phones lines when they apply; checkbox "Also reset phone and iPad pairing" (off; ticked explains the QR re-scan). Cancel · **Unpair and rotate key** → `mesh_remove(id, reset)`; a refusal stays on the confirm step. Then "Unpaired <m>" + progress: §12.9 headline per phase, a step row, bar while minting/sweeping, per-machine lines (has the key / offline / older theuth / pending), stuck rows, failed + Retry (`vault_rotate`). `null` rotation → "No vault-key rotation reported yet" + Rotate now; a "not built yet" error reads "Vault-key rotation is not built into this engine yet (183 phase 2)" with the raw text below. The page banner is the same component, dismissable once `done` |
| 4 | phone, 430 px | Rows wrap: the path chip, switch and ⋯ drop under the name |
| — | `lib/rpc.ts` | `queryKeys.meshNearby` / `meshPair` / `vaultRotation`; listeners `nearby`, `pair` (and `paired` refetches `mesh`), `SecretsRotation` write the whole state into the cache |

**Calls made**

- **Notes file**: the brief says record calls in `183-notes.md`; the session instruction limits edits to this file, so they are here.
- **LAN chip**: `MeshStatus` has no LAN/mDNS state; the chip follows `MeshStatus.nearby` (`✓` / `off`).
- **Done time**: `RotationState` has no finish time; "rotated at" uses `started_ms`.
- **Sign-in names** show the `oauth/<ns>` namespace as stored (`github`), not a brand spelling.
- **Checkbox** shows even with `devices: 0` (V fills `devices` later; C2 always sends 0).
- **`accepted`** with this side not yet answered still shows the code and Pair (it can mean the other side accepted first).
- **Incoming pair dialog** lives on the Network page (the only place the app opens the Nearby window), not a global Attention dialog.
- **Advanced** is a native `<details>`; "Rotate vault key" errors toast, success feeds the banner.
- **Not built**: §12.8's read-only Network on a phone/iPad (a bare device still sees only "Meshes this device can reach", as before).

**Screenshots** (`/tmp/rs183u/p2/shots/`, throwaway fixture with a fake Api, deleted): `page.png`, `full.png` (whole page + banner), `pair.png`, `unpair.png`, `progress.png`, `error.png`, `notbuilt.png`, `phone.png` (430 px).

## Phase 1 (U183)

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

## Phase 2 closer (Z2) UI fixes (Settings › Network)

- **U2-MUST-1 focus** (`PairDialog.tsx`): `onOpenAutoFocus` prevents Radix's default and focuses Cancel (Done on the end screens); Pair is never the default.
- **U2-MUST-2 cancel while Connecting**: a cancelled-before-id flag; when `mesh_pair_nearby` answers, the id goes to `seen` and `mesh_pair_answer(id, false)` is sent, and the dialog stays shut (an incoming-ceremony scan is also paused while a dial is pending). Closing `starting` with an id answers false too.
- **U2-S4 Nearby lease**: the page sends `mesh_nearby_watch(true)` on mount (once the connection is `open`), every 10 s, and again each time the connection returns to `open` (`useConnection` state); `false` on unmount. Calls made: the heartbeat is silent (nothing visible resets); nothing is sent while the socket is down.
- **U2-S5 unpair**: "Unpair and rotate key" is disabled until the preview resolved (loading and error both disable); the preview query is `gcTime: 0` + `refetchOnMount: 'always'`, so every open reads fresh.
- **U2-S6 Nearby off**: `MeshStatus.nearby` false shows "Nearby is off" naming the switch instead of "No unpaired machine found"; `meshNearby` is `staleTime: 0`.
- **U2-S7 incoming dialog**: name, then `os · addr · <8-char id>` (`PairCeremony.addr`, new engine field). `queryKeys.meshPair` now holds a map by ceremony id (`putPair`), so a second ceremony cannot blank an open dialog's code.
- **U2-S8 a11y**: code box `role="group"`; the vault warning's id is the dialog's `aria-describedby` (code/waiting views); one `DialogTitle` (the visible heading) per dialog; the progressbar is named "Re-encrypting secrets"; `role="status"` sits on the changing sentence only.
- **Engine reconciliation**: `UnpairPreview.offline` / `.old_build` and `RotationState.peers[].id` are engine ids; `nameOfIn(mesh_status)` maps them to names, an unknown id shows its first 8 characters (`RotationProgress`, the headline and the preview lines all take `nameOf`). The done line reads "Vault key rotated (started <time>)": `started_ms` is a start, there is no finish time. `UnpairPreview.devices` is real: the phones line shows only when it is above 0; the "Also reset phone and iPad pairing" box stays always (it also rotates the device key).
- **LAN chip**: still follows `MeshStatus.nearby` (the engine has no other source for it).
- **Tests**: `Network.test.ts` mounts with jsdom (`test-dom.ts`, from the apps package, imported first) for focus, `usePairFlow`, the Nearby lease, the `nearby`/`pair`/`SecretsRotation` listeners and the `useRotation` retry.
- **Found live (Z2)**: the dialer's Cancel lost the focus when `Connecting…` became the code (the `starting` and `code` views render different buttons); `cancelMount` focuses Cancel/Done whenever it mounts unless the user already focused a button in that dialog. The rotation progress names the machine being unpaired by the dialog's name (its row is gone from `mesh_status` by then) and maps `removed` ids through `nameOf`.
- **Quarantine (R1)**: `MeshStatus.quarantined` rows render after the paired ones in Machines: name, "Added by <name> after it was unpaired — not paired until you confirm", `Keep paired` / `Remove` → `mesh_confirm_peer(id, keep)`; both refresh `mesh_status`. Nothing renders for an empty list.
