# 68 — as built, worker U (desktop UI: owner chips, Continue here, owner-offline card, orphans; TUI `/take`)

*Branch `feature/68-sessions-u`, 2026-09-25. Built against the 68 contract (`68-contract.md`): `list_sessions` rows carry `owner/epoch/here`, `session_take/session_owner/session_orphans` exist as stubs, `SessionOwnerChanged` is on the event stream. Bindings regenerated with `just ts-desktop` (gitignored, not committed).*

## Per commit

### 1 `feat(ui): engine chip, Continue here, /take; sessions_status RPC stub`
- `src/lib/owner.ts` (new): `engineLabel(owner, peers)` — the peer's mesh name from `mesh_status().peers`, else the id's first 8 chars, `?` when empty; `useEngineLabel`; `ownerUnreachable(e)` reads `OwnerUnreachable{owner, last_seen}` out of an `RpcError`'s `data` (serde external form `{"OwnerUnreachable": {…}}` or a flat object tagged `kind`/`type`); `lastSeenText` (RFC 3339 or ms → `3m ago` / `never`); `ownerOfflineText`.
- `src/lib/rpc.ts`: `queryKeys.sessionOrphans(id)`, `queryKeys.sessionsStatus`; `withOwner(rows, e)` patches one cached `list_sessions` row; `useInvalidateOn` handles `SessionOwnerChanged` — patches the cache at once (chips, header and menus follow live), invalidates the session's orphans and `sessions_status`, then the debounced refetch.
- `src/rpc/api.ts`: `session_take(session, 'user'|'orphan'|'write')`, `session_owner`, `session_orphans`, `sessions_status` + `METHODS`.
- `components/sidebar/SessionItem.tsx`: `EngineChip` (muted mesh name, `title` = owner id + epoch) after the title on rows with `here == false`; menu item **Continue here** → `session_take(id, 'user')`, disabled with a tooltip when `here`. `SessionList.tsx` passes `owner/epoch/here`.
- `components/chat/Header.tsx`: `owner` header item — the chip in the bar, an `Owner · name · epoch N` row in the compact menu; the header now always reads the `list_sessions` row of the open session (was: only until the live title arrived).
- `components/chat/SessionMenu.tsx`: **Continue here** (disabled + tooltip when `here`) running `/take`; `components/chat/slash.ts`: `/take` → `session_take(session, 'user')`, toast `session continues here (epoch N)`.
- Rust (disclosed, outside the UI allow-list, sanctioned by brief item 5): `crates/sessionsync/src/types.rs` `SessionsStatus { engine, here, elsewhere, log_seq, last_snapshot: Option<String> }` (ts export); `crates/rpc/src/api.rs` `sessions_status()` — engine from `Store::engine()`, counts from `ListSessions`, `log_seq` from `Store::log_seq()`, `last_snapshot: None` with a `ponytail:` note for worker L; `crates/rpc/src/export.rs` exports the type.

### 2 `feat(ui): owner-offline card — continue here and resend`
- `components/chat/OwnerOfflineCard.tsx` (new): "Owned by <engine>, last seen <ago>. Continue here now? (turns not yet shipped from <engine> stay there)" with **Wait** (dismiss, draft kept) and **Continue here** (`session_take(id, 'user')` then `send_input` with the same body and files).
- `Composer.tsx`: the send's `body/files` are remembered before the RPC; a catch whose error parses as `OwnerUnreachable` (and there is an open session) shows the card instead of the toast.

### 3 `feat(ui): orphans banner and read-only ranges sheet`
- `components/chat/OrphansBanner.tsx` (new), mounted in `routes/c.$session.tsx` after `PrivateStrip`: `session_orphans(id)` under `queryKeys.sessionOrphans`; when non-empty, "N turns from <time> were not delivered from <engine> — view"; **view** opens a `Sheet` table `from · epoch · turns` per range. Read-only; nothing is merged. `orphanCount`/`orphansText` pure and tested.

### 4 `feat(ui): Settings › Mesh › Sessions block; fixtures gain owner/epoch/here`
- `components/settings/Mesh.tsx`: `Sessions` section between Engines and Pair — engine id (from `sessions_status().engine`, falling back to `mesh_status().self_id`) and name (`info().name`), `owned here · N elsewhere`, replication log head `seq N`, `last snapshot <age> ago` / `none yet`.
- `SessionList.test.ts`, `lib/attach.test.ts`: `SessionSummary` fixtures gain `owner/epoch/here`. `Composer.tsx` `create_session` literal completed with the 67 §8 fields (`effort`, `parent_session`, `parent_engine`, `harness`) — pre-existing tsc error in a file I touched.

### 5 `feat(tui): /sessions owner chip column, /take, owner-offline sentence`
- `model.rs`: `Command::TakeSession(SessionId)`; `/take` (needs an open session; `/help` line); `engine_chip(owner)` (first 8 chars, `?` when empty); `session_row(s, depth, width, current)` — the picker row moved out of `ui.rs` and gained a `{chip:<8}` column after the model (blank when `here`); `owner_offline_line(owner, last_seen)` — the same sentence with `— /take` as the action; `owner_unreachable(data)` — the `OwnerUnreachable` parser over the JSON-RPC error `data` (external or `kind`/`type`-tagged form; `last_seen` RFC 3339 or ms → `ago`, else `never`). `AgentEvent::SessionOwnerChanged` updates the open picker's rows in place and prints one info line for the open session.
- `lib.rs`: `Command::TakeSession` → `api.session_take(id, "user")` → `Info("taking session here (epoch N)")`; `reason()` — the one place every failed call's text comes from — turns an `OwnerUnreachable` refusal into `owner_offline_line`, so `/send`, steer, regenerate … all print it; the line printer's `/sessions` output gains the chip column.
- `ui.rs`: the Sessions menu renders `session_row`.

## Deviations
1. **`OwnerUnreachable` wire shape is not in the contract** (worker F defines it). Both clients accept the error's `data` as serde's external enum form `{"OwnerUnreachable": {"owner", "last_seen"}}` or a flat object with `kind`/`type: "OwnerUnreachable"`; `last_seen` may be RFC 3339 text, ms since the epoch, or null (`never`). Any other shape falls back to the plain message toast / `error:` line. F: emit one of those and it lights up with no UI change.
2. **`sessions_status`** was not in the contract; added as brief item 5 allows. It reports `log_seq` (head of `session_log`), not log bytes — the store exposes no size query and `store.rs` is O's. `last_snapshot` is `None` until L records one (`ponytail:` note at the call site).
3. **Orphan "turns"** are `session_log` rows (`to_seq - from_seq + 1`), one per committed write, not model turns; the copy says "turns" as briefed. `<engine>` in the banner is this engine's `info().name` — orphans are by construction rows this engine wrote.
4. **Resend after Continue here** is `send_input` even when the refused call was a `steer` (the actor restarts here, so the running state is unknown); the locally queued chip of a refused steer is left as the existing failure path leaves it.
5. **Engine names**: the TUI has no mesh roster, so its chip is the id's first 8 chars; the desktop chip uses `mesh_status().peers[].name` and falls back to the same. `here` sessions show no chip anywhere (brief item 1).
6. **Continue here lives in two menus**: the sidebar row's `…` menu (where the chip is) and the open session's header menu (`/take`). Both disabled with a tooltip when `here`.
7. `SessionOrphan.from_seq/to_seq` export as `bigint` (u64 without `ts(type = "number")` in the contract type); the UI converts with `Number()` rather than touching the contract.
8. **`SessionOwnerChanged` is not folded into the zustand `SessionSlice`**: the session list is react-query state, so the event patches that cache (`withOwner`) and every reader (sidebar, header, menu) follows from one source.
9. Files touched outside the allow-list, all disclosed: `crates/sessionsync/src/types.rs` (one struct), `crates/rpc/src/api.rs` (one method: declaration, body, `use`), `crates/rpc/src/export.rs` (one line).
10. The pre-existing tsc baseline on this branch is **69**, not 60: the 60 `@openuidev`/`packages/apps` errors plus 9 from other designs' contract drift (`tasks` `Owner::System`/`Task.blocks`/`Checkout.orphaned` in `components/tasks/*`, `AgentStatus::Invalid` in `agents/AgentsView.tsx`, `Meta.owner/visibility` in `lib/attach.test.ts:67`). None in files this work touched; not fixed (other workers' contracts).
11. `bun install --frozen-lockfile` was run once in `crates/desktop/ui` so tsc/vitest could run; `just ts-desktop` twice (once before `SessionsStatus` existed).
12. No route was added, so `routeTree.gen.ts` is untouched.

## Not built
- Log byte size and snapshot time (no store/L API yet — see deviation 2). Live event streaming from a remote owner (F's `Events` stream) needs nothing here: events arrive on the local bus like any other.

## Tests written
- `src/lib/owner.test.ts` — engine label precedence, `OwnerUnreachable` parsing in both forms and rejection of everything else, `lastSeenText`, the offline sentence, `withOwner` patching only the named row (run: 4 passed).
- `src/components/chat/OrphansBanner.test.ts` — row counting across ranges, singular/plural, oldest-range date (run: 1 passed).
- `crates/tui/src/model.rs::take_slash_owner_chip_and_offline_sentence` — `/take` without/with a session, `session_row` chip only when owned elsewhere, `SessionOwnerChanged` updating the picker and printing the line, `owner_offline_line`, `owner_unreachable` in both forms and its rejections (written, not run).

## Verification run
- `bunx tsc --noEmit`: 69 errors, none in files under this work (`lib/owner*`, `lib/rpc.ts`, `rpc/api.ts`, `sidebar/SessionItem.tsx`, `sidebar/SessionList*.ts(x)`, `chat/Header.tsx`, `chat/SessionMenu.tsx`, `chat/slash.ts`, `chat/Composer.tsx`, `chat/OwnerOfflineCard.tsx`, `chat/OrphansBanner*.ts(x)`, `routes/c.$session.tsx`, `settings/Mesh.tsx`, `lib/attach.test.ts`); breakdown in deviation 10.
- `bunx vitest run src/lib/owner.test.ts src/components/chat/OrphansBanner.test.ts src/rpc/api.test.ts`: 3 files, 7 tests passed (`api.test.ts` confirms `METHODS` == the regenerated `methods.json`). `SessionList.test.ts` fails to import for a pre-existing reason (`@openuidev/react-ui` CSS missing), unrelated.
- `cargo check -p tui --tests`: `Finished dev profile [unoptimized + debuginfo] target(s) in 6m 20s`, exit 0, no warnings in `crates/tui` (branch head `4813846`). Three earlier attempts never reached `tui`: two hit a 300 s shell deadline while checking cold dependencies, one ran under a PATH without `cmake` (boring-sys build script); disclosed against the once-per-commit allowance.

DONE U
