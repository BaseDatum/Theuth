# 185 notes — routine home and dreaming on the hub (as built)

Branch `fix/185-routine-home` off main b48578c2. Rulings: `185-rulings.md`. Recon: `185-recon-routines-dreaming.md`.

## A. Dreaming's routine file is local to each engine

### A1 Mirror skip (`replica/src/files.rs`, `schedule/src/file.rs`, `schedule/src/lib.rs`)
- `replica::files::Policy` (trait, two methods): `local(name)` and `clash(name, ours, theirs)`. `mirror` / `mirror_at` / the actor's `Mirror` message take `policy: Option<Arc<dyn Policy>>`; skills, mcp, doc_files pass `None` (unchanged).
- The schedules mirror passes `RoutinePolicy`. `local` is the named list `schedule::file::LOCAL_ROUTINES = ["dreaming"]` (`is_local_routine("dreaming.md")`). Not a front-matter key (`deny_unknown_fields`, file.rs:437, would break older engines).
- A local name is skipped at every point the mirror touches a file: `scan` never upserts it (never published) and never flags it deleted when it disappears (no tombstone goes out); the live-row loop never projects a stored row for it (no resurrection); `project_locked` returns before reading the row, so an older engine's row or tombstone never writes or deletes the file; `project_all` goes through the same function.
- Stored rows from before the upgrade stay in the table untouched. They are inert here; an older peer keeps its own copy.

### A2 Template and self-heal (`dreaming/src/lib.rs`, `night.rs`, `routine.md`)
- The template gains `{{engine_line}}` → `engine: <this engine's id>` (from the identity file, as before).
- `ROUTINE_KEYS` is now `budget, when, run_timeout, model, cap, cwd, engine`. Every plugin start and every settings save (`SetSettings` → `install_files`) rewrites those lines from THIS engine's config and data dir, so a file that holds another engine's bytes (`cwd`, `engine:`, `engine=`) repairs itself on the upgrade, on both engines. A file with no `engine:` line gains one. `enabled:` and every other line stay.
- The plugin passes only its own id (`me`); it never writes another engine's identity. `dreaming.home_engine` is no longer passed as the stamp.

### A3 Gate (`sensor.py` 4, `sensor_rules.rs`, `signals.rs`, `types.rs`)
- `signals.json` (`SignalsSnapshot`) gains `hub` (`mesh::is_hub`), `peered` (`mesh::has_peers`), `hub_reachable` (the lease holder is this engine or a connected peer, from `mesh::Status`) and `hub_name`. Refreshed at start and every 60 s heartbeat. Old files read all four as false/None (serde default), so the row never fires on them.
- New row, after "signals stale", in both tables: without the pin, `peered && !hub && hub_reachable` → `not the hub (dreaming runs on <hub name | the hub>)`. The Rust gate now answers `Result<(), String>`.
- Cut off: the lone side of a partition elects itself (lease.rs:133-141), so `hub` is true there. A `hub: never` engine is never elected; cut off, its holder is not connected (`hub_reachable: false`) → it dreams. No "hub covers an offline node" code.
- **Call: the `engine=` sensor arg is dropped from the routine** except as the explicit `dreaming.home_engine` pin. With the pin set, `engine=<pin>` is written and the pin decides alone ("another engine owns dreaming" unless pinned here); the hub row is skipped. Without it, the hub rule decides.
- Lockstep: `rule_table` and `python_sensor_answers_the_same_table` run the same `table()` = the 136 rows under the pin plus nine hub rows (lone, hub, non-hub reaching the hub, hub name unknown, partitioned, `hub: never` cut off, non-hub outside the window, pinned here, pinned elsewhere).

### A4 Mesh-wide activity (`signals.rs`, `agent/src/store.rs`)
- `sessions_since` = max(this engine's attended-root event count, `Store::attended_roots_since(last night's end)`): root, attended sessions of any engine whose `updated_at` is later (session rows replicate whole, sessionsync changesets).
- `new_concepts_since` = max(this engine's put-event count, `concepts_since(knowledge, last night's end)`): harvest/model concepts whose frontmatter `timestamp` is later. The frontmatter travels with the bundle file, so a concept another engine wrote and the sync brought here (a `Sync` event, which the local counter ignores) counts.
- **Call: max, not sum.** The store count already includes this engine's own work; the local counter is kept so nothing counts less than before (it also sees updates, which the store count — created-after only — does not).
- The store counts run from the last `Dreamt` (not `DreamStarted`), matching the old reset point, so a heavy day keeps its second window.
- **Idle and desks stay local.** They guard this machine's own load (a turn or desk running here must not race a night here). Using the mesh's would stop the hub dreaming while Sky works on the other machine, which is the exact failure A4 removes.

### A5 Dream now (`rpc/src/api.rs`, `dreaming/src/rpc.rs`, `night.rs`, UI)
- `dreaming.run_now` is `with_extensions`. `dreaming::rpc::run_now_host()` asks the Night actor (`Host`) for `signals::dream_host`: the `home_engine` pin when set and connected (none when pinned here), else the hub when this engine is peered, not the hub, and the hub is connected. Then `forward::to_peer(…, "dreaming.run_now", [request])`. `to_peer` never makes a second hop and answers `None` when the peer is not connected, so a cut-off engine runs it here.
- `DreamStatus.runs_on` (optional): the hub's name when the press would be forwarded. The Dream now dialog says "Runs on <hub>, the hub, at once …" or "Runs on this machine at once …".

## B. Knowledge: the hub wins real conflicts (`knowledge/src/sync.rs`)
- Base: per peer, `<data_dir>/knowledge-sync/<peer>.json`, per path `{blob, seconds}`. After each exchange it holds every path both manifests agreed on, every path this side sent, every path it applied. Outside the bundle root, so never committed.
- Per differing path: one side changed since the base → that side wins (even a node against the hub). Both changed → a conflict: exactly one `hub` → it wins; neither → newest-wins (two nodes syncing with each other).
- **Call: both claim hub (the heal window) → the conflict waits.** Each side keeps its own version, the base is not moved for that path, and the next sync tick (300 s) decides it, normally with one hub by then. The brief's "fall back to newest-wins and retry" would converge both sides on the newest version in that first exchange — which is the exchange right after a heal, since sync fires on `Connected` before the first heartbeat resolves the lease (recon 30) — and leave nothing to retry, so the hub would almost never win. Ceiling: if both keep claiming hub forever (gossip broken), that path stays split until one releases; nothing is lost.
- No base yet (first exchange with a peer, including the first after this upgrade) → newest-wins for everything, then the base is stored.
- Each side applies only the files its own rule gives the peer. Tombstones are versions; a deletion against a path the peer never had is not sent. The loser stays in git history (the received file lands as a `sync from` commit). One `info` line per conflict, naming the path, the peer and the outcome.
- `Manifest.hub` is `#[serde(default)]`. `knowledge` now depends on `mesh` (for `is_hub`; no cycle: mesh does not depend on knowledge).
- **Mixed versions.** An older engine sends no `hub` (false) and keeps newest-wins, and it applies everything it receives. New hub ↔ old node: the hub keeps its version of a conflict and sends it; the old node applies it, so both end on the hub's. Old hub ↔ new node: neither claims hub → newest-wins on both. One-sided changes behave as today. Under clock skew the old side may pick the other version for one round; upgrade both.

## C. Same-name routines keep both

### C7 On mirror apply (`schedule/src/file.rs` `clash`, `different_routines`, `clash_name`, `renamed_from`)
- The mirror asks the policy only for a peer's live row (`hlc.origin` ≠ this replica: never our own echo) that would replace a file on disk with other bytes.
- **Call: "different routine" = both stamped `engine:` with different engines, and not the same `created:`.** The brief also listed "different `created:` stamps"; the schedule tool rewrites `created:` on every write of a `when:` file (tools.rs `render_args_on`), so a created difference alone is an edit of one routine. Same `created:` with two engines is XTICK's racing claim of one legacy file and stays last-writer-wins. Legacy (unstamped) files on either side stay last-writer-wins as today.
- **Call: who is renamed.** The brief's "write the incoming one renamed" cannot converge (each side sees the other's as incoming). Both engines instead pick the same routine from the bytes alone: the one created later (then the higher engine id) is "the second" and moves to `<name>-<first 8 of its engine id>` (`[a-z0-9-]`, ≤ 40, stem cut). If it is the incoming one, the mirror writes it under the new name and re-stamps our row so ours keeps the name everywhere (`KeepOurs`); if it is ours, ours moves and the incoming takes the name (`MoveOurs`). A target name already holding other bytes → last-writer-wins with a warning.
- Convergence proof: `replica` test `same_name_different_routines_keep_both_and_converge` (two engines, both directions, then two more rounds write no batch: no ping-pong) with the same rule shape, and `schedule` test `same_name_routines_keep_both_and_both_engines_agree` (the real rule: A says KeepOurs{x-bbbbbbbb}, B says MoveOurs{x-bbbbbbbb}). At most one side re-stamps; the renamed copy is written by both with the same bytes, and `put_row` skips a row that already holds them.
- Never renamed: `dreaming.md` (local), `autoresearch-*` (its plugin recreates a missing study file by name, so a rename would fight it; last-writer-wins as today).
- Surface: `ScheduleInfo.renamed_from` (optional, bindings) = the stem, derived from the files (`<stem>-<own engine id8>` where `<stem>` is a different routine); no state kept.
- Ceiling: the renamed routine starts a fresh `runs` history under its new name; an `every:` one stamped here fires once at the rename (it is new here, `made_here`).

### C8 Create refuses a name in use (`schedule/src/lib.rs` `NameInUse`, `tools.rs`, `app/src/routine_draft.rs`)
- `NameInUse(name, legacy)` → `a routine named <x> already runs on <machine>; choose another name` (machine from `mesh_status`, else "another machine"). Ours (stamped here) is never refused; nor a legacy file with local history, or any legacy file on a lone engine.
- `schedule` tool: refuses before writing (`legacy = true`: an unclaimed legacy file this engine never ran counts as another machine's). `routine_draft`: the card is refused the same way (its accept is a `schedule` call). Editor `Put`: refused when the call carries no `engine:` (a create; an edit sends the stamp it read) and the file is stamped by another engine; **call:** a legacy file stands there, since the editor's create and an edit of a legacy file look alike.
- Model-facing text: the `schedule` tool description and its `name` doc, and `RoutineDraft.name`, say a name used on another machine is refused.

## D9 UI (`routines/RoutinesView.tsx`, `routines/helpers.ts`, `settings/Dreaming.tsx`, `lib/dreaming.ts`)
- `routineNotes`: `renamed: <machine> already had a routine called <name>` (machine = the engine of the routine that kept the name, by `engineName`), and `runs on the hub` on the dreaming row when this engine has peers and does not hold the lease. One muted line each, under the XTICK `waiting` line.
- Dream now dialog copy says where it runs (`dreamNowWhere`). No new settings.

## R1 (review round 1) — supersedes the lines above where they differ

- **MUST 1, B6 pending sends.** The base is now `{agreed, pending}`. A path this side sends is `pending` (the sent blob), not agreed. Next exchange: a peer still showing the old agreed blob never applied it (a dialer asleep after the acceptor's send, a failed apply) — the old base stands, the path reads as this side's one-sided change and is sent again: no swap, no revert. A peer showing the sent blob, or anything else, took it: the sent blob is the base (so its later edit is a one-sided change that wins). A path applied here is agreed at once. Test `a_send_the_peer_never_applied_is_sent_again_not_reverted` (the acceptor's edit, the dialer drops after the send, two more rounds converge, a third changes nothing). Old base files (a bare map, this branch only) read as "no base": newest-wins once.
  - **Residual 1 (R2, corrected):** a peer that dropped the send AND then edited its old copy is read here as "took it, then edited" (base = the sent blob), while its own base still holds the old blob. The two bases stay crossed: each side reads the other's version as the one-sided change, so the two versions can swap on every sync tick (300 s) until someone edits again; both versions stay in git history, nothing is lost, but the head flips. **Follow-up (the real fix):** each manifest carries the blobs it applied from this peer, so a side knows exactly which of its sends the peer took and both bases move together.
- **MUST 2, C7 one routine stays one.** Rule 1: two texts equal apart from their front-matter `engine:` and `created:` lines are one routine (`without_stamps`): XTICK racing claims of a plain file, a re-home by editing `engine:`, a tool re-save → last writer wins. Rule 2: the mirror asks the policy only on a path's **first contact** — the local, never-replicated table `<table>_shared` has no row for it. A path is marked shared when any peer row for it is applied here (any outcome, tombstones too), when this engine writes it while **every** paired peer is connected (R2: `Policy::all_peers_connected`; schedule's compares `mesh::paired_count` with the connected paired ids — with one peer offline, that peer's own same-named routine must still meet this one as a first contact, not be replaced by last-writer-wins), for both names of a rename, and at start for every row whose last writer is a peer (an upgraded engine). Only mirrors with a policy (schedules) keep the table. Tests: `schedule` `same_name_routines_keep_both_and_both_engines_agree` (racing plain claims, re-home, re-stamp → `Replace`; the first-contact pair still renames), `replica` `a_path_with_shared_history_stays_one_routine` (an edit here racing a claim-with-edit there after a shared history → one file), the first-contact `same_name_different_routines_keep_both_and_converge`, and (R2) `an_offline_peers_same_name_routine_is_kept_not_replaced` (three engines, `c` offline while `a` writes `x`: all three end with `x` = a's and `x-c` = c's).
  - **Ceiling:** a path written on this engine only while not every paired peer was connected, and never written by a peer since, has no mark here. If a peer then changes BOTH the body and the `engine:` line in one write (a re-home plus an edit made on that machine, or a claim racing a body edit), this engine reads it as first contact and keeps both. Rule 1 covers the common single-change cases (claims, re-homes). Closing it fully needs a per-peer acknowledgement of who holds a path (the replica's per-origin `remote_vv` mapped to the row's batch), which I left out.
- **A4, the mesh's last night.** The counts run from the later of this engine's last `Dreamt` and the last activity of another engine's dreaming session (`Store::routine_ran_elsewhere("dreaming")`: an unattended root titled `dreaming` or `dreaming · …`, owner ≠ this engine — the session rows replicate). A node that never dreamt counts from the hub's night, not from "ever". Tested in `counts_include_work_done_on_another_engine`.
- **A5.** A refusal or silence from the hub comes back as "Dream now runs on the hub, which said: …" / "… which did not answer (…); try again when it is back", code and `OwnerUnreachable` data kept; nothing runs here instead (the hub may have started). Tests: `a_forwarded_dream_now_runs_on_the_hub_with_the_forwarders_params` (the hub-side dispatch of the positional `[RunRequest]`, marked `Forwarded`: reaches dreaming, no second hop) and `the_hubs_refusal_of_dream_now_is_said_as_the_hubs`. The node-side `to_peer` hop itself is 183's, tested there.
- **C8.** `name_in_use` refuses nothing on a lone engine, and never for a stamp naming an engine that is not a paired peer (unpaired since, a restored backup: `Owner::Unknown`).
- **Single engine.** `hub_now` asks the mesh nothing without paired peers (so `dreaming.status` and the heartbeat make no mesh call), and the heartbeat skips the store recounts (its own events are all the work).
- **A2, `enabled:`.** When the file on disk is another engine's bytes (its `cwd`, its `engine:` stamp, or an `engine=` arg that is neither this engine nor the `home_engine` pin), the heal also sets `enabled:` from THIS engine's `dreaming.enabled` (on: the install runs only when enabled; off still goes through `disable_files`). A hand toggle on this engine's own file stays.

## Other calls
- Sensor header bumped to `dreaming 4` (the file is binary-owned and rewritten when it differs anyway).
- `tui` and the three other `mirror_at` callers changed only by the new `renamed_from: None` / `None` policy argument.
- Optional live check (two scratch engines) not run: the two-engine paths are covered by the replica two-engine mirror tests and the two-bundle knowledge tests; the forward of Dream now (`to_peer`) is the existing 183 path and is not exercised by a test here.

## Tests and gates

New tests (fail-before = the same test with the old behaviour put back for one run, then reverted):

| Item | Test | Old behaviour |
|---|---|---|
| A1 | `replica files::tests::local_names_never_leave_and_peer_rows_never_land` (both directions, old-engine row and tombstone, stored row no resurrect) | FAILED `published` |
| A2 | `dreaming tests::a_foreign_routine_file_heals_from_this_engine` | FAILED (lib.rs:490, foreign `cwd`/`engine` kept) |
| A3 | `dreaming sensor_rules::tests::rule_table` + `python_sensor_answers_the_same_table` (hub rows) | FAILED `a non-hub that reaches the hub holds` (both tables) |
| A4 | `dreaming signals::tests::counts_include_work_done_on_another_engine` | FAILED `(0, 0)` vs `(1, 1)` |
| A5 | `dreaming signals::tests::dream_now_goes_to_the_connected_hub_only` | new function |
| B6 | `knowledge sync::tests::after_a_partition_the_hub_wins_a_real_conflict`, `in_the_heal_window_the_conflict_waits_for_one_hub`, `a_one_sided_change_wins_even_against_the_hub` | first two FAILED (newest won); the one-sided test passes on the old code too (a guard: the hub must not revert it) |
| C7 | `replica files::tests::same_name_different_routines_keep_both_and_converge`, `schedule file::tests::same_name_routines_keep_both_and_both_engines_agree` | FAILED (B's routine overwritten by A's; `Replace`) |
| C8 | `schedule hub_tests::create_refuses_a_name_another_machine_uses` | FAILED (`None`) |
| D9 | `routines/helpers.test.ts` `routine notes: renamed on arrival, dreaming on a non-hub` | new function |

Gates (final tree):
- `mbx check -p replica -p schedule -p dreaming -p knowledge -p agent -p app -p tui -p rpc -p tools -p document -p theuth --tests`: 0 errors, 0 warnings.
- `mbx clippy` same set `--tests --no-deps -- -D warnings`: 0.
- `mbx test -p replica -p schedule -p dreaming -p knowledge -p agent`: run 1 — 4 agent failures, all environmental: `sensor::tests::{fs_exists_through_monty, monty_budgets_and_allowlist, pr_watch_type_checks}` and `loop_::runtime_bridge_tests::guessed_direct_calls_are_bridged_or_refused` picked up the installed `/opt/homebrew/bin/t` as the Python worker (`unsupported protocol version 6 (server supports protocol version 5)`); runs 2 and 3 with that directory off `PATH` (no worker found, so the worker-pool tests take their skip branch): all pass, 0 failed (agent 807, schedule 94, dreaming 67, knowledge 58, replica 21, plus the small binaries).
- Bindings export (`mbx test -p rpc --features ts --lib export_bindings`, 8 passed, 1010 files) → `tsc -b --noEmit` 0 errors → `vitest run src/components/routines src/lib/dreaming src/components/settings`: 19 files, 150 tests passed.

R1 (final tree):
- Fail-before with the old behaviour put back for one run: `a_send_the_peer_never_applied_is_sent_again_not_reverted` FAILED `flipped`; `a_path_with_shared_history_stays_one_routine` FAILED (`a\nv2` kept beside a rename); `same_name_routines_keep_both_and_both_engines_agree` FAILED `racing claims of a plain legacy file` (`KeepOurs` vs `Replace`).
- `mbx check` and `mbx clippy … -D warnings` on the same set: 0 / 0.
- `mbx test -p replica -p schedule -p dreaming -p knowledge -p agent -p rpc`, `/opt/homebrew/bin` off `PATH`: two runs under mbx's parallel test scheduler failed only `schedule watch_tests::cooldown_runs_from_the_fire_even_when_the_run_fails` (a sensor step answered `None` under load; the test is untouched by 185, passes 5/5 alone and 4/4 running the schedule binary alone); two runs with `MBX_SCHEDULER_TESTS=0`: all pass, 0 failed (1195 tests: agent 807, schedule 94, dreaming 67, knowledge 59, replica 22, …).
- No TS changed in R1 (bindings, tsc, vitest as above).

R2: `an_offline_peers_same_name_routine_is_kept_not_replaced` FAILED with the old "any peer connected" write-time mark (c's `x` replaced a's on a), passes now. check / clippy `-D warnings` on the set: 0 / 0. Replica suite: 23 passed. Schedule suite: 93 passed, 1 failed — the same `watch_tests::cooldown_runs_from_the_fire_even_when_the_run_fails`, a pre-existing flake (test from 2026-09-29, code untouched by 185) that also fails 1 run in 15 when the schedule test binary runs alone.
