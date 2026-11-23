# 210 as built — UI-M2 (Settings › Sharing names and machines, machine names on strip/cards; test env fixes)

Design: `docs/design/210-public-share.md` rev 8 (§5.2, §5.7 D22, §7.2 RPC table, §7.4 items 1, 3, 4). Seams: UI1, UI2,
CONTRACT2 as-built notes. Owner: `crates/desktop/**`. No Rust change in this phase.

## Files

| path | what |
|---|---|
| `ui/src/components/public-share/wire.ts` | `NAME_METHODS`, hand type `HeldName`, `NameAvailability` (= `NameCheck` + `reclaim_after_ms`), `NotOnEngine`, `isNotOnEngine(e)`, `namesApi.{list,reserve,release,primary,rotate,transfer}` (raw calls by name), `checkName(name)`; `reserveName` removed |
| `ui/src/store/public-shares.ts` | `Machines {self, hub, names}`, store `machines`/`setMachines`; `machineLabel`, `machineBreakdown`, `servedBy`; `stripLabel(sum, machines?)` |
| `ui/src/components/public-share/actions.ts` | `loadMachines()` from `mesh_status` (skipped when discovery lacks it) |
| `ui/src/components/public-share/PublicShareHost.tsx` | loads machines on every open connection, again when a `share.changed` names an unknown engine |
| `ui/src/components/public-share/PublicShareStrip.tsx` | `… 2 live · 1 pending (1 on this machine, 2 on Studio)` when any active share runs elsewhere |
| `ui/src/components/public-share/SharePublishCard.tsx` | Source row: `localhost:5173 on Studio` for another machine's share |
| `ui/src/components/settings/Sharing.tsx` | `NameSection` (rewritten), `HeldNameRow`, `ConfirmLine`, `MachinesSection`, `d22Text`, `nameProblem`, `availabilityText`, `heldStateText`; live-share rows say `on <machine>`; the expiry `<select>` is `text-base md:text-sm` (mobile-zoom rule) |
| tests | `settings/Sharing.test.ts` (+8 tests), `PublicShareStrip.test.ts` (+1) |
| test env fixes | `ApprovalRequested.human_only: false` in `chat/ApprovalCard.test.ts`, `chat/rows.test.ts`, `sidebar/SessionItem.test.ts`, `lib/owner.test.ts`, `store/approvals.test.ts`, `store/reduce.test.ts`; `settings/test-dom.ts` installs an in-memory `localStorage`/`sessionStorage` when absent; `settings/Contacts.test.ts` imports `test-dom` first; `chat/ElementFormCard.test.ts` mocks `useMesh` (the card gained `useMesh` in 209 P5 without a `QueryClientProvider` in its test) |

## Public API (UI)

- `namesApi.list(): Promise<HeldName[]>` → `share_names_list []`
- `namesApi.reserve(name)` → `share_names_reserve [name]` (the engine raises the owner-only approval row; the button reads "Waiting for approval…")
- `namesApi.release(name)`, `namesApi.primary(name)`, `namesApi.rotate(name)` → `share_names_{release,primary,rotate} [name]` (a UI confirmation before release and rotate, as §7.2 says)
- `namesApi.transfer(name, toMesh)` → `share_names_transfer [name, to_mesh]`; `toMesh` is the pasted string, sent as typed (the engine parses a mesh id or a ticket)
- `checkName(name)` → `share_names_check [name]`, raw, so `reclaim_after_ms` arrives even though the generated `NameCheck` lacks it
- A method the engine lacks (discovery `UnsupportedCapability` or JSON-RPC -32601) becomes `NotOnEngine`; the UI says "not available on this engine" (list: "Your names: not available on this engine."; row action: "Not available on this engine."; reserve: "Reserving a name is not available on this engine.").
- `HeldName` (hand type, what the UI expects `share_names_list` to return): `{name, domain, edge?, kind: 'reserved'|'random', primary, state?, queue_position?, queue_eta_s?, transfer_to?, transfer_until_ms?}`. The source is the replicated `share_names` row (§5.8) joined with the edge's `NameInfo` (§5.2, CONTRACT2 `transfer_to`/`transfer_until_ms`, `QueuePos`).

## Behaviour

- **Reserve**: the §5.2 rule runs client-side as you type (3–32 chars, `[a-z0-9-]`, no leading/trailing `-`, no `xn--`, the built-in reserved words); a well-formed name is checked with `share_names_check` after a 350 ms debounce. Free → green, Reserve enabled. `cooldown` (or any `reclaim_after_ms`) → "sky.tun.zip was released recently: free again on <date>"; without the date, "…is cooling down". Other reasons are spelled out.
- **Held names**: `fqdn`, a "primary" badge, "Reserved/Random · <state>" (`Ready`, `Getting a certificate…`, `Waiting for a certificate slot — position N, about T`, `Offered to mesh xxxxxxxx… until <date>`, banned, released). Buttons: Make primary (not on the primary), Rotate key (confirm), Transfer… (reserved names only; paste field + "Offer transfer"), Release (confirm stating that shares stop on every machine and the ≥ 30-day cooldown).
- **Your machines**: from `mesh_status` (`self_id/self_name`, `peers[].{id,name}`, `lease.holder` = hub); each machine with "Serves <host>[/path], …" from the active shares' `engine`, or "Nothing public"; the hub is badged; an engine the mesh no longer lists still shows when it holds an active share.
- **D22**: "When a machine is removed from your mesh, the hub (Studio) contacts tun.zip to revoke it there, even if sharing is off on the hub." (the hub's name when known).
- **Shares on other machines**: strip breakdown, card "on <machine>", Settings live-share rows "on <machine>"; Stop on any of them is `share_stop(id)` (the engine forwards to the hosting engine, §7.4).

## Deviations

- **`share_names_list` is not in the design's §7.2 table**; Settings needs the held names. Assumed name and shape above; CLIENT-M2 should confirm or rename (one place: `wire.ts`).
- **Hand types, raw calls**: the bindings lack the M2 methods. `TODO(210-M2, CLIENT-M2)` in `wire.ts`: switch `namesApi` to the generated `Api` (and `NameCheck.reclaim_after_ms`) after `just ts-desktop`.
- **Transfer target**: the design says `{name, to_mesh}`. The UI takes "mesh id or ticket" and sends the string unparsed; if the engine wants only the id, it must parse a ticket, or the UI needs a parser (none in `share-proto`'s TS surface).
- **Machine names come from `mesh_status`**, not from the share plugin: no share RPC lists engines.
- Settings shows no rekey states, vault-rotation warning, clock-off machine name or "Can't renew sharing: <hub> unreachable": no RPC or event carries them yet.

## Not built

- `TODO(210-M2)`: rekey state lines (§5.7 table: `waiting_vault` after 10 min, `contested`), "Can't renew sharing: <hub> unreachable since …", per-machine clock warning, `share_extend` (Extend on card), `share_settings_set {defaults}`, removal-duty attention items. They need engine RPC/events (CLIENT-M2).
- `share_edges` listing (which edges the mesh is registered at) in Settings: no RPC.
- Others' visitor counts (P2 per design).
- **The M2 exit tests** (three-engine family test, P0-6 races) are engine/edge tests in `crates/share` / `crates/share-edge`, outside this phase's files: not built here.

## Doubts

- `HeldName.state` values are taken from §5.2 (`allocated → queued → slotted → valid`, `released`, `banned`); an engine reporting another vocabulary falls back to "No certificate yet".
- The release confirmation shows "today + 30 days" as the earliest reclaim; the true `reclaim_after_ms` is `max(released + 30 d, cert_not_after)` and is only known after release.
- `loadMachines` re-reads `mesh_status` only on connect and when an unknown engine appears; a renamed peer shows its old name until reconnect.

## Verification

Run in `crates/desktop/ui` unless noted:

- `bun run check` → 0 errors (the 15 `human_only` errors are gone).
- `bunx vitest run` → 413 files; 3018 tests: all pass except, while cargo was compiling alongside, the wall-clock bound in `src/lib/costs/cube.test.ts` ("perf bound: 100 k cells … under 50 ms"); run alone, `bunx vitest run src/lib/costs/cube.test.ts` → 12 passed. That test is not touched here and is load-sensitive.
- `bunx vitest run src/components/settings/Contacts.test.ts` → 48 passed (was: suite failed to load, `localStorage` undefined on Node 26).
- `bunx vitest run src/components/settings/Sharing.test.ts src/components/public-share src/store/public-shares.test.ts` → all pass.
- `cargo clippy -p desktop --all-targets --no-deps -- -D warnings` and `cargo test -p desktop` (under `/tmp/theuth-build.lock`): see the result appended below.

No npm or crate dependency added. `Cargo.lock` shows as modified in the tree; not from this phase (other builders' crates).
