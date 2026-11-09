# 184 — as built, slice U (UI and guest page)

Branch `feat/184-ui` from W0 f170554c. Files under `crates/desktop/ui/src/`. Contract: `184-contract.md` §7; no "Contract changes after W0" existed in `theuth-184/docs/design/184-notes.md` at the end of this slice, and its bindings were identical to the W0 copy.

## What was built

| Item | Where | What |
|---|---|---|
| Provider (§4.7, §12.5) | `lib/doc/provider.ts` | Every acked local update stays in `#unconfirmed` (memory, 1 MiB cap, oldest dropped with a warning). On each `DocOpen` with a non-empty `sv`, entries the server vector covers are dropped; the rest go to the front of the queue, and the existing `#batch()` loop resends them in merged batches under `MAX_UPDATE_B64`. Then the catch-all `encodeStateAsUpdate(doc, sv)` is sent when it carries anything and fits under the cap. `Closed{slow, hub_moved, server_moved, NotHub:…}` sets `connecting` and re-syncs; other reasons close. `provider.server` / `provider.dual` come from `DocOpen` and `DocEvent::Server`, which is also forwarded to `onEvent`. |
| Machine names | `components/notes/machine.ts` (new) | `inMesh(mesh)` (= `peers` non-empty), `machineName(mesh, id)` (unknown id → first 8 chars). Shared by the chip, Move to, Share and the cells. |
| ServerChip (§12.1) | `components/notes/ServerChip.tsx` (new), `editor/DocumentPage.tsx` | `serverView(...)` → `On <home or server>` (green), `On <home> · offline` + banner, or `dual` + the "Also open on" banner; tooltip per §12.1. Notes, boards and meeting notes in a mesh, with a known server, while the document is open. Re-read on `Server` events and on every status change. The chip sits before Share; the banner tops the editor column. |
| Move to ▸ (§12.4, §4.8) | `components/notes/MoveNote.tsx` (new), `notes/NotePage.tsx` | `moveRefusal(kind, homes_on)` and `moveRows(mesh, home, pending)`. Rows: `lives here`, `offline Nm`, `Moving…` while the call runs (all rows disabled). Calls `doc_move_home(doc, to)`; toasts `Moved to <machine>` or the error verbatim. |
| Share (§12.2) | `components/share/ShareDialog.tsx` | `shareCopy(mesh, home, via, role, unavailable)`: title `Share "<title>"` and the case copy in a mesh; today's copy outside one. `mesh_link_unavailable` switches the copy and shows `Retry` (calls `doc_link_create` again) instead of a toast. |
| Guest (§7.1, §12.6, W-k) | `link/guest.ts`, `components/share/GuestApp.tsx` | v2 `2.<home>.<mesh>.<doc>.<secret>` → `engineIds = [home, mesh]`, v1 unchanged. `dialRotation` dials `ids[0]`, rotates on `HOME_DIAL_MS`, wraps; an open stops it, a drop restarts it. `doc_join` runs per connection; `joinBusy` retries `busy` once after 1 s (not a refusal); `Closed{not_member}` re-joins once. Store key stays `engineIds[0]`. |
| Cells (§6.5, §12.3, W-p) | `components/cells/{NoteCell.tsx, store.ts, Cell.tsx}` | The store reads `cell_latest` with each doc-scope refresh and on `CellLatestChanged{doc}`. A foreign row (engine not `''`, not self) newer than the own run renders `ran on <machine> · <age> · exit <n>` with no terminal, Attach, Stop or History. Run button: `Run on this Mac` (`LOCAL_NAME` of the shell, so `this PC` / `this computer` elsewhere) when the UI's engine is the local one, else `Run on <self_name>`; only in a mesh and without `label:`. `MemberAck("?")` opens `UnsureRun` with the §12.3 copy; Run re-presses with `member_ack = "?"`. |
| Single engine (§16) | all of the above | No peers: no chip, no banner, no Move to, today's Share copy, today's Run label, foreign rows ignored. Each is asserted in a test. |
| Fixture | `dev/notes-184-fixture.{html,tsx}` (new), `dev/cells-notes-fixture.tsx` (`?mesh`) | Views `p1 p2 p3 dual p4 p4mesh p4un move board single`; fake API, no engine. |

## Calls (U)

1. **Move to is its own bar button with a dropdown**, not a ⋯ submenu: `NotePage` has no ⋯ menu, only icon actions. Refused (board, meeting note, homes off): the button is disabled inside a `span title=<reason>`, so the tooltip still shows. Task threads and other kinds: no button.
2. **Unhomed note's current home** for Move to and the chip: `meta.home ?? provider.server` (the holder).
3. **Share copy before a link exists**: `via` is unknown, so a homed note reads as `via: home`; after `LinkCreated` the copy follows `via`. The viewer role switches `can edit` → `can view` in every in-mesh case.
4. **Chip precedence**: `dual` wins over offline (green chip, "Also open on" banner).
5. **Coverage check of `#unconfirmed`** decodes the update's structs (`Y.decodeUpdate`), because `Y.encodeStateVectorFromUpdate` is empty for an update that does not start at clock 0. The catch-all is built against the server vector advanced by what this sync just sent, so queued edits are not sent twice. Resent entries are not acked in IndexedDB a second time (its byte count would drift).
6. **Catch-all over the cap is skipped**: the `#unconfirmed` list already holds every lost update; only a missed one past 48 KiB would wait for the next re-open.
7. **`MemberAck("?")` detection**: RPC error `data.reason == "member_ack"` and a message containing ` by ? ` (the engine's text is `changed by {pub8} since your last run — review it first`; `data` carries no pub8). The confirm names `machineName(provider.server)`, else `the hub`. It is an inline notice, not a modal: a modal inside the note's node view loops ProseMirror (see `LineConfirm`).
8. **Age text** for `ran on` is local (`just now`, `4 min ago`, `1 h ago`, `2 d ago`): the existing `ago()` gives `4m`, not the design's words.
9. **Foreign result** renders the row's text with `AnsiText` (images and HTML are not in `cell_latest`).
10. **Guest rotation** re-joins only on a re-dial (a client's own auto-reconnect on the same id keeps the join, as before). A second `busy` ends with today's generic message.

## Contract assumptions (for the closer)

- `DocOpen.{server, dual, sv}`, `DocEvent::Server`, `LinkCreated.via`, `CellLatest.engine`, `MeshStatus.docs_homes_on`, `CellLatestChanged{doc}` exactly as the W0 bindings.
- `doc_link_create`'s refusal text contains `mesh_link_unavailable` (E-link `MESH_LINK_UNAVAILABLE`).
- `doc_join`'s retryable error text contains `busy` (`share::BUSY`); a server move's member refusal closes with a reason containing `not_member`.

## Proof

- `tsc -b --noEmit`: exit 0.
- Touched tests, twice: `provider.test.ts`, `guest.test.ts`, `NoteCell.test.ts`, `store.test.ts`, `Cell.test.ts`, `ServerChip.test.ts`, `MoveNote.test.ts`, `ShareDialog.test.ts`, `DocumentPage.test.ts`: 9 files, 88 tests, both runs green.
- Full suite (`BUILD_SLOTS=1 BUILD_LABEL=rs184u … bunx vitest run`): 319 files, 2119 tests passed, 0 failed. The first run had found a `▶` in a fixture comment (`no-emoji.test.ts`), which is now fixed.
- Screenshots (headless Chrome, vite dev :5183, the fixture's fake API), `/tmp/rs184u/shots/`: `panel1-on-xeus-ran-on.png`, `panel2-after-run-here.png`, `panel3-xeus-offline-banner.png`, `panel4-share-via-home.png`, `share-via-mesh.png`, `share-mesh-link-unavailable-retry.png`, `move-to-menu-refused-rows.png`, `move-to-progress.png`, `move-to-board-refused.png`, `dual-also-open-on.png`, `single-engine-nothing-new.png`, `guest-v2-fallback-connecting.png`. The fixture runs in a browser, so the button reads `Run on this computer` there (the shell's kind); the desktop reads `this Mac`.
- Guest fallback, observed over CDP on a v2 link: socket to `/h/<home>` at 83 ms, `/h/<mesh>` at 5090 ms, back to `/h/<home>` at 10089 ms (`guest-v2-fallback-ws.txt`).
