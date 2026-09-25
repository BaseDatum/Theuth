# 69 as built — worker D (device app: meshes list, per-mesh keys, pairing with pending id, failover UX, presence; TUI `/mesh devices`)

Branch `feature/69-devices-d`, worktree `theuth-69-d`, on top of C's contract commit `21af0ff`. Design: `69-devices.md` v0.2 §4; contract `69-contract.md` §5–§7, §9 (D's allow-list).

## Commits

### 1. `8fe8cc0` feat(desktop): meshes in the harness store, per-mesh device keys, 4-part pairing, failover chip, Settings › Mesh › Devices

`crates/desktop/ui/src/**` only.

- **`link/harnesses.ts`** — `Harness` gains `kind: 'engine' | 'mesh'` (required) and `meshName?: string`; `MeshMembers = { online, total }` and a transient `members: Record<id, MeshMembers>` beside `presence`; `setMesh(id, meshName, members)` (persists a new `harnesses` array only when the name moved); `applyMeshStatus(id, MeshStatus)` folds a hub's reply in: `meshName = mesh_name ?? self_name`, `online = 1 + reachable peers`, `total = 1 + peers` (the hub counts itself). `hydrateHarnesses` stamps `kind: 'engine'` on rows persisted before 69. `LOCAL_ROW` is an engine row. `add/remove/rename/setActive/reorder/setPresence` and the IndexedDB `state` row are unchanged in shape apart from the two new fields.
- **`link/keys.ts`** — `getIdentity(meshId?)`: `identity/device` for engine rows (unchanged, legacy `CryptoKey` row still honoured), `identity/<mesh id>` for mesh rows; one promise per key in a module `Record`. The first call for a mesh id generates and stores the key — pairing to a mesh generates it before the handshake.
- **`link/pairing.ts`** — `PairingKey.pending?: string`; `parsePairingKey` accepts 3 or 4 parts (`1.<pub‖secret>.<name>[.<pending>]`), the 4th decoded through `unb64url` + UTF-8; 5+ parts, an empty or non-base64url 4th part → `null`. `pair()` picks the per-mesh identity for a 4-part key, sends `PairHello { name, platform, pending }` (`pending` omitted for 3-part keys — `JSON.stringify` drops `undefined`, so the existing fake-harness test's exact-payload assertion still holds), stores the row with `kind: 'mesh'` and `meshName = <name part>`. `LinkOptions.identity` is now `(meshId?: string) => Promise<Identity>` (existing `() => …` seams still type-check).
- **`link/transport.ts`** — the session handshake uses `getIdentity(harness.kind === 'mesh' ? harness.id : undefined)`. Close 5 needed no code: every reason but 3 was already transient (contract §5); the comment names 4 and 5.
- **`App.tsx`** — `loadInfo` (every `open`, so after pairing and after every hub failover) calls `mesh_status` for a mesh row → `applyMeshStatus`. The "Connection lost, reconnecting…" / "Reconnected" toasts are skipped for mesh rows: failover is invisible by design (decision 6); the switcher chip carries it.
- **`components/link/HarnessSwitcher.tsx`** — `harnessLabel(h)` (`meshName` for mesh rows), `meshChip(p)` (`'reconnecting…'` for offline/connecting, `'revoked — pair again'` for refused, `null` online), `useMembersLine(id)` (`"2 of 3 engines online"`). The trigger shows the active mesh's name + chip; each mesh row shows the chip while not online, else the members line; the engine-only `offline`/`refused` trailer is unchanged for engine rows.
- **`components/link/Harnesses.tsx`** (Settings › Harnesses — the device-side list) — a mesh row's label carries `mesh · <meshName>`; its description is `relay · paired … · <chip | n of m engines online> · relay connected`.
- **`components/link/PairScreen.tsx`** — a 4-part key never offers "add as node" (`add_node` takes engine keys); success toast names the mesh. Web mode pairs exactly like mobile (no `hasLocal()` branch was needed).
- **`components/launch/StartupScreen.tsx`** — mesh wording: `Hub offline — waiting for an engine to take over`, `<mesh> revoked this device — pair again` with the Settings › Mesh › Devices / `t mesh devices add` hint.
- **`components/settings/Mesh.tsx`** — "Mesh" row (`mesh_name`) under This engine; new **Devices** section: `devices_list` (revoked rows struck through, no Revoke button), `device_rename` via the shared `Editable`, `device_revoke` behind the AlertDialog, "Pair a device" → `device_pair_start(null)` → `QRCodeSVG` + string + `Countdown` + pending id (the same inline QR block Link.tsx and the engine-pairing block use). Invalidates on `DevicePaired | DevicesChanged | LinkRevoked`; `DevicePaired` also collapses the QR.
- **`components/settings/Link.tsx`** — `LINK_EVENTS` gains `MeshRelay | DevicePaired | DevicesChanged`; a "Mesh uplink" row shows `LinkStatus.mesh` (`device_id`, connected = this engine is the hub).
- **`rpc/api.ts`** — `devices_list`, `device_revoke`, `device_rename`, `device_pair_start(name: string | null)` on `Api` and in the sorted `METHODS` list; imports `DeviceInfo`, `PairingPayload`.
- One-line disclosures inside D's tree: `link/nodes.ts` placeholder row, `components/share/GuestApp.tsx` guest row, `link/nodes.test.ts` row helper and three `pairing.test.ts` rows gained `kind: 'engine'` (the field is required).

### 2. `87f4b36` feat(tui): `/mesh devices`

`crates/tui/src/{model.rs, lib.rs}`.

- `Command::MeshDevicesList | MeshDevicesAdd(Option<String>) | MeshDevicesRevoke(String) | MeshDevicesRename(String, String)`; `UiEvent::MeshDevices(Vec<link::DeviceInfo>) | MeshDevicePairing(link::PairingPayload)`.
- `/mesh devices [list|add [name]|rm <id>|name <id> <name>]` (`Model::mesh_cmd`; anything but `devices` is usage) in the help table; dispatch in `lib.rs` calls `devices_list` / `device_pair_start` / `device_revoke` + relist / `device_rename` + relist.
- Output mirrors the CLI: one `Info` row per device (`mesh_device_line`: `<id>  <name>  <platform>  paired 2d ago · seen 3h ago | never seen | revoked`), `mesh devices: none paired` when empty; `add` pushes a QR card titled `mesh device pairing · pending <id>` that ticks in the node-invite countdown slot (one ticking invite card at a time — the existing ponytail). Headless mode prints the QR, the string and `pending <id> — expires in 5 min`, as `t mesh devices add` does.

### 3. docs: this report as `docs/design/69-as-built-D.md`.

## Deviations / decisions
1. **No "Connection lost" toast for mesh rows** (the brief says "no error toast"; the existing toast is neutral, but decision 6 says failover is invisible, so both the lost and the reconnected toasts are skipped for mesh rows; engine rows unchanged).
2. **Members = counts, not names.** `members` holds `{ online, total }`; the brief's list copy is `n of m engines online`. Per-engine names live in Settings › Mesh on the active mesh.
3. **Members refresh on every `open`** (first connect, after pairing, after each failover), not on mesh events — only the active harness subscribes to events, and a non-active mesh's count would go stale either way. The active mesh's Settings › Mesh page uses its own `mesh_status` query.
4. **`Harness.kind` is required**, as the brief writes it; eight in-tree literals (incl. tests) gained `kind: 'engine'`.
5. **Bindings**: generated with the contract's recipe (`cargo test -p rpc --features ts --quiet export_bindings`, then the `ts-desktop` copy) — no ts-rs derive was missing. The first run hit the harness's 300 s command deadline (a cold `rpc` build), the second completed. `bindings/` and `src/rpc/bindings/` are gitignored, nothing committed.
6. **`routeTree.gen.ts`** untouched: no route was added (Devices lives under the existing `settings/$tab` `mesh` tab).
7. **`Connections.tsx`** untouched: it is the accounts/gateways page; the device-side harness list the brief calls "Connections/Link settings" is `components/link/Harnesses.tsx` (Settings › Harnesses), which is where the mesh rows are.
8. To type-check, `crates/desktop/ui/node_modules` was symlinked to the main worktree's install and removed again after; the worktree has no install of its own.

## Not built
- A per-engine member list on the device (names/reachable per peer) — counts only, see deviation 2.
- A dedicated mesh picker beyond the existing switcher rows.

## Tests written (not run, per the brief)
- `link/pairing.test.ts`: existing parser test asserts `pending` is `undefined` on 3 parts; new `parsePairingKey: a 4th part is the pending id…` — the contract's example string (`d29yaw.QWJDLV8xMjM` → name `work`, pending `AbC-_123`), 5 parts → `null`, non-base64url 4th part → `null`, empty 4th part → `null`.
- `link/harnesses.test.ts`: `hydrate: rows persisted before 69 get kind engine; a mesh row keeps its kind and name` — an old row without `kind` hydrates as `engine`, a stored mesh row keeps `kind: 'mesh'` + `meshName`, `active` survives, `setMesh` renames and stores `members`.
- `tui::model::tests::mesh_devices_command_mirrors_the_cli`: every `/mesh devices` verb maps to its `Command`, `/mesh status` is usage with nothing sent, `mesh_device_line` for a never-seen and a revoked row, the empty-list `Info`, and the pairing card (title ends `pending AbC-_123`, `invite` slot set).

## Verification (exact results)
- `bunx tsc --noEmit` (crates/desktop/ui): exit 1, 85 lines; **0 in any file D touched** (`src/link/*`, `App.tsx`, `components/link/*`, `components/launch/*`, `components/settings/{Mesh,Link}.tsx`, `components/share/GuestApp.tsx`, `rpc/api.ts`). 16 lines are the pre-existing `@openuidev` errors (`AppHost.tsx`, `OpenUIFence.tsx`, `lifecycle.ts`); the rest are pre-existing bindings drift in `components/tasks/*`, `components/agents/AgentsView.tsx`, `lib/attach.test.ts` (`AgentStatus.invalid`, `Owner.system`, `Task.blocks`, `Task.next_wake_after_ms`) — none of those files or types are D's.
- `cargo check -p tui --tests`: first run (before commit 2) `error[E0308]: mismatched types --> crates/tui/src/lib.rs:1516:24` (an early `return` inside the headless `print` match); fixed to a guarded arm; second run `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1m 30s`, no warnings. Two runs for one commit, disclosed.
- `bun test` not run (brief).

DONE D
