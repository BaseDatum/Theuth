# 185 recon: routines need a home; dreaming on the hub (RoutineHomeRecon)

Read-only recon, 2026-11-09, at main b48578c2. This is the recon's report reproduced verbatim (it sent it twice, both drafts are kept).

(In this file, double braces are written with a space, as `{ {` and `} }`, so the harness does not try to expand secret placeholders.)

**VERDICT** (main b48578c2, read-only. I could not inspect A/B; this Mac is a lone engine.)
1. **Cause.** `schedules/` is a flat file mirror keyed by file name. The mirror is replica/src/files.rs:199-208 (`path TEXT PK`) and merges by per-column HLC last-writer-wins (apply.rs:555-562, types.rs:27-39). The wall part of the HLC is the file's mtime (files.rs:243-271).
2. dreaming writes the same name `schedules/dreaming.md` on every engine (dreaming/src/lib.rs:169-183). The bytes are engine-specific (lib.rs:105-133) and carry no `engine:` stamp (routine.md:1-16).
3. It also re-patches them at every start and settings save (night.rs:229-292, lib.rs:260).
4. At pairing the newer write wins on both engines. The loser is overwritten via temp+rename with no backup (files.rs:350-389).
5. **182's stamp is not the defect.** Stamped routines (schedule tool, editor, autoresearch) are fine. Design 136 only handled "two engines, one data dir" with `home_engine` plus the sensor's `engine=` (136-dreaming.md:366), and never met the replica.

**1. REPLICATION**
6. Table `schedules` in schedule.sqlite via `mirror_at` (schedule/src/lib.rs:3137-3143). It mirrors top-level files only; dotfiles and subdirs are never rows (files.rs:5-7,49-54,239). Row = (path, bytes, mtime_ms, deleted, hlc).
7. Same name, different bytes: the higher (wall, counter, origin) wins on both engines. Nothing merges. A delete sets `deleted=1` and removes the peer's file too (files.rs:368-376).
8. A plugin rewrite, a restart patch or `dreaming.enabled:false` replicates. The last one writes `enabled: false` into the shared file (lib.rs:193-206), and `disabled_by_us` (lib.rs:157,173) keeps the peer off.

**2. ENGINE-WRITTEN ROUTINES**
9. **dreaming.md** is written if absent. Otherwise 5 keys (budget, when, run_timeout, model, cap) are patched at EVERY start and settings save (lib.rs:148-186; night.rs:236-292,1391-1397), so it fights the replica.
10. Engine-specific fields:
    - `engine=<home_engine|self id>` in `when:` (lib.rs:117-119,255-259);
    - `cwd:` data dir (lib.rs:131), never re-patched;
    - `model:` resolved tidy tier (lib.rs:112-116);
    - budget, run_timeout, window/idle/heavy args from config;
    - `created:` stamped by the first loader (schedule/lib.rs:938-962).
11. It has no `engine:` stamp. `sensors/dreaming.py` (not mirrored) and `skills/dreaming.md` (mirrored, agent/src/supervisor.rs:11314, so mixed versions fight) are also rewritten at start (lib.rs:159-168).
12. **autoresearch-<slug>.md** is written at study start (autoresearch/src/schedule_file.rs:52-118,141-153). It is re-written at restart only if missing (actor.rs:5581-5593). `enabled:` is flipped on pause/resume (schedule_file.rs:163-172).
13. Its engine-specific fields are `cwd` (worktree), model, budget, run_timeout and the `when:` window. It IS stamped `engine: <self>` (schedule_file.rs:98-101). `disable_all` skips other engines' files (255-290). Only a same-slug clash would collide.
14. **`schedules/<name>.md` from a dreaming Routine proposal** is written on user press (night.rs:1495-1519). It is unstamped and refuses an existing name.
15. Nothing else writes routines.
    - The `schedule` tool and routine_draft accept stamp via render_args (schedule/src/tools.rs:155-161,185). The editor Put stamps at lib.rs:3024-3027. XTICK adopt stamps at lib.rs:1840-1904.
    - A grep of `schedule::DIR` and "schedules" hits only dreaming, autoresearch, app and rpc.
    - `first_run` writes skills only (theuth/src/main.rs:1331-1357). `install_builtins` writes sensors only (agent/src/sensor/mod.rs:422-468).

**3. DREAMING**
16. **Run decision.** The routine has `every: 30m`, `when: dreaming.py window= idle= heavy_* engine=`, `fire: while_true`, `cooldown: 3h`, `mode: skip`, `cap: mutating`, `budget`, `run_timeout: 45m` (routine.md:1-16). The sensor runs with the file's `cwd` (schedule/lib.rs:2183-2208).
17. Gate order (sensor.py:66-116, twin in sensor_rules.rs):
    - engine arg ≠ signals.engine → "another engine owns dreaming" (sensor.py:68-70; rs:46-50);
    - signals stale (>180 s);
    - window;
    - idle;
    - a desk is doing;
    - already dreamt this window;
    - nothing new;
    - battery.
18. `dreaming.enabled:false` makes plugin start flip the file to `enabled:false` and delete the sensor (lib.rs:230-243). The budget guard is the file's `budget:`. `Night::status` computes `budget_left_tonight_usd` (night.rs:166).
19. **Gate inputs are this engine's own bus events.** `sessions_since`, idle and desks are local, and Sync-sourced concept puts are excluded (signals.rs:205-257, esp. 245-255). A hub whose user works on a node would read "nothing new" (sensor.py:102-103).
20. **Writes.**
    - Knowledge: merge/supersede/archive/verify/retitle/put, one local git commit per op (knowledge/src/store.rs:1814,1946,1987,2008,2048).
    - Under `dreaming/`: state.json, ledger/, cases.jsonl, edits.jsonl, backups/, proposals.jsonl (night.rs:46-58,373-389; edits.rs:53-63). These never replicate.
    - Sidecars: `skills/learned/<skill>.md` (edits.rs:46-50), in a subdir the mirror ignores.
    - journal.md (journal.rs:33-36), mirrored into the `Dreaming` note.
21. `CreateNote` is hub-gated (document/src/actor.rs:1767-1776, hub.rs:114-122). A connected non-hub logs `dreaming: journal document not created: NotHub:<id>` (journal.rs:187). Recall reads the local store with no engine filter (agent/src/store/dreaming.rs:1-11).
22. **"Broken" on A after B's bytes land.**
    - (i) If B's copy carries `engine: B`, A is `Owner::Elsewhere` and never ticks (lib.rs:1915-1936). This is silent: no log, no skip row. A non-hub with local history stamps at once (lib.rs:1840-1904); the hub waits 150 s (lib.rs:920).
    - (ii) If A's bytes land on B, B stamps itself over A's `engine=A`. B ticks, but its gate says "another engine owns dreaming" (a runs row or last_check, not a log; lib.rs:2278-2345). Nobody dreams. The log shows `schedules: legacy routine stamped with the engine that runs it name=dreaming engine=<B>` (lib.rs:1889).
    - (iii) A foreign `cwd` gives `WARN sensor dreaming: mount /cwd → <path>` (agent/src/sensor/bridge.rs:168) and `events=no-signals` (sensor.py:126-131).
    - (iv) "Dream now" has no owner check and runs B's model/cwd (night.rs:1313-1319; lib.rs:1292-1301).
    - (v) Every restart re-patches the lines and the peer's gate flips. One engine's `enabled:false` kills the other.
23. Missing nights show as no `schedule: fired by: when (go) name=dreaming` (lib.rs:1363-1371) and no `watch committed name=dreaming` (lib.rs:2717).
24. **XTICK.** With peers, an unstamped file fires nowhere until claimed (lib.rs:1933). An engine with local history claims at once; the hub claims after 150 s. The claim ignores whose `engine=`, cwd and model bytes are in the file, so stamp and gate can disagree. XTICK did not touch the name collision.

**4. KNOWLEDGE**
25. It is not the record replica. It is a git-backed bundle synced by manifest on connect and every `sync_secs`=300 (knowledge/src/sync.rs:1-12,99-118; lib.rs:69-81). Unit = concept file, entry = (blob, commit seconds) (sync.rs:37-45,170-188).
26. Rule: the later commit second wins, ties go to the higher blob id (sync.rs:190-196). The loser stays in git history, and received files land as one commit `sync from <peer>` (sync.rs:223-269). A deletion applies only if the peer's tombstone is newer.
27. Two engines dreaming the same night: merge/archive/supersede are per-file rewrites (store.rs:1808-2003), so the later commit wins per file. A canonical and its stubs can come from different engines. Undo receipts hold local commit ids (night.rs:309-320).
28. There is no winner engine: the manifest has no origin or hub field. `home_engine` and the sensor `engine=` only decide who starts a night.
29. **"Hub wins" needs three things.**
    - A per-peer last-agreed manifest, to tell a both-sides conflict from a one-sided edit. Without it, hub-wins reverts node edits.
    - A `hub` bool in the Manifest (serde-default) from `mesh::is_hub()` (mesh/lib.rs:85). knowledge has no direct `mesh` dep.
    - The rule itself in sync.rs `newer()`/`files_for()` (190-215).
30. **Hub identity at merge time.** Sync fires on Connected (sync.rs:107), but election waits for the first heartbeat (mesh/lib.rs:793-797). Both sides may claim hub for a moment; `resolve` then picks by rank or lowest id (lease.rs:155-204).
31. So: act only when exactly one side claims hub, else keep newest-wins and retry on the next 300 s tick. If the lease moves often, pin via `dreaming.home_engine`.

**5a. ROUTINE HOME — options and risks**
32. **O1 per-engine name** `dreaming-<id8>.md` plus `engine: <self>` at creation. No replica or schedule change; peers hold inert copies (lib.rs:1929-1936).
    - Risks: ~8 hard-coded `dreaming` sites (lib.rs:136,169,194; night.rs:242,1263,1315).
    - `valid_name` is `[a-z0-9-]`, ≤40 chars (schedule/file.rs:762-769), and ids are 64 hex, so no dots and a short prefix.
    - The old shared file must be retired in place, never deleted at start (the mirror re-projects it, files.rs:11-14; a delete replicates).
    - Old engines keep writing `dreaming.md`.
33. **O2 non-replicated subdir.** Free for the mirror, but loader and watchers are flat (file.rs:774-798; lib.rs:3202). Most new schedule/RPC/UI machinery.
34. **O3 (engine,name) replica key.** Single-column PK (replica/lib.rs:104-105) shared by agents/skills/mcp mirrors plus wire format. Mixed-version breakage. Reject.
35. **O4 plugin-owned routines local.** A skip-list in `Mirror::scan` (files.rs:239), `project_locked` (351), the live-row loop (316-321) and `mirror_at` (55).
    - Roughly 15 lines.
    - Risks: older engines still publish `dreaming.md` rows, so apply must ignore them. Existing rows and tombstones need handling. A front-matter key is unsafe because of `deny_unknown_fields` (file.rs:437). It is a special-case list.
36. **Recommend O4 for dreaming.** Its state is already local (dreaming/, skills/learned/, sensors/). Also:
    - add `engine: <self>` to the template;
    - add `cwd` and `engine` to the patched keys (night.rs:229) so a damaged file self-heals;
    - keep autoresearch as is.
    Choose O1 only if Sky wants every machine's routine visible everywhere.
37. **Stopgap (unverified live):** identical `dreaming:` section on both engines (home_engine = hub id, same models.tidy/budget/run_timeout/window) so the patched lines are byte-equal. The hub's cwd and stamp stay in the file.

**5b. HUB-ONLY DREAMING + PARTITION RULE**
38. **Gate.** Add `hub` and `peered` to signals.json from `mesh::is_hub()` and `mesh::has_peers()`. Signals refresh every 60 s (signals.rs:39).
    - Add a hold row "not the hub" when `peered && !hub`, in both gate tables (sensor.py and sensor_rules.rs).
    - Drop the `engine=` arg; keep `home_engine` as a pin.
39. **"Cannot reach the hub".** The QUIC control connection is the liveness signal (mesh/lib.rs:596-600). On Disconnected, `recompute("disconnect")` elects the lone side itself (lib.rs:819-822; lease.rs:133-141). `is_hub()` (set at lib.rs:516-522) is therefore already true on every partitioned side, so no new reachability code is needed.
40. Exception: `hub: never` engines are filtered out of elections (lease.rs:133-136). For those, test `mesh::connected_ids()` (lib.rs:214) against `MeshStatus.lease.holder` (types.rs:273-281,323-334).
41. **Risks (b).**
    - Local-only gate inputs (item 19): the hub never dreams while Sky works on a node. Feed counts from the replicated store, which is a signals.rs change.
    - At heal both nights may be running. `resolve` releases the loser at once (lease.rs:157-162), but a 45-min night keeps writing, so the merge rule is mandatory.
    - Recall sees node sessions only after sessionsync. Cases, ledger and sidecars stay with the engine that dreamt.
    - Two `Dreaming` notes appear, one per engine (journal.rs:164-191).
    - A partitioned node starts ≤30 min after the cut (`every: 30m`).
    - The twin gate tables must stay in lockstep.
42. **Merge rule risks.**
    - Manifest change: old engines ignore the new field and still apply newest-wins, giving asymmetric results in mixed versions.
    - Base-manifest persistence.
    - The hub ambiguity window at heal.
    - A lease move between dream and merge can flip the winner.

**6. THIS MAC (lone engine c2d88a8b…, laptop, hub)**
43. `~/.theuth/schedules/dreaming.md` front matter:
    - `created: "2026-10-08T15:12:11+01:00"`
    - `every: 30m`
    - `when: dreaming.py window=01:00-06:00 idle=20 heavy_concepts=300 heavy_sessions=40 engine=c2d88a8b…9272`
    - `fire: while_true`, `cooldown: 3h`, `mode: skip`, `memory: clear`, `runtime: clear`, `quiet: true`, `approvals: false`, `cap: mutating`
    - `run_timeout: 45m`, `budget: 25 USD`, `max_runs: 100000`
    - `cwd: "/Users/Sky.Moore/.theuth"`, `model: openai/gpt-6.1-sol`
    - No `engine:` stamp, as expected on a lone engine.
44. In theuth.log.2026-11-03…09 there are no lines about engine, skip or not-here: no "another engine owns", "legacy routine", "not fired", "mount /cwd" or `dreaming:` warnings. Gate holds are never logged.
45. The only dreaming lines are `started plugin="dreaming"` at boots, slow-handler WARNs (Night::Status 31 s on 11-09), and one `schedule: fired by: when (go) name=dreaming` per night, each followed 16-39 min later by `watch committed`. Fired at (UTC): 11-03 02:01, 11-04 01:27, 11-05 01:13, 11-06 02:05, 11-07 02:25, 11-08 01:19, 11-09 02:23.
46. The "another engine holds the hub lease; serving stopped hub=None" lines are document::hub at boot, not dreaming.

**NOT VERIFIED:**
- Which file wins at first pairing (depends on mtimes).
- Whether replicated sessions emit local TurnFinished events (I inferred not).
- The QUIC idle timeout before `Disconnected`.
- A/B's actual state.
VERDICT (main b48578c2, read-only; every cite below re-read against the tree. A/B are not inspectable, and this Mac is a lone engine.)
1. Cause: `schedules/` is a flat file mirror keyed by file NAME, with newest-write-wins. Cites: replica/src/files.rs:199-208 (`path TEXT PK`); apply.rs:555-562 (per-column HLC LWW); types.rs:27-39; the wall part is the file mtime (files.rs:243-271). dreaming writes the same `schedules/dreaming.md` on every engine (dreaming/src/lib.rs:169-183). The bytes are engine-specific (`engine=<id>`, `cwd`, `model`, `budget`: lib.rs:105-133), there is NO `engine:` stamp (routine.md:1-16), and they are re-patched at every start and settings save (night.rs:229-292, lib.rs:260).
2. At pairing the newer write wins on both engines. The loser's file is replaced via temp+rename with no backup (files.rs:350-389). 182's stamp is not the defect: stamped routines (schedule tool, editor, autoresearch) are fine. Design 136 only solved "two engines, one data dir" (`home_engine` + sensor `engine=`, docs/design/136-dreaming.md:366) and never met the replica.

1. REPLICATION OF schedules/
3. Table `schedules` in schedule.sqlite via `mirror_at` (schedule/src/lib.rs:3137-3143). Top-level files only; dotfiles and subdirs are never rows (files.rs:5-7,49-54,239). Row = (path, bytes, mtime_ms, deleted, hlc). Same name + different bytes: the higher (wall, counter, origin) wins on both engines and nothing merges. A delete sets `deleted=1` and removes the peer's file (files.rs:368-376).
4. So any plugin rewrite replicates. That includes `dreaming.enabled:false`, which patches `enabled: false` into the shared file (lib.rs:193-206). The peer stays off because its sensor still exists (lib.rs:157,173).

2. ENGINE-WRITTEN ROUTINE FILES
5. **dreaming.md**: written if absent, else 5 keys are patched (budget, when, run_timeout, model, cap) at EVERY start and settings save (lib.rs:148-186; night.rs:236-292,1391-1397). That is why it fights the replica.
6. Its engine-specific fields: `when:` `engine=<home_engine|self id>` (lib.rs:117-119,255-259); `cwd` = data dir (lib.rs:131; never re-patched); `model` = resolved tidy tier (lib.rs:112-116); budget/run_timeout/window/idle/heavy from config; `created:` by the first loader (schedule/lib.rs:938-962). It has no stamp. `sensors/dreaming.py` (not mirrored) and `skills/dreaming.md` (mirrored, agent/src/supervisor.rs:11314, so mixed versions fight) are also rewritten at start (lib.rs:159-168).
7. **autoresearch-<slug>.md**: written at study start (autoresearch/src/schedule_file.rs:52-118,141-153), re-written at restart only if missing (actor.rs:5581-5593), `enabled:` flipped on pause/resume (schedule_file.rs:163-172).
8. Its engine-specific fields: `cwd` worktree, model, budget, run_timeout, `when:` window. It IS stamped `engine: <self>` (schedule_file.rs:98-101), and disable_all skips other engines' files (255-290). It collides only on a same slug.
9. **`schedules/<name>.md` from a dreaming Routine proposal**, on user press (night.rs:1495-1519): unstamped, and refuses an existing name.
10. Nothing else writes routines. The `schedule` tool, routine_draft and the editor stamp (schedule/src/tools.rs:155-161,185; lib.rs:3024-3027). `first_run` writes skills only (theuth/src/main.rs:1331-1357). `install_builtins` writes sensors only (agent/src/sensor/mod.rs:26-40,422-468). There is no other bundled watcher routine.

3. DREAMING END TO END
11. **Run decision**: `every: 30m`, `when: dreaming.py window= idle= heavy_* engine=`, `fire: while_true`, `cooldown: 3h`, `mode: skip`, `cap: mutating`, `budget`, `run_timeout: 45m` (routine.md:1-16). The sensor runs with the file's `cwd` (schedule/lib.rs:2183-2208).
12. **Gate order** (sensor.py:66-116 ≡ sensor_rules.rs): `engine=` ≠ signals.engine → "another engine owns dreaming" (sensor.py:68-70; rs:46-50); stale >180 s; window; idle; desk doing; already dreamt; nothing new; battery. `dreaming.enabled:false` makes start flip the file off and delete the sensor (lib.rs:230-243). The budget is the file's `budget:`; status is at night.rs:166.
13. Gate inputs are THIS engine's bus events only: attended-root TurnFinished, Harvest/Model puts, Sync puts excluded (signals.rs:205-257, esp. 245-255). A grep of crates/sessionsync/src finds no TurnStarted/TurnFinished; it publishes only SessionOwnerChanged/ContextChanged/ResumePending (owner.rs:174,286,351; sync.rs:192,683). A hub whose user works on a node therefore reads "nothing new" (sensor.py:102-103).
14. **Writes**: knowledge merge/supersede/archive/verify/retitle/put, one local git commit each (knowledge/src/store.rs:1814,1946,1987,2008,2048). `dreaming/` state, ledger, cases, edits, backups and proposals (night.rs:46-58,373-389; edits.rs:53-63) are never replicated. Sidecars `skills/learned/<skill>.md` (edits.rs:46-50) sit in a subdir the mirror ignores.
15. journal.md (journal.rs:33-36) is pushed to the `Dreaming` note, whose CreateNote is hub-gated (document/src/actor.rs:1767-1776; hub.rs:114-122). A connected non-hub logs `dreaming: journal document not created: NotHub:<id>` (journal.rs:187).
16. **"Broken" on A after B's file lands.**
    - (i) B's copy carries `engine: B` (a non-hub with history stamps at once, the hub waits 150 s: lib.rs:920,1840-1904). A is then Owner::Elsewhere and never ticks (lib.rs:1915-1936). This is SILENT: no log line and no skip row.
    - (ii) A's bytes land on B: B adds `engine: B` while `when:` still says `engine=A`. B ticks but its gate holds "another engine owns dreaming" (runs row/last_check only, lib.rs:2278-2345), so nobody dreams. The log shows `schedules: legacy routine stamped with the engine that runs it name=dreaming engine=<B>` (lib.rs:1889).
17. - (iii) A foreign `cwd` gives `WARN sensor dreaming: mount /cwd → <path>` (agent/src/sensor/bridge.rs:168) plus `events=no-signals` (sensor.py:126-131). "Dream now" has no owner check and runs B's model/cwd (night.rs:1313-1319; lib.rs:1292-1301).
    - (iv) Each restart re-patches and replicates, flipping the peer's gate, and one engine's `enabled:false` kills the peer.
    - Tell-tale: no `schedule: fired by: when (go) name=dreaming` (lib.rs:1363-1371) or `watch committed name=dreaming` (lib.rs:2717) on expected nights.
18. **XTICK on dreaming.md**: with peers an unstamped file fires nowhere until claimed (lib.rs:1933). An engine with local history claims at once, the hub after 150 s. The claim ignores whose `engine=`/cwd/model bytes are inside, so stamp and gate can disagree (the notes' residual). It does not touch the name collision.

4. KNOWLEDGE
19. Not the record replica: a git-backed bundle with a manifest exchange on connect and every `sync_secs`=300 (knowledge/src/sync.rs:1-12,99-118; lib.rs:69-81). Unit = concept file; entry = (blob, commit seconds) (sync.rs:37-45,170-188).
20. The newer commit second wins, tie goes to the higher blob id (sync.rs:190-196). The loser stays in that engine's git history. Received files land as one `sync from <peer>` commit (sync.rs:223-269). A delete applies only via a newer tombstone.
21. Two engines dreaming one night: merge/archive/supersede are per-file rewrites (store.rs:1808-2003), so the later commit wins per file. A canonical and its stubs can come from different engines. Undo receipts hold local commit ids (night.rs:309-320).
22. There is NO winner engine: the Manifest has no origin or hub field, and `home_engine`/`engine=` only choose who starts a night.
23. **"Hub wins" needs**:
    - a per-peer last-agreed manifest, else it reverts one-sided node edits;
    - a `hub` bool in Manifest (serde-default) from `mesh::is_hub()` (mesh/src/lib.rs:85; knowledge has no direct mesh dep);
    - the rule in sync.rs `newer()/files_for()` (190-215).
24. **Hub identity at merge time**: sync fires on Connected (sync.rs:107) but election waits for the first heartbeat (mesh/lib.rs:793-797), so both sides may claim hub; `resolve` then picks by rank, else lowest id (lease.rs:155-204). So apply the rule only when exactly one side claims hub, else keep newest-wins and retry on the next 300 s tick. Pin with `dreaming.home_engine` if the lease churns.

5a. ROUTINE HOME — OPTIONS AND RISKS
25. **O1 per-engine name** `dreaming-<id8>.md` + `engine: <self>` at creation: no replica or schedule change, and peers hold inert copies (lib.rs:1929-1936).
    - Risks: 6+ hard-coded `dreaming` sites (lib.rs:136,169,194; night.rs:242,1263,1315).
    - The editor and tool accept only `[a-z0-9-]` ≤40 (file.rs:762-769; lib.rs:3021) and ids are 64 hex.
    - The old shared file must be disabled in place, never deleted at start (the mirror re-projects it, files.rs:11-14; a delete replicates).
    - Old engines keep writing dreaming.md.
26. **O2 non-replicated subdir**: free for the mirror, but loader and watchers are flat (file.rs:774-798; lib.rs:3202), so it means schedule, RPC and UI work. Most new machinery.
27. **O3 (engine,name) replica key**: single-column PK (replica/src/lib.rs:103-105) shared by the agents/skills/mcp mirrors, plus a wire change and mixed-version breakage. Reject.
28. **O4 plugin-owned routines local**: a skip-list in Mirror::scan (files.rs:239), project_locked (351), the live-row loop (316-321) and `mirror_at` (55), about 15 lines.
    - Risks: older engines still publish dreaming.md rows, so apply must ignore them too.
    - Stored rows and tombstones need handling.
    - It is a special-case list, because a front-matter key breaks older engines (`deny_unknown_fields`, file.rs:437).
29. **Recommend O4 for dreaming**, since its state is already local (dreaming/, skills/learned/, sensors/). Also:
    - put `engine: <self>` in the template;
    - add `cwd` and `engine` to ROUTINE_KEYS (night.rs:229) so a damaged file self-heals at next start;
    - leave autoresearch as is.
    Choose O1 only if Sky wants every machine's routine visible everywhere.
30. Stopgap (unverified live): give both engines an identical `dreaming:` section (home_engine = hub id, same models.tidy/budget/run_timeout/window) so the patched lines are byte-equal. The hub's cwd and stamp stay in the file.

5b. HUB-ONLY DREAMING + PARTITION RULE
31. **Gate**: add `hub` and `peered` to signals.json from `mesh::is_hub()`/`has_peers()` (60 s refresh, signals.rs:39). Hold "not the hub" when peered && !hub, in both gate tables. Drop `engine=` and keep `home_engine` as a pin.
32. **"Cannot reach the hub"** = the QUIC control connection is down (mesh/src/lib.rs:596-600). On Disconnected, `recompute` makes the lone side elect itself (lib.rs:819-822; lease.rs:133-141), and `is_hub()` is set there (lib.rs:516-522). So every partitioned side already reads hub, with no new reachability code.
33. `hub: never` engines are filtered out of elections (lease.rs:133-136); for those, compare `mesh::connected_ids()` (lib.rs:214) with `MeshStatus.lease.holder` (types.rs:273-281,323-334). Do NOT add "hub covers an offline node": XTICK R1 removed it because it duplicated tasks.
34. **Risks**:
    - (1) Local-only gate inputs (item 13): the hub never dreams while Sky works on a node. Count from the replicated store instead, which is a signals.rs change.
    - (2) At heal both nights may run: resolve drops the loser at once (lease.rs:157-162) but a 45-min night keeps writing, so the merge rule is mandatory.
    - (3) Recall sees node sessions only after sessionsync; cases, ledger and sidecars stay with the engine that dreamt.
    - (4) Two `Dreaming` notes appear (journal.rs:164-191).
    - (5) A partitioned node starts ≤30 min after the cut.
    - (6) The twin gate tables must stay in lockstep.
35. **Merge-rule risks**: mixed versions ignore the new Manifest field, giving asymmetric results; the base manifest must be persisted; there is a hub ambiguity window at heal; a lease move between dream and merge can flip the winner.

6. THIS MAC (lone engine c2d88a8b…, laptop, hub)
36. `~/.theuth/schedules/dreaming.md` front matter:
    - created "2026-10-08T15:12:11+01:00", every 30m;
    - when `dreaming.py window=01:00-06:00 idle=20 heavy_concepts=300 heavy_sessions=40 engine=c2d88a8b…9272`;
    - fire while_true, cooldown 3h, mode skip, memory clear, runtime clear, quiet true, approvals false, cap mutating;
    - run_timeout 45m, budget 25 USD, max_runs 100000;
    - cwd "/Users/Sky.Moore/.theuth", model openai/gpt-6.1-sol.
    No `engine:` stamp, because adopt runs only while peered (lib.rs:1841-1852).
37. theuth.log.2026-11-01…09 has no engine, skip or not-here dreaming lines: no "another engine owns", "legacy routine", "not fired", "mount /cwd" or `dreaming:` warnings. Gate holds are never logged.
38. The only dreaming lines are `started plugin="dreaming"` at boots, slow-handler WARNs (Night::Status 31 s on 11-09), and one `fired by: when (go)` per night (UTC 11-03 02:01, 11-04 01:27, 11-05 01:13, 11-06 02:05, 11-07 02:25, 11-08 01:19, 11-09 02:23). Each is followed 16-39 min later by `watch committed`. The only "another engine" hits are document::hub boot lines with hub=None.

NOT VERIFIED: which file wins at first pairing (depends on mtimes); the QUIC idle time before Disconnected; A/B's real files.
