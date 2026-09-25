# Design 67 — loose ends after stage 1

*2026-09-25. Companion to `docs/design/67-agents.md` §20 (as built). Everything here is either a stage-1 gap a worker could not build, an open question the design left for evidence, or stage 2 itself. Nothing here blocks merging `feature/67-agents` (86efc3a): the single-engine smoke and the workspace tests pass, and each item degrades to "the old behaviour" today.*

## 1. Stage 2 — session replication with transferable ownership

The second half of ruling 10 ("stage it"). Stage 1 replicated the coordination state (agents, tasks, wakes, schedules, skills, mcp, vault, OAuth grants, knowledge, engine stats). Stage 2 replicates the **session store** itself.

| | Stage 1 (built) | Stage 2 |
|---|---|---|
| Where a session lives | the engine that ran it | every engine holds a replica; the `sessions` row carries `owner: <engine>` and `epoch` |
| Who writes | the creating engine, for life | one writer per epoch; ownership transfers when the owner is unreachable for `sessions.takeover_secs` (default 300) and a peer needs to write — the taker sets `owner = self, epoch + 1`; the old owner demotes itself to a replica on return; nothing is ever merged |
| Resuming a lost task's work | from the progress note in the task thread (§7.5) | `session/load` of the full transcript on another engine |
| Opening a past chat | only where it ran | anywhere the client is attached |
| Capture | AFTER-triggers into `_replica_log` | the SQLite session extension — single writer on our own connection; proven under SQLCipher by `67-replica-spike.md` (+0.45 µs/row, 40 B/row changesets, 10k rows apply in 22 ms) |
| Stream | — | `theuth/sessions/1`, commit-ordered row log for `messages`, `model_calls`, `inner_calls`, cost rows |

Open questions the stage-2 doc must answer: first-sync size (a new engine pulling 100+ MB — bootstrap from a **snapshot**, not a replay of every batch; this also closes item 2.4 below), transcript privacy on shared engines (an agent's private session on a box the user does not own), stale-owner writes after a partition (epoch guards on every write), whether artifacts travel with sessions or on demand, and whether devices move from the 31 relay onto iroh at the same time (item 3.2).

## 2. Stage-1 gaps (a worker could not build it as designed)

| # | Item | Design says | Today | Fix shape | Size |
|---|---|---|---|---|---|
| 2.1 | Manager-routed **tool** approvals | §11: a manager agent may approve a child's `approvals: ask` tool call within its own `tools` ceiling; above it, the user | every tool approval goes to the user as before; only *decisions* (`escalate`) go up the line | a registry hook that lets a tool resolve a pending `Approval` by id, then a `decide` path for managers mirroring `escalate`'s | small |
| 2.2 | 42 push for escalations | §11, §12: the user gets an escalation as a push notification | desktop card + Slack/Signal thread only | depends on design 42 landing; nothing to do in 67 | n/a |
| 2.3 | `mesh.name` in placement | §8: engines are named by `mesh.name` (default hostname) | `tasks::place` and `agent: "<harness>@<engine>"` resolve the *local* engine by hostname only | read `mesh.name` where X builds the local engine's `EngineStats`/node row (`crates/rpc/src/acp.rs::serve_mesh`, `crates/tasks/src/place.rs`) | tiny |
| 2.4 | Outbox not pruned while solo | §20: "not pruned — it is the catch-up log for a later-paired engine" | after 500 tasks: 33–152 rows / 1.1 MB, idle growth 0; grows with use on a never-paired engine | a snapshot bootstrap for the first pairing (stage 2), after which the outbox prunes below the minimum acked seq | part of stage 2 |
| 2.5 | `tasks::store::hlc` text not parseable by `replica::Hlc::parse` | one HLC format | replica re-stamps the row on capture — correct, untidy, and the task's own stamp is lost | emit `replica::Hlc::to_text` from the tasks store as M2 did for mesh | tiny |
| 2.6 | Windows targets in `t mesh add --ssh` | §13: detect OS/arch, install `t` | detected, then refused | a PowerShell install path (service via `--install-service`, which 32's installer already has for Windows) | small |
| 2.7 | `put` over SSH is `cat >` over exec | §13 / 64 §4: `put` primitive | works; binary-safe via base64 in the built-in transport | `russh-sftp` if large files ever matter | optional |
| 2.8 | Slack call-out reply prefix | §10 `[orch]`: `<Title> —` prefix | as built | Slack's `chat.postMessage` `username` override (needs `chat:write.customize`) if the prefix reads badly in practice | tiny, needs a look at real threads |

## 3. Open questions (need evidence, not code, first)

| # | Question | Why it matters | How to settle it |
|---|---|---|---|
| 3.1 | **Slack Socket Mode under two hubs** | §4.4 assumes Slack load-balances events across an app's open connections, so a brief double-hub yields at most a duplicate reply | read Slack's Socket Mode docs on multiple connections; if events are *fanned out* rather than balanced, the Slack duty needs a stricter lease (a short-TTL vault row as a mutex, accepting a one-TTL outage on hub change) |
| 3.2 | **Devices over iroh** | today phones/desktops reach an engine over the 31 relay (Noise); engines reach each other over iroh — two transports, two relays on `relay.theuth.io` | a small doc: cost of moving the device link onto iroh (QR pairing is the same shape) vs. living with two; natural to decide alongside stage 2 |
| 3.3 | **Availability observation across never-overlapping engines** | §18 item 6: two engines only ever on at different times score each other as mostly-unreachable; election buckets may need tuning | run the mesh on Sky's real machines (Mac + build-A + a container) for a week; read `availability` rows and `lease_log` |
| 3.4 | **Desk on the hub vs. on a preferred engine** | §18 item 3 `[orch]`: desks follow the lease; an agent whose work is always on one engine might prefer its desk there | same week of real use; look at how often desks rotate on lease changes |
| 3.5 | **Placement latency for unplaced tasks** | `task_place` asks theuth's desk for ambiguous cases — one model turn | measure on real call-outs; add a pure-policy fast path for the unambiguous majority if it annoys |
| 3.6 | **iroh relay deployment** | `iroh.relay.theuth.io` is configured as the default but not yet deployed; stage 1 engines join the network lazily so nothing breaks offline | deploy `deploy/relay`'s `iroh-relay` beside the 31 relay (compose/k8s files exist); confirm the UDP port |

## 4. Verification debt

- `cargo test -p tools` — `pty::tests::{query_responder, tty_holder_outliving_exit_keeps_an_operation_open}` fail from any `shell()` job under Sky's zsh and pass in omp's shell; pre-existing, documented, not 67's. Worth a fixture that does not depend on the login shell.
- `cargo test -p codeindex --test index` indexes the live repo: `indexes_this_repository` goes red if anyone commits mid-test; `map_and_symbol` now follows `open_db` by symbol (1dc8ec2). A fixture repo would end both.
- Load-sensitive flakes seen once each during the wave, all passing alone: `rpc engine::tests::second_acquire_in_one_process_is_running`, `voice live::tests::utterer_returns_the_spoken_sentence_punctuated`, `replica files::tests::schedules_skills_mcp_round_trip` (fixed in V2 to drive the poll instead of racing it).
- Not exercised yet by any test or smoke: two real engines on two machines (pairing string over the relay, lease handover on lid close, orphaned checkout resumed on the other engine, Signal as two linked devices). That is the first thing to do after merging — on the Mac + build-A — before trusting the mesh with real work.

## 5. Housekeeping

- Merge `feature/67-agents` into `main` on Sky's word (`git merge --no-ff`); move the untracked `docs/design/67-agents.md` in Sky's checkout aside first (it is the same v0.6 text). Push only when told. Then `git worktree remove ../theuth-67`.
- Design 64 is committed as withdrawn; `docs/design/scratch-always-on-recon-*.md` and `scratch-broadcast-recon.md` in Sky's checkout are recon scratch and can go.
- `~/.theuth/briefs/67/` and `/tmp/67/` hold the twelve briefs, reports and fix-round briefs; keep the briefs, the rest is disposable.
