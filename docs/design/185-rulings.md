# 185 rulings — routine home and dreaming on the hub

## Sky's report (2026-11-09 11:50, verbatim)

 "ok, it seems like routines need to be tied to a node as well, because node B's dreaming job got copied to node As dreaming job and so now it's broken. so 1st, routines need a home. 2nd, dreaming since knowledge is shared in nodes in a mesh, should only happen on the hub, if the hub is unreachable when the schedule occurs for dreaming, then it runs on the hub and on any node that can't reach the hub, hub wins in merge conflicts when they can reach each other again."

## Known when he asked
- Since 182 (v2.18.12), user routines carry an `engine:` stamp, and XTICK (c553c363, not yet released) claims legacy files.
- ~/.theuth/schedules/ is a replica file mirror between paired engines (facts/theuth-replica-file-mirror-reprojects-files-deleted-at-startup-2026-10-08).
- The suspected defect is that each engine's dreaming plugin writes the same file name, schedules/dreaming.md, so the mirror keeps one copy and A gets B's.

**Recon:** RoutineHomeRecon (read-only), started at 11:52.

## Recon verdict (RoutineHomeRecon, main b48578c2)
- schedules/ is a flat file mirror keyed by file NAME, newest-write-wins (replica/src/files.rs:199-208; apply.rs:555-562; the HLC wall time is the mtime, files.rs:243-271). The loser is replaced with no backup (files.rs:350-389).
- The dreaming plugin writes the same schedules/dreaming.md on every engine (dreaming/src/lib.rs:169-183). It holds engine-specific bytes: `when: … engine=<id>`, cwd = data dir, model, budget (lib.rs:105-133). It has no `engine:` stamp, and is re-patched at every start and settings save (night.rs:229-292).
  - Effects: A silently never ticks (Owner::Elsewhere), or the stamp and the sensor gate disagree and nobody dreams.
  - `dreaming.enabled:false` on one engine kills the other.
  - "Dream now" uses the peer's model and cwd.
- Other engine-written routines: autoresearch-* is stamped `engine: <self>` and fine. A dreaming Routine proposal is unstamped and written on a user press. Nothing else is engine-written.
- Knowledge is a git bundle synced by manifest (knowledge/src/sync.rs). The later commit second wins per concept file, and there is no winner-engine notion. "Hub wins" needs a per-peer last-agreed (base) manifest, so only both-sides conflicts are decided by the hub; a hub bool in the Manifest; and the rule in sync.rs newer()/files_for().
- Dreaming gate inputs are local-only bus events (signals.rs:205-257). So a hub whose user works only on a node would read "nothing new".
- On disconnect, the lone side elects itself hub (lease.rs:133-141), so `is_hub()` already covers "cannot reach the hub". The exception is `hub: never` engines: compare connected_ids with the lease holder.
- Recommended fix:
  - O4: plugin-owned dreaming.md stays local and is skipped by the mirror. Add an `engine: <self>` stamp, and self-heal cwd/engine at start.
  - Gate rows "not the hub" when peered && !hub.
  - Mesh-wide activity counts.
  - A 3-way hub-wins knowledge merge for both-sides conflicts.

## Sky's rulings (2026-11-09 12:14, ask_user)
- **same_name: "Keep both, rename one".** When two machines hold different routines with the same name, at pairing or later, the second becomes `<name>-<machine>` and the routines list says so. Creating a routine with a name already in use is refused with a clear message.
- **stopgap: "Wait for the fix".** Nothing is done on his machines now; the fix repairs both dreaming files on upgrade.
