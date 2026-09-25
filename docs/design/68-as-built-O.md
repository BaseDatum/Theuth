# 68 worker O — as built (2026-09-25)

Branch `feature/68-sessions-o`, worktree `/Users/Sky.Moore/Develop/theuth-68-o`. Goal: 68 §4 end to end — take, release, demote, orphans, epoch fence, resume takes the session.

## Commits

### 1. `feat(sessionsync): ownership — take, release, demote, orphans, epoch fence; resume takes the session` (3eb8ad9)

**`crates/sessionsync/src/owner.rs` (new, mine)**
- `Take { rt, config, store, transport, connected, peers }` + `Take::run(id, reason) -> Result<(owner, epoch), String>` (68 §4.2). Reads the fence; owner `""`/self → no-op `Ok((owner, epoch))`; no mesh identity → `Err`. Owner connected → `release_from`: `open(peer, ALPN_SESSIONS)`, `LengthDelimitedCodec` frame `Msg::Release`, await `Msg::Released{last_seq}` ≤ `release_wait_secs`, then poll `Store::cursors()` every 100 ms until cursor `(owner, "")` ≥ `last_seq` (same bound). Any failure there: `reason = User` → proceed with a warning (the tail becomes A's orphans); otherwise `Err("owner … busy: …")`. Owner not connected and `reason = Write` → `unseen_long_enough`: mesh `Peer.last_seen_ms` must be ≥ `takeover_secs` ago (peer unknown to the mesh = allowed; `reachable = true` = `Err`, retry). Then: re-read the fence (a concurrent take moved it → `Err`), `UnloadSession` on the agent supervisor, `Store::set_owner(id, me, e+1)` (the logged first row of the new epoch), `Store::set_fence(id, e+1, me)`, `rt.publish(AgentEvent::SessionOwnerChanged { here: true })`.
- `release(store, id) -> Result<u64>`: `UnloadSession`, `Store::flush()` (empty write transaction on the single write connection), `Store::session_log_seq(id)` as `last_seq`. `answer_release(store, session, framed)` sends `Msg::Released`.
- `Demote { session, old_epoch, by }` (tell) + `demote(rt, store, m) -> Result<u64>` (§4.3): `UnloadSession`; orphan `session_log` rows of `(session, old_epoch)` with `seq >` cursor `(me, by)` — what the taker had applied of our log (0 = all); `set_fence` to the applied row's `(owner, epoch)`; publish `SessionOwnerChanged { here: false }`; if `n > 0`, `Store::append_local_note` "N turns from <rfc3339 of the first orphan's HLC> were not delivered on <engine>; `session_orphans` to view".
- `on_applied(store, origin, session, fence_before, epoch)`: called from the apply path; when the applied row's epoch exceeds a fence whose owner is this engine, `tell`s `Demote` to the sessions actor (warns if none).
- `sweep_orphans(store, keep_days, shutdown)`: hourly `Store::prune_orphans_before(now − keep_days)` until shutdown.
- `framed(stream)`: the record replica's framing (`MAX_FRAME`).

**`crates/sessionsync/src/lib.rs`**
- `TakeSession` handler: `DelegatedReply` — snapshots `rt`/`config`/`store`/`transport`/connected peers and the mesh's `Status.peers` (empty without a mesh), runs `Take::run` in a spawned task (the actor's mailbox never waits on the network). RPC `ask` is unchanged: `DelegatedReply`'s `Ok` is the inner tuple.
- `ReleaseSession` handler → `owner::release`. New `Message<owner::Demote>` handler → `owner::demote`.
- `SessionSync::serve` (L's; disclosed): decodes the first frame; `Msg::Release` → `owner::answer_release`; any other tag logs "not served yet" and returns, as before. L's Hello protocol slots in around the `match`.
- Plugin `start`: spawns `owner::sweep_orphans` before the actor.

**`crates/sessionsync/src/apply.rs` (disclosed, 5 lines)**: `apply_log_row` keeps the fence it read (`before`) and after `commit` calls `owner::on_applied(store, origin, session, before, row.epoch)`. Signature and `Applied` unchanged.

**`crates/sessionsync/Cargo.toml`**: `mesh` dependency (for `mesh::Status`/`Peer`); dev-deps `figment`, `uuid`, `genai`, `parking_lot`.

**`crates/agent/src/store.rs`** (guard + orphans allow-list)
- `WriteTx::commit` step 4 now reads the **fence**: `SELECT owner, epoch FROM (session_epochs ∪ sessions row) ORDER BY epoch DESC LIMIT 1` inside the transaction (one query per touched session, as before). A fence raised by `set_fence` to a foreign owner refuses writes even while the session row still names us. The DELETE fallback is unchanged. The take (`owner = me, epoch = e+1`) still passes: its post-state row is the highest epoch.
- `log_orphan_after(session, epoch, after)`; `log_orphan` = `log_orphan_after(…, 0)`.
- `session_log_seq(session) -> i64` (`MAX(seq)` of the session's rows, 0 when none).
- `flush()`: `begin_write` + `commit` (logs nothing).
- `append_local_note(id, text) -> seq`: `ChatMessage::system(text)` through `begin_apply` (capture off → not logged, no guard).

**`crates/agent/src/lib.rs`, `supervisor.rs` (disclosed)**: `UnloadSession { session }` (`ask → bool`): removes the live entry, `tell_cancel` + `kill` as `DeleteSession` does, `running.remove`. Rows untouched; the next input respawns an actor whose writes the guard decides.

**`crates/tasks/src/ledger.rs`**
- `trait SessionTaker { async fn take(&self, session: &str) -> Result<(), String> }`; `MeshTaker` = `sessionsync::actor().ask(TakeSession { reason: Orphan })`. `Ledger.taker: Arc<dyn SessionTaker>` defaults to `MeshTaker` in `Ledger::new` (no change in `lib.rs`).
- `sweep`: for a checkout whose engine `!= self.engine` (never a hung local session), `taker.take(&c.session)` before the `Resume` wake. Success → `n.engine = self.engine`, `n.taken = true`, thread note "… presumed lost; taken here (<engine>), resume it". Failure → the old note ("resume from the last progress note") and the checkout stays on the lost engine.
- **`crates/tasks/src/types.rs` (disclosed)**: `Checkout.taken: bool` (`serde(default)`), set only by the sweep; `checkout()` writes `false`. Needed so the roster can tell "taken here" from "hung here".
- **`crates/tasks/Cargo.toml`**: `sessionsync` dependency (acyclic: sessionsync → agent/replica/mesh only).

**`crates/roster/src/lib.rs` (disclosed)**: `DeliverWake` sends the wake input to `task.checkout.session` when `wake.reason == Resume && checkout.taken` (else the desk session, as before). The resumed work continues in the replicated transcript, now owned here; the desk itself is untouched.

**`crates/rpc/src/api.rs`**: `session_take` body unchanged — the contract's body already asks `TakeSession` and returns `SessionOwner { here: true }`; with the handler real it is complete. `session_owner`/`session_orphans` unchanged.

### 2. `docs(68): as-built O` — this report copied to `docs/design/68-as-built-O.md`.

## Deviations

| # | Decision | Why |
|---|---|---|
| O1 | **Guard reads the fence (`session_epochs` ∪ row), not the row alone** (changes contract C3's query, same shape/cost). | The brief's item 4 says "fence owner ≠ self"; with it, a released/demoted owner is refused as soon as its fence moves, not only once the taker's row lands. `log_api_round_trip` fenced `id` then wrote to it; it now fences `other`. |
| O2 | **`reason = Orphan` skips the `takeover_secs` check** (only `Write` waits). | `tasks.orphan_secs` defaults to 120 s < 300 s; the ledger has already judged the engine lost by its own timer and re-woken the owner, so a refused take would only mean a new session from the progress note — strictly worse. The user path (`User`) skips as briefed. |
| O3 | **Release round-trip failure with `Orphan`/`Write` → `Err(owner busy)`**; `User` proceeds. | Brief specified `Write` → `OwnerBusy` and `User` → proceed; `Orphan` treated as `Write` (conservative; the ledger's fallback stays). |
| O4 | `Take` runs off the mailbox (`DelegatedReply`). | A take waits up to 2 × `release_wait_secs` on the network; the mailbox must keep taking `PeerConnected`/`Demote`. |
| O5 | Demote is triggered from `apply_log_row` via a `tell` to the actor, not from L's sync worker. | L's branch is not merged; the hook is 5 lines in C's real apply function, signature unchanged, so L needs nothing. |
| O6 | The **`Release` answer is served in `SessionSync::serve`** (L's function) as a first-frame dispatch. | The taker's stream is a fresh `theuth/sessions/1` stream; something must route tag 3. L's Hello protocol goes in the `other` arm. |
| O7 | `fork` of a foreign session **passes** the guard. | It reads the source and writes only the new session (owner = me); the source row is untouched (C2). Asserted explicitly in the test. |
| O8 | `set_owner(id, "y", 3)` behind fence `(x, 2)` is refused as **`y`'s** (`NotOwner { owner: "y", epoch: 3 }`). | Post-state judgement (C3). |
| O9 | `Checkout.taken` flag added (tasks `types.rs`). | Without it the roster cannot distinguish a session taken here from a hung local session — routing a `Resume` into a hung session would regress 67 §7.5. |
| O10 | `release()` does not fence locally. | A turn starting between the kill and the taker's row still writes and becomes an orphan on demote — 68 §4.3 by construction. Marked `ponytail:` in `owner.rs`. |
| O11 | The demote note lands at `MAX(seq)+1` of the local copy; the new owner's next message replaces it (owner wins). | Marked `ponytail:`; `session_orphans` keeps the ranges. |
| O12 | `sweep_orphans` is hourly, spawned by the plugin (not in L's `prune.rs`). | Brief item 3 names the `orphan_keep_days` sweep under O. |
| O13 | Tests use `std::sync::Mutex` in `tasks` (matching the file's existing `MemRouter`), `parking_lot` in `sessionsync` (new dev-dep). | Convention of each file. |

## Not built
- `session_take` from the TUI/desktop (U). Forwarding/take-on-write at the front door (F) — `TakeSession { reason: Write }` is ready for it.
- Re-attempting a refused orphan take on later sweeps (the sweep fires once per checkout, as in 67).
- No mesh in tests: `Take.peers` is injected; the actor fills it from `mesh::Status`.

## Tests written (all run, all green)
- `sessionsync` (`cargo test -p sessionsync --lib owner::`, 3 passed): `take_from_a_reachable_owner_moves_the_tail_and_the_epoch` (loopback transport, `Release`/`Released` through `SessionSync::serve`, cursor wait, epoch 2 on both, A fenced out, 0 orphans), `take_from_an_unreachable_owner_respects_takeover_secs` (Write refused at 100 s / reachable; Orphan at once; no-op when ours; third engine takes at 301 s → epoch 3; unknown id), `stale_owner_demotes_with_its_unshipped_rows_orphaned` (2 unshipped rows orphaned, one `SessionOrphan` range with a timestamp, local note text, stale rows → `StaleEpoch` on B, `release` on a non-owner). In-memory transport: 40 lines (`Switch`/`Loop`).
- `agent` (`cargo test -p agent --lib -- store::`, 55 passed): `every_mutator_is_refused_behind_a_foreign_fence` (35 mutating calls, each `NotOwner { owner: "x", epoch: 2 }`, nothing logged, rows rolled back, fork passes, take back at epoch 3), `orphans_above_the_shipped_cursor_and_local_notes_are_not_logged`; `log_api_round_trip` adjusted (O1).
- `tasks` (`cargo test -p tasks --lib resume_takes`, 1 passed): `resume_takes_the_lost_engines_session` (take before wake, checkout → this engine + `taken`, note text, taken session keeps commenting; refused take keeps the fallback; hung local session never taken).

## `cargo check`
`cargo check -p agent -p sessionsync -p tasks -p roster --tests`: `Finished dev profile` (no warnings). `cargo check -p rpc`: `Finished dev profile` (`session_take`'s `ask` compiles against `DelegatedReply` by `<Reply>::Ok`).

DONE O
