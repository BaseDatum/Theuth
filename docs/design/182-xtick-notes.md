# XTICK: legacy (unstamped) routines fired on the wrong machine (182 D6 follow-up)

Reported 2026-11-09 on v2.18.12: two meshed engines, agent `x` only on A. `x-tick` sessions showed up in the sidebar on both engines.

## Cause

- `schedules/x-tick.md` on A is a legacy file: no `engine:` line (it predates 182 D6), and nothing ever stamps one. `tools.rs` keeps a replaced legacy file bare, and `Put` stamps new names only.
- Schedule files replicate. `runs` rows do not.
- On B, a legacy file fires on the hub (`fires_here`: `(None, _) => hub`), and since 182 B takes the lease at once by rank.
- B has no local `runs` row. An `every:` file with `last = None` is due now (`When::next`), so B fires it about 1 s after it lands, or once B becomes the hub.
- B's fire creates B's own unattended session titled `x-tick`. That session replicates to A, and the sidebar lists routine sessions from every engine.

B never rewrote A's sessions (`agent/src/store.rs` refuses writes to a session owned elsewhere). Each engine made its own.

## Fix (`crates/schedule/src/lib.rs`, `file.rs`; R1 after review)

Ownership (`Schedule::owner`):
- `Here`: stamped with this engine.
- `Elsewhere`: stamped with a paired peer, or the pairing is not known yet.
- `Unknown`: S8, stamped with an engine that is neither this one nor a paired peer.
- `Nobody`: legacy, no `engine:` line. XTICK changes the rules for `Nobody` only.

Definitions:
- "Local history": this engine has a `runs` row for the name (`latest()` at load; fires, skips and watcher gate rows all count), or has ticked or fired it since (`Entry::ran_here`, `last`).
- "Peered": the 183 R4 paired set, which leaves out this engine, tombstoned rows and quarantined rows. Once the endpoint is bound it is `mesh::has_peers()` (`Net::paired_ids`). Before that it is `mesh::has_stored_paired`, read at load, because the schedule plugin starts before mesh; that is the same filter as `Store::peers`, read-only. `has_stored_peers` counts raw rows and is left as it was for its other callers.
- "Me": `mesh::self_id()`; before that, `mesh::stored_self_id` (the identity file) read at load.

1. **Who claims a legacy file** (`Schedule::adopt`, every tick, only while peered):
   - **A non-hub claims at once** what it can tell is its own: a plain file it has local history for gets `engine: <self>`. An `agent:` file gets `engine: <e>` when `engines_with(agent)` names exactly one engine (`TaskCreator::engines_with` → `tasks::EnginesWith` → the roster's `DeskRouter::engines_with`).
   - **The hub claims nothing for `CLAIM_GRACE`** (150 s: 2 × TICK plus 30 s of replication slack). The grace counts from when the hub first saw the file unclaimed, or from when it gained the lease. During it, the hub does not fire the file either, even with history, so an engine that ran it gets to claim it first. If the file is still unclaimed when the grace ends, the hub claims it: for the agent's one engine when `engines_with` names exactly one, otherwise for itself, armed at `now + every`. This is the exit from every wait: a hand-written file, a history engine removed or offline. Nothing waits for good.
   - **The write.** The only change to the file is the `engine:` line (`file::set_engine`). The claim is written only when the file on disk still equals the bytes last read, so a replicated or hand edit inside the watcher's 300 ms debounce is never overwritten. One `info` line is logged per claim, and the write replicates.
2. **A legacy file fires nowhere while peered** until it is claimed (`fires_here`: `Nobody => hub && !peered`). A lone engine behaves as before.
3. **S8 is as 182 had it** (`Unknown => hub`): history is not checked, and the file is not claimed.
4. **No offline cover (R2).** A stamped `agent:` file whose engine is offline does not fire on the hub (`Elsewhere => false`), like any stamped file. The R1 cover was removed. "Not connected" covered a closed laptop or a 1 s blip, and every cover fire had a new `fire_ms`, which the ledger does not dedupe: about one task per interval while the engine was off, plus one when it came back. That engine's own `missed:` decides what runs when it returns.
5. **Replicated `every:` files arm at now + d** (`Entry::hold_first`). An `every:` file with no local `last` fires at once only when `made_here`: stamped with this engine (every create path stamps it, 182 D6), or legacy on a lone engine. Any other such file is held one interval, and so is one whose stamp just changed to this engine (a claim made elsewhere, seen in `rescan`).
6. **Surface.** `ScheduleInfo.waiting` (optional) is set while peered for a legacy file: `waiting for a machine to claim it`, plus ` (claims in Ns)` on the hub. `schedule_status` appends it to the line, and the routines list shows it as one muted line. The hub also logs once per file.

## What the user sees

- After the upgrade, the non-hub engines that ran a legacy routine claim it on their first tick (`engine:` appears in the file). From then on it fires only there, and no `x-tick` sessions are made on the other machine.
- On the hub, an unclaimed legacy routine shows "waiting for a machine to claim it (claims in Ns)" for up to 150 s, then the hub claims it.
- A lone engine is unchanged.
- **Release note:** in a mesh, upgrade the engines together, or first add `engine: <id>` by hand to old routines. During the grace, who claims a file follows the lease role: the hub claims a still-unclaimed file for itself once the grace ends.

## Calls and known ceilings

- **Brief item 4.** The brief said "fires on the hub only if NO paired peer has local history". R1 restores S8 for stamped files instead, as the reviewer asked. XTICK changes unstamped files only.
- **Racing claims.** Two non-hubs that both ran the same legacy file both claim it. Last writer wins in the replica, and the losing engine sees a peer's stamp and stops. Each may fire it once in that window. The hub never races: it waits out the grace.
- **A peer that is not upgraded yet** cannot claim. The hub claims the file for itself after the grace, and the claim then holds.
- **S8 first fire.** A file stamped with an unknown engine and with no local `last` first fires one interval out (`made_here` is false). A copied data dir carries its `runs`, so it is not affected.
- **`dreaming.md` is a legacy watcher.** It is claimed like any other file, and `update_routine_file` keeps the `engine:` line. The sensor's `engine=` arg still decides whether a night runs, as before.
- **Firing engine on routine sessions (optional item 5):** not done.
