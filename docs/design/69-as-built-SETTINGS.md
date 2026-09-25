# 69 as-built — SETTINGS: Mesh · Link · nodes vocabulary

Branch `chore/settings-mesh-link-nodes`, worktree `theuth-settings`.

## Commits

1. `63c1b1e` `chore(ui,tui): settings = Computer · Mesh · Link; nodes/mesh/device vocabulary (engine stays an identifier)` — 37 files, +544/−668.
2. `docs(69): as-built SETTINGS` — this file at `docs/design/69-as-built-SETTINGS.md`.

## What was built

### Settings tab list (`components/settings/SettingsDialog.tsx`)
`… Skills · Agents · Mesh · Link · Connections · Secrets · Channels · Shortcuts`. The `engines` and `nodes` tabs are gone (also from `chat/slash.ts` `SettingsTab`). The `useHasNodes` tab filter moved into Mesh (see below); the dialog only filters Labs now. `components/settings/Nodes.tsx` and `components/link/Engines.tsx` deleted — nothing else imported them.

### Mesh tab (`components/settings/Mesh.tsx`) — final section list
On a node (desktop / dev browser with a local daemon):
1. **This node** — name (`MeshStatus.self_name`) with a `hub` badge when this node holds the lease, id short + copy; *Mesh* (`mesh_name`); *Prefer this node as hub* (`mesh_set_hub`, Select prefer / eligible / never, description shows the current hub, reason and since); *Sessions* (`sessions_status`: owned here / elsewhere); *Replication log* (seq, last snapshot). When the active node is not the local one the section opens with "Managing the mesh of <name>".
2. **Nodes in this mesh** — `MeshStatus.peers`: dot, name, `hub` badge, id short, online/offline, hub pref, last seen, first address; Remove = `mesh_remove`.
3. **Add a node** — "Show pairing string" (`mesh_pairing_string`, QR + copy) and paste (`mesh_add`); then *On another computer over SSH*: paste the key printed by `t --install-service` → `add_node`, followed by the nodes added that way (rename `rename_node`, Remove `remove_node`, bus-event refresh). The SSH part renders only where the active node answers `list_nodes` (`useHasNodes`, as the old Nodes tab did). `invite_node` and the relay-invite QR popover are gone from the UI (the RPC stays; `link/nodes.ts` auto-pair still calls it).
4. **Other meshes this app can reach** — the former Engines tab: rows from `useEngines` with presence dot, local rename, `mesh · <name>` / `node` tag, `via <parent>`, `n of m nodes online` or the failover chip, move up/down, Forget. Action "Pair with a mesh…" → `/link`.

On a bare device (web/phone, `!hasLocal()`): only section 4, titled **Meshes this device can reach**, empty state pointing to Settings › Link.

### Link tab (`components/settings/Link.tsx`) — final section list
On a node:
1. **Devices paired with this mesh** — `devices_list`, "Pair a device" = `device_pair_start` (4-part string as QR + copy, countdown, `pending <id>`), inline rename `device_rename`, Revoke `device_revoke` (revoked rows greyed/struck). Refreshes on `DevicePaired` / `DevicesChanged` / `LinkRevoked`; the QR collapses on `DevicePaired`. Copy names the mesh from `mesh_status` (`mesh_name ?? self_name`).
2. **Relay** — `link_enable` switch in the header, relay URL editable, connected/connecting/off dot, *Node name* (`link_rename`), *This node* (`harness_id` + copy), *This mesh* (`LinkStatus.mesh.device_id` + copy, "this node is the hub …" / "another node holds the hub lease").
3. **Legacy devices** — `LinkStatus.devices`, read-only rows (name, platform, paired, online/seen, "· re-pair with the mesh"), Revoke = `link_revoke_device`. Rendered only when there is at least one. `link_pair` / `link_cancel_pair` / `link_rename_device` are no longer called from the UI.
Footer paragraph (relay privacy note) kept, now says node/computer.

On a bare device: one section **Pair with a mesh** with a "Pair…" button → `/link` (the existing `PairScreen`).

`Countdown` and `Editable` stay exported from `Link.tsx` (Mesh.tsx uses `Editable`).

### Vocabulary sweep (strings only; identifiers untouched)
- Switcher (`components/link/EngineSwitcher.tsx`): fallback name `Node`, `n of m nodes online`, menu items "Pair with a mesh…" and "Manage…" → Settings › Mesh.
- `PairScreen.tsx`: heading "Pair with a mesh", "the active node", relay note, choice buttons "as a node of <parent>" / "as a node this phone reaches directly".
- `launch/StartupScreen.tsx`: "Node offline", "waiting for a node to take over", revoked/offline copy (nodes of the mesh; new pairing code under Settings › Link), "Forget this node", "Pair with a mesh…", "Switch node".
- Everything else that said engine to a person: `apps/AppHost.tsx`, `apps/lifecycle.ts`, `chat/OrphansBanner.tsx`, `chat/Peers.tsx`, `chat/SessionMenu.tsx`, `chat/slash.ts` (descs + login toast), `knowledge/Document.tsx`, `meeting/ControlCenter.tsx` (Settings › Node › Documents), `settings/Communication.tsx` (node-wide), `settings/Connections.tsx`, `settings/General.tsx` (section "Engine" → "Node"), `settings/Loop.tsx`, `settings/Meetings.tsx`, `sidebar/SearchDialog.tsx`, `sidebar/SessionItem.tsx`, `tasks/Board.tsx` ("Node" filter, "Any node"), `tasks/NewTaskSheet.tsx`, `tasks/TaskDetail.tsx`, `agents/AgentDetail.tsx`, `lib/injected.ts` (notice descriptions), `lib/labs.ts`, `lib/private.ts`, `link/frames.ts` (`CLOSE 4` = "node gone"), `link/pairing.ts` ("node offline"), `routes/meetings.new.tsx`.
- TUI (`crates/tui/src`): `/link` title `… n node name …`, `Prompt::LinkName` "link  node name", `/take` and `/mesh devices` help lines ("on this node", "any node serves them"), attached-mode notes "node process gone — starting one…" / "node process started here (pid N)" / "node process failed to start". `/mesh devices` rows had no engine word.

### Grep gate
`git grep -n -i -P "\bengine(s)?\b" -- crates/desktop/ui/src crates/tui/src ':!*.test.*' ':!*/rpc/bindings/*'` → 497 lines. (Note: macOS git grep `-E` does not know `\b`; use `-P`.) All are identifiers, imports, comments or doc-comments except these six, none of which a user sees:
- `components/settings/Mesh.tsx` — `<MeshLine engine={h} …>` prop name.
- `components/tasks/NewTaskSheet.tsx:117-118` — DOM ids `nt-engines` (datalist wiring).
- `link/pairing.ts:189` — `kind: … 'engine' : 'mesh'` discriminant.
- `crates/tui/src/lib.rs:232` — `tracing::warn!(…, "engine event stream")` log target.
Remaining identifier families: `Engine`/`useEngines`/`engines.ts`/`engine-scope`, `EngineSwitcher`, `ENGINE_NOTICES`, `kind: 'engine'`, `Entry.kind === 'engine'`, `t.checkout.engine`, `?engine=` task search param, `harness_id`.

## Deviations

- **"This node" name is not editable.** The brief wants `mesh.name` editable; there is no RPC that writes `mesh.name` (only `mesh_set_hub`, `link_rename` for the Link display name) and Rust behaviour is out of scope. The row shows `self_name` with the hint "`mesh.name` in this node's config, else its hostname". `link_rename` stays in Link › Relay as *Node name* because that is what legacy devices show.
- **Hub preference stays a three-way Select** (prefer / eligible / never) labelled *Prefer this node as hub*; a Switch would drop `never`.
- **Sessions / Replication log** (68 §10, previously their own Mesh section) folded into *This node* as two rows rather than dropped.
- **SSH-added nodes list** (rename/remove of `list_nodes` rows) kept under *Add a node › over SSH* so the old Nodes tab's management is not lost; it is gated on `useHasNodes()` like the old tab was.
- **Bare device** is `!hasLocal()` (no local daemon). A desktop that switched to a remote node keeps the full node-side Mesh/Link with a "Managing … of <name>" line, as before.
- **Runtime-as-actor sentences** ("The engine asked the model…", "Show engine nudges", "the engine picks up a reply") say **theuth** rather than node — "the node asked the model" reads wrong; node is used wherever the machine/daemon is meant. Brand appears only in prose, never identifiers.
- `StartupScreen` pairing button says "Pair with a mesh…" (matches the switcher and the Link tab entry); a direct node key still works through the same screen.
- One-line edits outside the settings/link files, all string-only, listed above under the sweep.

## Not built

- No screenshot (not possible); section lists above are the description.
- No Rust change; `invite_node`, `link_pair`, `link_cancel_pair`, `link_rename_device` RPCs remain, unused by the desktop UI.

## Tests

No new tests: every change is copy, layout or moved JSX; no existing test asserted on a changed string or on the removed tabs (`Nodes.tsx`, `Engines.tsx`, `SettingsDialog` had none). `lib/owner.test.ts` still tests `engineLabel` (identifier, unchanged).

## Verification (all run in the worktree)

- `just ts-desktop` — bindings exported (343 files; `export_bindings` 5 passed).
- `cd crates/desktop/ui && bun install --frozen-lockfile` and `(cd packages/apps && bun install --frozen-lockfile)` — ok (`packages/apps` is at the repo root, not under `crates/desktop/ui`).
- `bun run build` — exit 0.
- `bunx tsc --noEmit` — exit 0, 0 errors.
- `bunx vitest run` — 65 files, **389 passed**, 0 failed.
- `cargo check -p tui --tests` — `Finished dev profile … in 11m 14s`, no warnings printed for `tui`.

DONE SETTINGS
