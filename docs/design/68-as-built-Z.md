# 68 worker Z — report (two-engine smoke), 2026-09-25

Worktree `/Users/Sky.Moore/Develop/theuth-68`, branch `feature/68-sessions`, started from the merged head `bf18784`. Full evidence and recipe: `docs/design/68-two-engine-smoke.md`; raw notes in `/tmp/68-z/evidence.md`; scripts in `/tmp/68-z/*.ts`. Brief: `/tmp/68/omp-Z.md`.

## Result per invariant

| # | Invariant | Result | Key evidence |
|---|---|---|---|
| 1 | lone engine: log written + pruned, no shipping | ✓ | 12 log rows → 1 (head) after the 60 s tick; 0 sessionsync log lines |
| 2 | session RPCs byte-identical to `main` 654febe | ✓ | 15-call diff: only `owner/epoch/here` on `list_sessions` rows |
| 3 | append latency unchanged within noise | ✗ | release in-process 85 → 163 µs/append (+78); debug RPC-level p50 407 → 710 µs |
| 4 | 126 MB migration < 2 s, `owner = self, epoch = 1` | ✓ | migration 3.76 ms; agent plugin up in 0.44 s; 173/173 `self/1/true` |
| 5 | chat on A on B within 1 s | ✓ | user row +127 ms; assistant row on B ≤ A |
| 6 | continue here: epoch 2 both, B runs the turn, chip on A | ✓ after fixes 1+2 | take 53 ms; first turn on B `FOREIGN KEY … cost_owners` → fixed; `pung` on both at 781 ms |
| 7 | SIGKILL A: `write` refused < 300 s, `user` succeeds, A demotes, orphans, no merge | ✓ | `was seen 98s ago; takeover after 300s`; epoch 2; A demotes, orphans `[{epoch 1, seq 33}]`; transcripts identical |
| 8 | desk steer from the non-hub forwarded, events < 1 s | ✓ after fix 4 | desks were taken (`sessions.agent` null) → fixed; steer 53 ms, owner unchanged, events ≈15 ms behind the hub |
| 9 | fresh C bootstraps the 126 MB store, follows live | ✓ after fix 5 | empty-log store never offered a snapshot → fixed; 122 MB / 24,944 rows in ≈8 s; live row 777 ms |
| 10 | artifact on B by hash; spill fetched | ✓ after fix 6 (spill file hand-placed) | record 41 ms, blob 51 ms, identical; spill 109 KB in 70 ms |

## Commits (each `fix(<crate>): two-engine — …`)

1. `ea785a5 fix(replica)` — `ReplicaHandle::announce_unstamped()` (whole-row announce of `hlc IS NULL` rows, then stamp); `row_as_changes` `pub(crate)`; test `pre_existing_rows_are_announced_once` in `crates/replica/tests/sync_loopback.rs` (+ `node_in`-style setup inline).
2. `ab94e7c fix(agent)` — `Store::replicate_records()` attaches `projects`/`cost_owners` to the stage-1 record replica on the write pool and announces unstamped rows; `supervisor::start` spawns it behind `lookup_when::<replica::Replica>`. Root cause of invariant 6's failure: decision 8 was never wired by C/L/O/F, so a taken session's `model_calls` insert on the new owner hit the FK.
3. `4075efa fix(mesh)` — `mesh.bind: Option<SocketAddr>` → `BindOpts.bind_addr`; default-config comment in `crates/theuth/src/main.rs`.
4. `a658477 fix(rpc)` — `forward::classify`: a pinned root titled `agent:<name>` is a desk → `Forward(owner)`; `classify_user_vs_desk` extended (5 `forward::` tests green). Also commits the generated `crates/rpc/bindings/FrontDoorError.ts` (F's `ts(export)` type; produced by running the rpc tests).
5. `bafbbd4 fix(sessionsync)` — `held_here` offers `(me, head)` when `Store::owns_sessions()` (new, `crates/agent/src/store.rs`), advertises every cursor row, `Node::cursor → Option<u64>`; `receive_snapshot` applies when no cursor row exists; test `a_store_with_sessions_but_no_log_bootstraps_a_peer` (+ `node_in` helper) in `sync.rs` (12 sessionsync tests green).
6. `b0b8a8f fix(rpc)` — `get_attachment` routes `…/spill/<session>/<call_id>.txt` to `artifact::mesh::fetch_spill`; `spill/` joins the served directories.
7. last: `docs(68): two-engine smoke + as-built Z` — `docs/design/68-two-engine-smoke.md`, `docs/design/68-as-built-Z.md`.

## Deviations

- **Firewall / pairing.** macOS ALF had `target/debug/t` set to *Block incoming connections*; `mesh_add` with the LAN pairing string timed out and restarted engines never reconnected (random port + LAN addrs). No `sudo`. Decided: add `mesh.bind` (fix 3) and run every engine on `127.0.0.1:<fixed port>`; the first pairing before the knob used a pairing string rewritten to `127.0.0.1` (`pair.ts … lo`).
- **Two hubs.** Two fresh engines each keep their own lease (6 h probation); `mesh_set_hub ["never"]` on B for invariant 8. Not a 68 issue; recorded.
- **Invariant 7 `user` take.** I probed `session_take(id,"orphan")` before `"user"`; `orphan` succeeded (O2 skips the 300 s wait), so the `user` call was a no-op success on an already-taken row. The unreachable-owner `user` path shares that branch; the reachable-owner `user` path is invariant 6.
- **Invariant 8 first run was on a stale build** (A/B restarted before the classify build finished): those steers took the desk. Redone on the fixed build; only the redo counts.
- **Spill** was hand-placed (no reachable direct tool exceeds the 32,000-char cap here: the python REPL caps prints at 8,000; `spawn_subagent` needs `agent.communication`). The mesh fetch path is exercised end to end; the writer side (`spill::cap`) is unchanged code.
- **Invariant 3 fails as literally stated**; reported with numbers, not fixed (follow-up named in the smoke doc §2.3).
- **Store test probe** for latency was a throwaway `#[ignore]` test, run in debug and release, then removed before committing.
- Files outside a Z allow-list (Z may fix anywhere with disclosure): `crates/agent/src/{store.rs,supervisor.rs}`, `crates/replica/src/{engine.rs,apply.rs}`, `crates/replica/tests/sync_loopback.rs`, `crates/mesh/src/{config.rs,lib.rs}`, `crates/theuth/src/main.rs`, `crates/rpc/src/{forward.rs,api.rs}`, `crates/rpc/bindings/FrontDoorError.ts`, `crates/sessionsync/src/sync.rs`.
- Ran what the brief allows: `cargo build -p theuth --bin t` (several times), headless engines, `cargo test -p replica --test sync_loopback`, `cargo test -p sessionsync --lib`, `cargo test -p rpc --lib forward::`, `cargo test -p agent --lib store::` (+ the release probe). `main`'s `t` built in `~/Develop/theuth` (its tree has only pre-existing untracked docs; nothing changed there).

## Not built / known gaps confirmed

- L's gap (take row before the sessions row): common join-after-take path works (fresh D got B's taken sessions at epoch 2 through A's owner-filtered snapshot); the partitioned-peer case not reproduced.
- F's gap (attachments over the mesh; forwarded `send_input{files}`): confirmed by reading, not fixed.
- Append-latency cost (invariant 3); `agents` record/file not projected on B within the window; `last_seen` in the take refusal is the presence stamp; orphan count = unacked rows; artifact "owner offline" wording.

## Tests written
- `replica/tests/sync_loopback.rs::pre_existing_rows_are_announced_once`
- `sessionsync/src/sync.rs::tests::a_store_with_sessions_but_no_log_bootstraps_a_peer` (+ `node_in`)
- `rpc/src/forward.rs::tests::classify_user_vs_desk` (extended with a roster desk row)

## cargo check
`cargo check -p theuth -p agent -p sessionsync -p rpc -p replica -p mesh --tests`: `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 1m 14s`, no warnings. `cargo test -p agent --lib store::`: `55 passed; 0 failed`. `cargo test -p sessionsync --lib`: `12 passed`. `cargo test -p replica --test sync_loopback`: `3 passed`. `cargo test -p rpc --lib forward::`: `5 passed`.

DONE Z
