# 139 — Mesh durability: a turn survives the session changing engines

## 1. Status

Design v0 · 2026-10-08 · author: theuth orchestrator from recon R1
(`~/.theuth/briefs/2026-10-08/139-recon-R1.md`, design 68/69 as built) and R2 (`139-recon-R2.md`,
design 109 as built). Every `path:line-range` below was re-read in the worktree
`~/Develop/theuth-139-doc` cut from `origin/main` at `07f8e1f3`; R2's cites were taken at
`17769a5c`, which is an ancestor with no change under `crates/**/*.rs` between the two (`git diff
--stat 17769a5c 07f8e1f3 -- crates` touches only `models.json` and two desktop manifests), so they
are valid at this sha. Extends 109 (`docs/design/109-durability.md`, vocabulary kept: journal,
`ResumePlan`, `ResumeKind`, safe replay, drain, suspended), 68 (`68-sessions.md` §3–§6: owner,
epoch, take/release/demote, forwarding, log) and 69 (`69-devices.md`: devices, peers follow the
owner). Today's three single-engine fixes (`109-as-built-{BANNER,REPORT,LIVE}.md`) are the baseline
for `ResumeState`, `resume_reports` and `running`. Rulings D1–D19 come from the orchestrator's brief
and are written here as decisions; where the recon contradicts one, §12 says so. Abbreviations used
for paths after this paragraph: `store.rs` = `crates/agent/src/store.rs`; `dur.rs` =
`crates/agent/src/store/durability.rs`; `capture.rs` = `crates/agent/src/store/capture.rs`; `sup.rs`
= `crates/agent/src/supervisor.rs`; `resume.rs`/`drain.rs`/`waits.rs`/`acp_resume.rs` =
`crates/agent/src/supervisor/<name>.rs`; `loop_.rs` = `crates/agent/src/loop_.rs`; `resume_calls.rs`
= `crates/agent/src/loop_/resume_calls.rs`; `reaper.rs` = `crates/agent/src/durability/jobs.rs`;
`journal.rs` = `crates/agent/src/durability/journal.rs`; `mod.rs` =
`crates/agent/src/durability/mod.rs`; `lib.rs` = `crates/agent/src/lib.rs`;
`owner.rs`/`apply.rs`/`sync.rs`/`events.rs`/`ssync lib.rs`/`config.rs` =
`crates/sessionsync/src/<name>.rs`; `forward.rs`/`api.rs` = `crates/rpc/src/<name>.rs`;
`attention.rs` = `crates/roster/src/attention.rs`;
`tjobs.rs`/`pty.rs`/`approval.rs`/`ask.rs`/`registry.rs` = `crates/tools/src/<name>.rs`; `0033` =
`crates/agent/migrations/0033_durability.sql`; `0035` =
`crates/agent/migrations/0035_resume_reports.sql`; `peers.rs` = `crates/agent/src/peers.rs`;
`acp_child.rs` = `crates/agent/src/acp_child.rs`; `store/communication.rs` =
`crates/agent/src/store/communication.rs`; `durability_tests.rs`/`vision_fallback_tests.rs` =
`crates/agent/src/loop_/<name>.rs`; `ledger.rs` = `crates/tasks/src/ledger.rs`;
`roster/src/lib.rs`/`roster/handoff.rs` = `crates/roster/src/{lib,handoff}.rs`; `secrets/tools.rs` =
`crates/secrets/src/tools.rs`; `model.rs` = `crates/tui/src/model.rs`; `hud/queries.ts` =
`crates/desktop/ui/src/components/hud/queries.ts`; `actor_ref.rs` =
`~/.cargo/registry/src/*/kameo-0.22.2/src/actor/actor_ref.rs`. "R1 §n"/"R2 §n" cite the recon
reports; "109 §n"/"68 §n"/"69 §n" the design docs.

## 2. The question and the answer in one page

Sky, 2026-10-08 03:22: *"What's gonna happen if a running session is synced to another mesh node
while it's mid turn?"* and then *"this whole flow needs to be thought through and worked out."*
Three scenarios, today and after this design.

**Scenario A — sync: the owner is mid-turn and a replica receives the rows.** Today: every 109
journal table except `pty_screens` is in `capture::ATTACHED` (`capture.rs:17-34`), so the replica's
`turn_steps`, `tool_calls`, `questions`, `waits`, `jobs` and `resume_reports` mirror the owner's
within one log hop (R1 §3). Nothing acts at apply time — `receive_log` publishes only
`SessionOwnerChanged` and `ContextChanged{reason:"replicated"}` (`sync.rs:485-502`) and the agent
has no handler for either (R1 §1). But `load_session` on the replica is not routed
(`api.rs:3222-3241`) and `Agents::resume_state` reads the global `open_turns()` (`drain.rs:238-296`,
`dur.rs:554-564`), so the replica's desktop renders the owner's live turn as **"Interrupted by a
restart at hh:mm — resuming…"** with `resumed = false` (the local `running` flag, `drain.rs:282`),
the Attention inbox under `resume: ask` lists it as a `resume:` item (`attention.rs:1046-1055`), and
the item's `resume_turn` RPC runs **locally** (`api.rs:5909-5917`). After: `open_turns(Scope::Here)`
on every acting consumer (D1); `Loaded.resume.owner = Some("A")`, `here = false`, banner **"running
on A"** (D2); `resume_turn` on a non-owner is forwarded to A (D18); the Attention item is raised
only on A.

**Scenario B — take: the user types on B while A is mid-turn.** Today: `Take::run`
(`owner.rs:57-112`) asks A to `Release`; A's `release` is `unload(id); store.flush();
Ok(session_log_seq)` (`owner.rs:184-190`); `unload` is `tell_cancel(None)` + kameo `kill()`
(`sup.rs:5872-5887`) — the cancel is a mailbox message the immediately following `kill` may drop (R1
§2, kameo drops queued messages on abort), the turn task is a detached `tokio::spawn`
(`loop_.rs:1212-1215`), and when the cancel does land the stop reason is `"cancelled"`, not
`SUSPENDED`, because `rt.shutdown` is not cancelled (`loop_.rs:4527-4533`) → `finish_turn(..,
"cancelled")` (`loop_.rs:4578-4589`). Either way those rows post-date the `flush`, so they sit above
`last_seq`: B applies `set_owner(e+1)` first and drops them as `StaleEpoch` (`apply.rs:37-42`); A
orphans them at demote (`owner.rs:209-254`). **B's copy keeps the turn open at whatever shipped
before the flush, and B does nothing with it** — `Take::run` ends at the publish
(`owner.rs:103-111`); the turn is closed lazily on B's next input by `settle_open_turns` →
`finish_turn("superseded")` **without running anything** (`loop_.rs:3250-3304`, `:3292-3299`), or at
B's next boot by `resume_scan`. After: the owner side of the take drains that one session with
reason `release` and writes `turn_finished` or `suspended{"reason":"release"}` **before** `flush`
(D3); B, right after `set_owner`, runs `resume_turn` with cause `transfer` (D4) under the configured
`durability.resume` (D5); jobs/PTYs of A settle `lost` with "owner changed (ran on A)" (D6);
questions and waits re-arm on B with the same ids (D7); A unloads via the drain, never via a bare
`kill` (D19).

**Scenario C — replica boot: B restarts while A is mid-turn.** Today: `ActivityRegistered` runs
`reap_jobs → rebuild_waits → resume_scan → respawn_acp_children → deliver_spared_results`
(`sup.rs:2263-2282`), every one over the global `open_turns()`/`jobs_open()` (R2 §0). Under `resume
= auto`, B's `resume_scan` calls `resume_turn` for A's open turn (`resume.rs:65-90`, `:95-127`),
`Agents::session` spawns an actor with no `here` check (R2 §2.4), the actor publishes `TurnResumed`,
re-issues A's model call and **runs planned/read-only tools locally**; every journal write is
refused `NotOwner` (`store.rs:243-251`) and swallowed (`journal.rs:17-21`; R2 §3.5), the turn stays
open and is re-resumed at every boot. `reap_jobs` `killpg`s A's replicated `pgid` in B's pid
namespace (`reaper.rs:93-98`; R2 §6). `rebuild_waits` re-arms A's approvals on B
(`waits.rs:101-120`). After: every one of those consumers is `Scope::Here` (D1); `jobs.engine` gates
the reaper (D14); the replica resumes nothing and A's turn is untouched (test T1, §11).

What does not change: 109 §4.2's replay rules (byte-identical model re-issue, read-only re-run,
mutating `started` → interrupted), 68's fence (`WriteTx::commit` is still the only write-side
guard), 69's device transport. This design adds a *scope* to every actor of the journal, a *drain*
to the ownership path, a *cause* to the resume, and an *engine* to the rows that name a process.

## 3. Vocabulary

Terms are 68's and 109's; the new ones are marked.

- **Owner** (68 §2) — the engine holding the live actor; the only writer. `sessions.owner` (mesh
  `EngineId` text, `''` on an engine without a mesh identity), `sessions.epoch`
  (`0023_sessions_everywhere.sql:6-7`). Rows are created with `owner = me, epoch = 1`
  (`store.rs:992-1003`).
- **Epoch** (68 §2) — increments on every transfer; the commit fence refuses a write whose session's
  max-epoch owner is not `me` (`store.rs:230-251`); a replicated row with `epoch < fence` is
  `StaleEpoch` (`apply.rs:37-42`).
- **here** (68 §5) — computed per row, never stored: `owner.is_empty() || owner == me`
  (`store.rs:468`). On `SessionSummary`, `SessionOwnerChanged` (`lib.rs:2447-2458`) and — **new** —
  `ResumeState`.
- **Replica** — any engine that holds a session it does not own; it applies the owner's log through
  `ApplyTx` with capture off and no owner guard (`apply.rs:31-77`). "Hub" is 69's lease holder that
  serves devices; for this design a hub is a replica like any other unless it owns the session.
- **Transfer** (**new** word for 68 §4.2's take with a reachable owner) — `Release` → drain →
  `Released{last_seq}` → taker's cursor reaches `last_seq` → `set_owner(e+1)`. The old owner stopped
  on purpose; nothing above `last_seq` exists.
- **Takeover** (**new** word for 68 §4.2's take with an unreachable owner) — the taker could not
  reach the owner (`unseen_long_enough`, `owner.rs:159-178`, or `reason = user` skipping it,
  `:76-78`); the old owner may be alive and still executing; its tail becomes orphans at demote (68
  §4.3, `owner.rs:209-254`).
- **Cause** (**new**) — why a turn is being resumed: `crash | shutdown | update | transfer |
  takeover`. Today's `ResumeKind` is two-valued (`Interrupted | Suspended`, `mod.rs:210-215`) and
  the model-facing note spells it `kind="crash|shutdown"` (`loop_.rs:6991-7002`); §6.3 and §10
  define the extension.
- **Open turn** (109 §4.6) — `(session, turn)` whose newest non-partial `turn_steps` row is not
  `turn_finished` (`NEWEST`, `dur.rs:403-406`; `open_turns`, `dur.rs:554-564`). A `suspended` row
  keeps the turn open with `kind = Suspended`.
- **Plan** (109 §4.1) — `ResumePlan { turn, kind, interrupted_at, last, calls, partial }` built from
  the journal by `resume_plan` (`dur.rs:569-675`). Unchanged in shape except for the cause (§10).
- **Scope** (**new**) — which sessions a journal reader is asked about: `Scope::Here` (sessions
  where `here`) or `Scope::All`. Never implicit (D1).
- **Safe replay**, **drain**, **suspended** — 109 §4.2, §9.2 and §3.2 as built: `Drain` writes
  `suspended{reason}` per open turn before the window (`drain.rs:93-105`), the loop writes
  `suspended{"reason":"drain"}` instead of opening a model call (`loop_.rs:4796-4812`), and a cancel
  under the shutdown token becomes `SUSPENDED` (`loop_.rs:4527-4533`).

## 4. Invariants

Each with the mechanism that enforces it and the test (§11 Z) that proves it. 109's Resume Invariant
and I1–I7 stand; these are added on top.

| # | Invariant | Mechanism | Test |
|---|---|---|---|
| **I1 One executor** | At any instant at most one engine runs a given turn: the session's current owner. A replica never spawns an actor for a session it does not own on account of the journal. | `Store::open_turns(Scope::Here)` / `jobs_open(Scope::Here)` / `spared_child_results(Scope::Here)` filter on `sessions.owner IN ('', me)`; every acting consumer (D1 list, §5.1) passes `Scope::Here`; `Agents::session()` refuses a lazy load for `!here` rows when called from the resume/respawn paths (§5.1 item 9). The commit fence (`store.rs:243-251`) stays the backstop for writes. | T1 (replica boots mid-turn → zero `TurnResumed` on B, A's turn unaffected), unit `open_turns_here_excludes_foreign_owners` |
| **I2 Replicas display** | A non-owner answers `load_session`/`list_sessions`/`attention_list` from its copy and says who runs the turn; it never says "resuming". | `ResumeState { owner: Option<String>, here: bool }` (§9.1) filled by `resume_state` from the `sessions` row; the desktop banner branches on `here` before it reads `resumed`; `resume:` Attention items are built only from `OpenTurns(Scope::Here)`. | T7 (replica's `Loaded.resume.owner == "A"`, `here == false`; `resume_turn` on B is forwarded/refused) |
| **I3 Release is a drain** | The owner side of a transfer leaves the turn's journal in exactly one of two states before `Released` is sent: `turn_finished` (the loop reached its end inside the window) or `suspended{"reason":"release"}`; never a bare `turn_finished{cancelled}`, never an open turn without a `suspended` row. | `owner::release` calls `Agents::DrainSession { session, reason: "release", deadline }` (§6.1) instead of `unload`; `DrainSession` is the per-session body of today's `Drain` (`drain.rs:70-155`) — `suspended` row first, `Draining` told, `wait_idle`, then the actor is stopped — and `flush` follows it. | T2 (`suspended{release}` present on B before B becomes owner; A's `session_orphans` = 0), unit `release_drain_writes_suspended_or_finished` |
| **I4 Resume follows ownership** | The new owner resumes the taken open turn exactly once, immediately after it becomes owner, with the cause recorded. | `Take::run` → after `set_fence` → `Agents::ResumeTurn { session, turn, cause }` (§6.2 step 9); `OpenTurn(session)` is read on B's own copy after the cursor wait. | T2 (B publishes one `TurnResumed{cause: transfer}`), T3 (`takeover`) |
| **I5 Duplicate execution is opt-in** | A system-initiated takeover never auto-resumes; a user-initiated one does and tells the user the old owner may still be working. | `cause = takeover` ∧ `reason ∈ {write, orphan}` → Attention item + transcript note, no `ResumeTurn`; `reason = user` → `ResumeTurn` now (§6.3). | T3 (user), T4 (system: no `TurnResumed` until the Resume press) |
| **I6 Processes stay where they ran** | A job/PTY row is settled `lost` only by the engine that spawned it or, after a transfer, by the new owner marking it "owner changed"; no engine ever signals a pgid it did not spawn. | `jobs.engine` column (D14, §10); `reap` filters `engine = me` before `adopt/killpg`; the transfer path (§7.1) settles rows with `engine != me` via `job_settle_transferred` (no signal). | T6 (A's job shows `lost` on B with the owner-changed reason; A's local process group untouched), unit `reaper_ignores_foreign_engine_rows` |
| **I7 Waits and questions re-arm with the owner** | After a transfer the pending approvals/asks/handoffs/secrets of the taken turn reappear on the new owner's clients with the same ids; the old owner's in-memory copies die with its actor. | `rebuild_waits_for(session)` + `Republish` run inside resume-on-take (§7.3) using the replicated `questions`/`waits` rows; the old owner's `settle()` writes are refused by the fence (`approval.rs:162-176` via `journal.rs:17-21`, already today). | T5 (same `ask:<id>` listed on B; answer from B's client reaches the call) |
| **I8 Transcript equality modulo notes** | A turn transferred mid-batch yields, on the new owner, the transcript an uninterrupted run would have produced, differing only in the `<engine_restart>` note and the owner-change notes; a mutating `started` call ran at most once. | 109 §4.2 unchanged; the drain in I3 guarantees no call is `started` by A after `Released`; the fence drops anything A writes after `last_seq`. | T2 (`transcripts equal modulo notes`, mutating call count ≤ 1 across both fakes) |
| **I9 Scope is explicit** | No store reader of open turns/jobs/spared results has a default scope; the compiler, not a code review, catches a new global consumer. | `Scope` is a required parameter with no `Default`; the old zero-arg `open_turns()` is removed, not deprecated (clean cutover, §11 W1). | `cargo build` (no zero-arg callers remain); grep gate in Z |

## 5. Owner-scoped durability (D1, D2, D10)

### 5.1 Every consumer, today and after (D1)

The store's `open_turns()` is one SQL over `turn_steps` with no owner predicate (`dur.rs:554-564`);
`jobs_open()` likewise (`dur.rs:1162-1170`, doc comment "across sessions"). Because the journal
tables replicate (`capture.rs:17-34`), each of the readers below sees the owner's turns as its own.
The change is one join per query — `JOIN sessions s ON s.id = session_id WHERE (?scope = 'all' OR
s.owner = '' OR s.owner = ?me)` — and an explicit `Scope` at every call site:

```rust
// crates/agent/src/store/durability.rs (W1)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope { Here, All }          // no Default on purpose (I9)
impl Store {
    pub async fn open_turns(&self, scope: Scope) -> anyhow::Result<Vec<(SessionId, TurnId, DateTime<Utc>)>>;
    pub async fn open_turn(&self, session: SessionId) -> anyhow::Result<Option<(TurnId, DateTime<Utc>)>>; // one session, any owner (display)
    pub async fn jobs_open(&self, scope: Scope) -> anyhow::Result<Vec<JobRow>>;
}
```

| # | consumer | today (site, scope) | after |
|---|---|---|---|
| 1 | `resume_scan` | `resume.rs:65-90`, `open_turns()` global; `Auto` → `resume_turn` per row | `open_turns(Scope::Here)`; a non-owner never reaches `resume_turn`. R1 §8.2 / R2 §10.1–2 fixed by W1 |
| 2 | `resume_turn` | `resume.rs:95-127`: `resume_plan` → `self.session(Some(session), ..)` (no `here` check, `sup.rs:705-888`) → `actor.ask(Resume)` | gains `cause: ResumeCause` (§6.3) and an `ensure!(row.here)` before `self.session` — the second line of defence behind item 1 (the `ResumeTurn` message can arrive from the RPC and from `Take::run`). R2 §10.1 |
| 3 | `ResumeTurn` message | `resume.rs:31-37` `{session, turn}` | `{session, turn, cause}`; sent by `resume_scan` (cause from the plan: `crash` without a `suspended` row, else the row's reason), the `resume_turn` RPC (same rule), `Take::run` (`transfer`/`takeover`) |
| 4 | `OpenTurn(session)` | `resume.rs:41-58`, global read then `.find(session)` | `store.open_turn(session)` — one session, any owner; the RPC behind it decides routing (§9.2). Display read: allowed on a replica |
| 5 | `OpenTurns` | `drain.rs:54-68`, global; callers: attention `resume_ask` (`attention.rs:1046-1055`), schedule plugin (`crates/schedule/src/lib.rs:2614-2621`) | `OpenTurns(Scope)`; attention passes `Here` (the item acts: its button is `resume_turn`); the schedule plugin passes `Here` (it spares runs of sessions this engine will resume; a session owned elsewhere is not its run to keep open). R1 §8.5 |
| 6 | `Drain` handler | `drain.rs:93-105` writes `suspended` for **every** open turn in the file (a foreign one → `NotOwner` logged `error!`); `:127-135` publishes `TurnSuspended` for every still-open turn | both loops `Scope::Here` (D16). A replica's shutdown writes nothing for the owner's turn and publishes no `TurnSuspended` for it. R2 §10.5 |
| 7 | `rebuild_waits` | `waits.rs:101-120` global → re-arms approvals/peer waits of other engines' turns (R1 §8.4) | `Scope::Here`; plus a per-session entry point `rebuild_waits_for(session)` for resume-on-take (§7.3). R2 §10.13 |
| 8 | `reap_jobs` | `reaper.rs:55-137`: `jobs_open()` global, `adopt(pgid)` + `killpg` in the local pid namespace, `job_settle(Lost)` refused on a foreign session, `<job_result exit="lost">` injected into a session this engine does not own (R2 §6, §10.6) | `jobs_open(Scope::Here)` **and** `WHERE engine = me` (D14, §7.1): a row this engine did not spawn is never signalled, whatever its session's owner |
| 9 | `Agents::session()` lazy load | `sup.rs:705-888`: no `here` check; today the front door pre-checks ownership before any write RPC (`forward.rs:162-175`), and `inject`/`route` do not (`sup.rs:1302-1306`; R2 §6) | unchanged for display loads; the two journal-driven entry points (`resume_turn`, `respawn_acp_child`) check `row.here` themselves (items 2, 10). Making `session()` itself refuse `!here` would break forwarded desk steers that are served on the owner with `Forwarded` — the owner *is* `here` — so no change there; see §12 Q3 for `inject` |
| 10 | `respawn_acp_children` | `acp_resume.rs:24-59` global; `respawn_acp_child` gates on the local harness entry only (`:77-84`); inert in production because `waits(child_result)` has no writer (R2 §1.3, §7) | `Scope::Here` + the child's `row.here` + `cwd` readable (D13, §7.4); the writer is added by W3 |
| 11 | `deliver_spared_results` | `waits.rs:264-280`: `spared_child_results()` (`store/communication.rs:1421-1432`, `peer_exchanges`, an engine-local table — 68 decision 8) minus waits of global `open_turns()` | `open_turns(Scope::Here)` for the "awaited" subtraction; the spared rows are local by construction but are delivered only into sessions `here` — a result for a session taken elsewhere is forwarded through `Agents::deliver` → `remote_owner` (`sup.rs:4139-4142`, 69 P) |
| 12 | `Agents::resume_state` → `Loaded.resume` | `drain.rs:238-296`: global `open_turns()` filtered to the session; `resumed = running` is the **local** actor's flag (R1 §8.9) | reads `open_turn(session)` (any owner — it displays) and the `sessions` row; fills `owner`, `here`; `resumed` is meaningful only when `here` (§9.1) |
| 13 | Attention `resume:` items | `attention.rs:1040-1055`: live `ResumeReport`s (`self.reports`, never cleared — R2 §10.12) + `OpenTurns` under `ask` | `OpenTurns(Scope::Here)`; live reports are published only by the engine that resumed (I1), so they are already owner-scoped; the never-cleared `reports` map is §12 Q9 |
| 14 | `settle_open_turns` | `loop_.rs:3250-3262` global read filtered to own session | `open_turn(session)`; the actor exists only on the owner after D1, so the scope is implied; the call shape changes with the API |
| 15 | `journal_prune` | `dur.rs:1309-1348`, no caller (R2 §1.6) | `Scope::Here` added to the `fin` CTE; called by the D15 tick (§5.3) |
| 16 | `question republish` / `resume_question_call` | `resume_calls.rs:194-241` via `ResumeQuestion` — per call, reached only from a resumed batch | no scope change: reached only through items 1–3 |

Items 1, 5–8, 10–11, 13 are the "acting" consumers of D1; 4, 12, 14 are display or per-session reads
and keep an any-owner form.

**The owner backfill (D17).** `Store::open(path, Some(me))` runs `UPDATE sessions SET owner = ?
WHERE owner = ''` before the capture exists (`store.rs:552-558`). It is the 68 §8 migration case: a
store written before the engine had a mesh identity. It stays, with one `tracing::info!("68: claimed
{n} unowned session(s) as {me}")` when `n > 0` (W1). Hazard check against the recon: the backfill is
not a `begin_write`, so it logs no `session_log` row and never replicates — the claim is local, and
a peer that also claims `''` rows claims *its own copy*; two engines can thus own "the same" `''`
session without either fence noticing (R2 §10.11). This cannot happen through replication (a
replicated row always carries its origin's `owner`), only through a copied or restored file. No new
behaviour; the smallest guard is proposed in §12 Q6.

### 5.2 Replicas display, never act (D2)

`ResumeState` (`lib.rs:929-948`) gains `owner: Option<String>` (the `sessions.owner` text when
non-empty) and `here: bool`, both `#[serde(default)]` so old engines and old clients keep today's
shape (`here` defaults to `true` — a client talking to an old engine renders as before).
`resume_state` fills them from the `sessions` row it already has via `store.session(id)`. Wording by
state (§9.3): `here && resumed` → today's "resumed automatically"/"resumed"; `here && !resumed` →
"resuming…"/"resumes after the restart" (`ask` mode on the owner); `!here` → **"running on
\<owner\>"** for an open turn with no `suspended` row, **"paused on \<owner\> — resumes there"**
when the newest step is `suspended` with a drain reason, and **"taken from \<old\> at hh:mm —
resumed here"** is the *owner-side* banner after a transfer (driven by `TurnResumed{cause}`, §9.3).
When a replicated `resume_reports` row exists for the open turn it is attached as today
(`drain.rs:273-279`): the counts are the owner's and are true on every copy.

The `resume_turn` RPC on a non-owner routes to the owner (D18, §9.2); the Attention `resume:` item
is raised only on the owner (item 13). A replica's `running` flag never mentions a foreign session
(it is process-local, `sup.rs:2144-2149`), so `SessionSummary.running` is `false` there while
`Loaded.resume.owner` says who runs it; `discover_peers` already marks such rows `PeerState::Remote`
(`sup.rs:4700-4703`).

### 5.3 Journal retention and replication (D10, D15)

All 0033/0035 tables stay in `ATTACHED` (`capture.rs:17-34`): they are the plan the new owner needs
(109 §3.5 said so and this design relies on it). `pty_screens` **joins** the set (D14, §7.2) — the
one table 109 §3.5 kept engine-local — because the lost-PTY banner on a taker needs the owner's last
page (R1 §8.10); its writer is the owner by construction (the PTY lives there) and from now on the
fence applies to it like any attached table (R2 §1.4 noted it was the one unfenced journal write).

Retention runs where the rows are owned: a `journal_prune` tick on the owner with `Scope::Here` —
interval 1 h (ponytail: one `tokio::spawn`ed sleep loop in `supervisor::start`, the shape of
sessionsync's orphan sweep spawned at `ssync lib.rs:347-351` and its 60 s log-prune loop
`crates/sessionsync/src/prune.rs:20-30`; no config key for the interval,
`durability.journal_keep_days` for the cutoff as `mod.rs:36-37` already defines it) — and the
resulting `DELETE`s travel to every replica as ordinary log rows (the prune is a `begin_write`,
`dur.rs:1315`). A replica never prunes a foreign journal itself: without `Scope::Here` it would
write a `DELETE` under a foreign fence and be refused, so the scope is what makes the rule hold
rather than a second guard. The compaction-cut path (`cut_journal_in`, `dur.rs:184-204`) and the
session delete (`delete_journal_in`, `dur.rs:130-151`) are unchanged — both already run on the owner
because they are part of owner-only writes.

## 6. Transfer mid-turn (D3, D4, D5)

### 6.1 Release goes through the drain (D3, D16, D19)

Today's owner side of a take is `release(store, id)` = `unload(id).await; store.flush().await?;
Ok(session_log_seq(id))` (`owner.rs:184-190`), and `unload` is `Agents::UnloadSession` =
`drop_queued` → `sessions.remove` → `tell_cancel(None)` → `kill()` → `running.remove`
(`sup.rs:5872-5887`). Three defects, all R1 §8.7–8 / R2 §4.3: (a) `tell_cancel` is a mailbox message
and kameo's `kill()` "aborts the actor immediately; messages in the mailbox will be ignored and
dropped" (`kameo-0.22.2/src/actor/actor_ref.rs:298-308`), so whether the turn's cancel token fires
is scheduling luck, and the turn is a detached `tokio::spawn` (`loop_.rs:1212-1215`) that the
actor's death does not abort; (b) when the cancel does land the stop reason is `"cancelled"`, which
becomes `SUSPENDED` only under `rt.shutdown` (`loop_.rs:4527-4533`), so the journal closes the turn
as `turn_finished{cancelled}` (`loop_.rs:4578-4589`) — a *closed* turn the taker cannot resume; (c)
those rows are written after `flush`, above `last_seq`, and are dropped on the taker as `StaleEpoch`
(`apply.rs:37-42`) and orphaned on A at demote — so the taker's copy keeps the turn open at an
arbitrary earlier step whatever A did.

**New supervisor message** (contract C, body W2):

```rust
// crates/agent/src/supervisor/drain.rs
pub struct DrainSession { pub session: SessionId, pub reason: &'static str, pub deadline: Instant }
// ask → SessionDrainReport { open: bool, finished: bool, suspended: bool }
```

Its body is today's `Drain` handler (`drain.rs:70-155`) narrowed to one session, in the same order
and for the same reasons: (1) `open_turn(session)`; if open, write `turn_steps(suspended{"reason":
reason})` **first, durably** — a kill inside the window still leaves the resumable wording, and a
`turn_finished` written by a turn that makes it inside the window closes the row (`drain.rs:89-105`
comment); (2) `tell(Draining)` to the native actor (`drain.rs:109-112`) so no new model call opens
(`loop_.rs:4796-4812`); (3) `wait_idle` until the actor's `is_running()` is false or the deadline
(`drain.rs:168-186`); (4) `Jobs::KillSession{session}` — the per-session form of `KillAll`
(`tjobs.rs:1196-1237`): rows `exit = lost` first, PTY screens snapshotted, then `kill(pgids)`
(§7.1); (5) stop the actor: `tell_cancel(None)` then **`stop_gracefully()`** (kameo processes the
queued `CancelTurn` before stopping, `actor_ref.rs:282-290`) and `wait_for_shutdown()`;
`sessions.remove`, `running.remove`, `drop_queued` as today. `TurnSuspended{session, turn, reason}`
is published when the turn did not finish (today's `drain.rs:127-135` shape).

**Loop change (W2, one condition):** `loop_.rs:4530` becomes `if stop_reason == "cancelled" &&
(self.shared.rt.shutdown.is_cancelled() || self.shared.draining.load(SeqCst))` — a cancel that
arrives while this session is draining is a suspension, exactly like one under the shutdown token;
the turn returns before `finish_turn` (`loop_.rs:4572-4575`) and the `suspended` row written in step
1 stands. Without this line step 5's cancel would overwrite the suspension with
`turn_finished{cancelled}`.

**`release` becomes** (W2, `owner.rs:186-190`): `drain_session(id, "release",
release_window()).await; store.flush().await?; Ok(session_log_seq(id))`, where `release_window() =
Duration::from_secs(config.release_wait_secs.saturating_sub(1))` — the taker's `Released` timeout is
`release_wait_secs` (`owner.rs:119`, `:129-131`), so the owner must answer inside it: drain for all
but the last second, then flush and reply. `Msg::Release{session}` is unchanged on the wire
(postcard; adding a field would break older peers), so the window is the owner's own
`sessions.release_wait_secs`; the two engines' configs are assumed equal (§12 Q4). After `flush`
every row of the drain — the `suspended` step, the `lost` job rows, the PTY screens (now attached,
§5.3), the `TurnSuspended`'s cause — is at or below `last_seq`; the taker's `release_from` already
waits for its cursor to reach `last_seq` (`owner.rs:138-154`), **so the suspended row is on B before
B writes `owner = B`**. Nothing A writes after the reply can be anything but refused: step 5 stopped
the actor, and the fence moves with B's first row.

**Inputs during the drain (D16).** `Agents` gains `draining_sessions: HashSet<SessionId>` set at
step 1 and cleared at step 5. `tell_input` for a session in the set (and `Agents::inject`/`route`,
which reach `tell_input`, `sup.rs:1302-1306`) returns `Err(SessionDraining { session, reason })`;
the RPC layer maps it to a new retryable error `SESSION_DRAINING = -32011` with data `{reason,
next_owner?}` and the message *"<session> is moving to <taker> — retry in a moment"* (release) /
*"<session> is pausing for <reason> — resumes on the next start"* (other reasons). The composer
treats `-32011` like `-32010`'s retry path without a card (§9.3). For the **global** drain 109 §9.2
step 1's behaviour is kept: the input is accepted, journaled `input_accepted`, and the first `round`
writes `suspended{drain}` (`loop_.rs:4796-4812`; test `durability_tests.rs:1349`) — the fence does
not move at shutdown, so no turn opens under a moving fence there; §12 Q5 names the divergence from
D16's literal text.

**What `UnloadSession` does now (D19).** It stays for two callers and is made safe for both:
`demote` (`owner.rs:210`) — the foreign take has landed, the fence is already B's, A's actor must
die and anything it writes is refused by design; and the idle-session close (`DeleteSession`'s stop,
which the handler's comment cites, `sup.rs:5876-5877`). Its body swaps `kill()` for
`tell_cancel(None)` + `stop_gracefully()` + `wait_for_shutdown()` so the cancel is never dropped
(defect (a)); when a demoted turn is cancelled this way its `finish_turn("cancelled")` is refused
`NotOwner` (R2 §4.3), which is correct — B owns the turn and B's rows will overwrite A's copy. The
ownership path (`Take::run :93` on the taker, `release :187`) no longer calls it: the taker's own
`unload(id)` before `set_owner` (`owner.rs:93`) becomes `drain_session(id, "transfer-out", 0)` only
when a live actor exists locally for a session this engine does not own — which after D1 cannot
happen (I1) — so W2 deletes that line and the test
`take_from_a_reachable_owner_moves_the_tail_and_the_epoch` (`owner.rs:483`) gains an assertion that
no actor existed on B. A live turn outside the drain is no longer reachable from any ownership path.

### 6.2 Resume on take (D4)

Today `Take::run` ends at the publish (`owner.rs:103-111`); nothing on B opens the turn (R1 §8.6, R2
§10.3). After: step 9 below. Two sequences.

**Transfer (owner A reachable; B takes with `reason ∈ {user, write, orphan}`).** Rows in *italics*
are journal rows; `e` is the epoch before the take.

1. B: `Take::run` reads the fence `(e, A)` (`owner.rs:59-67`); A is in `connected` →
   `release_from(A, id)` (`:72-75`, `:117-155`): opens `theuth/sessions/1`, sends
   `Msg::Release{session}`.
2. A: `SessionSync::serve` dispatches `Release` → `answer_release` → `release` (`ssync
   lib.rs:186-188`) → `drain_session(id, "release", release_wait_secs − 1 s)`.
3. A: *`turn_steps(suspended{"reason":"release"})`* for the open turn (if any) — written under A's
   fence, epoch `e`, logged; `Draining` told; `draining_sessions += id`.
4. A: the loop's current `round` finishes its in-flight tool calls (*`tool_calls → finished`* per
   call, `registry.rs:1652`), reaches the next `round`, sees `draining` and returns
   `Round::Stop(SUSPENDED)` (`loop_.rs:4796-4812`) — or finishes the turn (*`turn_finished`*) if it
   was on its last reply. A turn still busy at the deadline is cancelled; the cancel is a suspension
   by the §6.1 loop change; its running calls stay *`started`*.
5. A: `Jobs::KillSession{id}` — *`jobs(state=running, exit=lost, settled_ms)`* per live job,
   *`pty_screens`* snapshot, then SIGKILL the groups (§7.1). `TurnSuspended{id, turn, "release"}`
   published (to A's clients and to B's follower over `EventRelay`, `events.rs:250-256`).
6. A: actor stopped (`stop_gracefully` + `wait_for_shutdown`); `flush()` (`store.rs:784-786`);
   `Released{last_seq}` sent (`owner.rs:186-190` shape).
7. B: `release_from` polls `cursors()` until `(A,'') ≥ last_seq` (`owner.rs:138-154`): every row of
   steps 3–5 is applied on B under epoch `e` (its fence is still `(e, A)`, `apply.rs:37-42` passes).
   B re-reads the fence (`:84-92`).
8. B: `set_owner(id, B, e+1)` (`store.rs:928-944`) — the first log row of epoch `e+1` — then
   `set_fence(id, e+1, B)` (`:99-102`) and `SessionOwnerChanged{here:true}` (`:103-110`).
9. **B (new):** `let turn = store.open_turn(id)?; if let Some((turn, _)) = turn {
   agents.tell(ResumeTurn { session: id, turn, cause: ResumeCause::Transfer }) }` — fire-and-forget
   after the publish so `Take::run`'s reply (`SessionOwner`) is not delayed by the resume.
   `ResumeTurn` → `resume_turn(session, turn, None, cause)` (`resume.rs:95-127` + cause):
   `resume_plan` finds the newest step `suspended{release}` → `kind = Suspended` and continues to
   the step before it (`dur.rs:599-614`); `self.session(Some(id))` spawns B's actor from B's copy of
   the frame (`sup.rs:705-888`); `actor.ask(Resume{plan, slots: None, cause})`.
10. B: `Turn::resume` → `drive(turn, Some(plan))`: `TurnResumed{.., kind: Suspended, cause:
    Transfer, from: Some("A")}` (`loop_.rs:4429-4438` + two fields), `rebase`, `resume_prelude`
    (`loop_.rs:3308-3354`): `ModelCall` → partial dropped and re-issued as `attempt + 1`;
    `ToolBatch` → `resume_batch` with 109 §4.2's per-call rule unchanged — `finished` rows reuse
    their output, `planned` run, `started` read-only re-run, `started` mutating → *`tool_calls →
    interrupted`* with the interrupted note; the `<engine_restart … kind="transfer" from="A"/>` note
    (§6.3) heads the first tool response; *`resume_reports`* row with `cause`; `ResumeReport`
    published.
11. A: B's `set_owner` row arrives (`apply.rs:52-64` raises A's `session_epochs` to `e+1`),
    `on_applied` → `Demote` (`owner.rs:259-283`) → `demote` (`:209-254`): `UnloadSession` (no actor
    — stopped in step 6), `log_orphan_after(id, e, shipped)` → **0 rows** (everything shipped before
    `Released`), `SessionOwnerChanged{here:false}`, no orphan note (`n == 0`, `:233`). A's clients
    see the owner chip move; A's `Loaded.resume` for the session now says `owner: B, here: false`
    (§5.2).

**Takeover (owner A unreachable; B takes).** Steps 1–7 are replaced by: `release_from` fails or A is
not connected (`owner.rs:72-83`): `reason = user` → warn and proceed without the tail (`:76-78`);
`reason = write` → `unseen_long_enough(A)` must pass (`:81-83`, `:159-178`) else `Err`; `reason =
orphan` → proceed (68 §14 "Orphan takes skip the 300 s gate"). B's copy of the turn is whatever
shipped before A went dark: possibly a `model_call` with a partial, possibly a `tool_batch` with
`started` rows, **no `suspended` row** — so `resume_plan` says `kind = Interrupted`. Then:

8′. B: `set_owner(e+1)`, `set_fence`, `SessionOwnerChanged` as above.
9′. **B (new):** `open_turn(id)` → `ResumeTurn{.., cause: Takeover { by: reason }}` **or** an
    Attention item, by D5 (§6.3). The `TakeReason` reaches here from `Take::run`'s argument
    (`owner.rs:57`).
10′. B (if resumed): as step 10 with `cause: Takeover`; every `started` mutating call of A's batch →
     `interrupted` (its effect on A is unknown — A may still be running it); `started` read-only →
     re-run; `planned` → run. The `<engine_restart kind="takeover" from="A"/>` note says so.
11′. A returns (or was partitioned): applies B's `set_owner` row → `demote`: `UnloadSession`
     **does** find a live actor if A was mid-turn and alive → `tell_cancel` + `stop_gracefully`
     (§6.1) → the cancel lands → `finish_turn("cancelled")`/`append_batch` are refused `NotOwner`
     (fence is `e+1`, B) → A's rows above `shipped` are orphaned (`log_orphan_after`, `:218-220`)
     and surfaced by the local note (`:233-251`) and `session_orphans`. Anything A's tools *did* in
     the meantime happened; 68 §4.3: "there is no merge, ever; the user chose B".

**Epoch fence interplay.** Three facts keep the two engines from both writing the same turn: (i) B's
`set_owner(e+1)` is the first row of the new epoch and is B's own write, so B's fence is `(e+1, B)`
from that commit on; any row of A's at epoch `e` that arrives later is `StaleEpoch`
(`apply.rs:37-42`); (ii) on A, applying that row raises `session_epochs` (`apply.rs:56-64`) so A's
`WriteTx::commit` refuses its own loop (`store.rs:243-251`); (iii) between A's `Released` and A's
receipt of B's row, A's actor is already stopped (transfer) — the only writer that could sneak a row
in is a *new* input on A, which D16 refuses while `draining_sessions` holds the id and which, after
the id is cleared, hits a session whose owner column still says A until B's row lands. That last
window is the `owner.rs:184-185` ponytail note ("a turn that starts between the kill and the taker's
row still writes and becomes an orphan on demote"); D16's set is cleared only when `demote` runs for
that session or after `2 × release_wait_secs`, whichever first — closing the window for the normal
case and bounding it for a taker that never writes its row. For a takeover there is no such window:
A was unreachable, and whatever it writes is orphaned.

**The partition hazard and D5.** A takeover cannot distinguish "A is dead" from "A is partitioned
and still executing the turn". If B resumes and A is alive, a `started` mutating call is not re-run
(109 §4.2) but a `planned` call *is*, and A will run it too when its batch reaches it: duplicate
execution. The journal cannot prevent this — only the user, who can see both machines, can judge.
Hence D5: a user-initiated takeover resumes at once and the card says what it means; a
system-initiated one (a desk wake through `MeshTaker`, `crates/tasks/src/ledger.rs:85-103`; a
forwarded write that fell into `Route::Take` on an unreachable owner, `forward.rs:201-221`) does
**not** resume: it raises an Attention item and writes a transcript note, and the user decides. The
take itself still happens in the system case (the engine needed to write), so the session is
consistent and writable on B; only the *open turn* waits.

### 6.3 Resume policy by cause (D5)

```rust
// crates/agent/src/durability/mod.rs (contract C)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeCause { Crash, Shutdown, Update, Transfer, Takeover }
impl ResumeCause { pub fn as_str(&self) -> &'static str; pub fn parse(&str) -> Option<Self>; }
```

`ResumeKind` (`Interrupted | Suspended`, `mod.rs:210-215`) stays: it is what the *journal* says (a
`suspended` row or not) and drives the plan (`dur.rs:599-614`). `ResumeCause` is what the *engine*
knows about why it is resuming and drives wording and policy. Derivation at the call sites:

| caller | cause |
|---|---|
| `resume_scan` at boot, `resume_turn` RPC | newest step `suspended{reason}` → `shutdown` for `shutdown`/`parent_gone`/`drain`, `update` for `update`, `transfer` for `release` (a transfer the taker never resumed — e.g. B crashed between step 8 and 9; B's boot finds the row and resumes with the true cause); no `suspended` row → `crash` |
| `Take::run`, release succeeded | `transfer` |
| `Take::run`, release failed or owner unreachable | `takeover` (with `TakeReason` beside it for the policy row below) |

The model-facing note `restart_note` (`loop_.rs:6991-7002`) spells `kind` from the cause — `crash |
shutdown | update | transfer | takeover` — instead of from `ResumeKind`, and gains `from="<engine>"`
for `transfer`/`takeover`; 109 §4.3's other notes are unchanged. `TurnResumed` gains `cause` and
`from: Option<String>` (§9.1); `resume_reports` gains a `cause` column (§10); `ResumeState` surfaces
it for the banner.

**Policy:**

| cause | initiated by | action on the new owner |
|---|---|---|
| `transfer` | any `TakeReason` | `durability.resume` mode as at boot (`mod.rs:60-66`): `auto` → `ResumeTurn` now; `ask` → Attention item `resume:<s>:<t>` with the button; `off` → nothing, the next input resumes lazily (`settle_open_turns` closes the old turn as `superseded` after answering its dangling batch from the journal — 109 §4.6 `off` semantics unchanged) |
| `takeover` | `TakeReason::User` ("Continue here" on the owner-unreachable card, TUI `/take`) | `ResumeTurn` now, regardless of `durability.resume` (the user just pressed the button that means "resume here"); the card's text (§9.3) says A may still be working and that whatever it did after the last shipped row is discarded (68 §4.3) |
| `takeover` | `TakeReason::Write` (front door on an unreachable owner, `forward.rs:201-221`) or `Orphan` (`MeshTaker`, `ledger.rs:85-103`) | **no `ResumeTurn`**: Attention item `resume:<s>:<t>` with detail *"<session> was taken from <A>; resume?"* (kind `Resume`, 104 §A3, the existing `ResumeNeed` shape `attention.rs:1049-1054`) and a local transcript note *"taken from <A> at hh:mm while a turn was running; press Resume to continue it here"* via `append_local_note` (`owner.rs:239-247` shape); the item's button is the existing `resume_turn` RPC, which on this engine (now the owner) runs `ResumeTurn{cause: Takeover}`. For the `Orphan` case the ledger's `Resume` wake (`crates/roster/src/lib.rs:1318-1324`) will send a new input into the session; `settle_open_turns` (`loop_.rs:3250-3304`) then answers the taken turn's dangling batch from the journal and supersedes it — the wake's input *is* the user's (the task's) decision, so this is consistent with "the user decides" and needs no special case |

`durability.resume = off` on the new owner with `cause = takeover, reason = user`: the user's
explicit press wins over the boot-time default; recorded as a rule, not a config knob.

## 7. Resources across a transfer (D6, D7)

Jobs and PTYs are not durable (109 R1) and were never engine-portable; questions and waits are rows
that replicate but live in process-local maps; ACP children are processes on one machine. Per item:
today / after / mechanism (existing or **new seam**) / test.

### 7.1 Jobs (D6, D14)

- **Today.** `jobs` rows replicate with the owner's `pgid` (`capture.rs:31`, `0033:77-92`); there is
  no engine column. `reap` (`reaper.rs:55-137`) lists every `running` row in the file, `adopt(pgid)`
  + `killpg(pgid, 0)` in the **local** pid namespace and SIGKILLs a live group (`:93-104`) — on a
  replica that is an unrelated local process group (R2 §6, §10.6) — then `job_settle(Lost)` (refused
  `NotOwner` on a foreign session, `error!`, the row stays `running` and is re-listed every boot)
  and builds `<job_result exit="lost">` for injection into a session it does not own (`:117-134`;
  `tjobs.rs:1240-1320` `Reaped` → `admit_job` → `Agents::inject`). On a take nothing touches A's
  jobs (R1 §6): they keep running on A, their rows stay `running` everywhere, and B's
  `resume_job_call` (`resume_calls.rs:52-105`) finds a `running` row for a `started` call and
  reports the job as if alive.
- **After.** Transfer: A's release drain settles the session's live jobs itself —
  `Jobs::KillSession{session}` (**new seam**, `crates/tools/src/jobs.rs`, the per-session body of
  `KillAll` `tjobs.rs:1196-1237`): `job.row(.., Running, Some(Lost), now)` per live job through the
  journal **first**, PTY `snapshot_to_store`, then `kill(&job.pgids)` and `task.abort()`; released
  charges as `KillAll` does. The rows are written under A's fence before `flush`, so B receives them
  settled, with `exit = lost` and `engine = A`. Takeover: the rows are still `running` with `engine
  = A`; B's resume-on-take runs `reaper::settle_transferred(store, session, from)` (**new seam**,
  `reaper.rs`): `UPDATE jobs SET state='lost', exit='lost', settled_ms=? WHERE session_id=? AND
  state='running' AND engine != ?me` — B is the owner so the write passes; **no `adopt`, no
  `killpg`** (I6). In both cases the rows reach B's `Jobs` actor as `Reaped { restarted_at, jobs }`
  (`tjobs.rs:439-444`) with `LostJob.reason = LostReason::OwnerChanged { engine: "A" }` (**new
  field** beside `row/screen/tail/deliver`, `tjobs.rs:447-454`; today's reaper sets
  `LostReason::Restart`), so `jobs()` lists them `lost` and `lost_body` (`reaper.rs:141`) renders
  *"owner changed (ran on A)"* in place of *"the engine restarted at hh:mm"*. The `started`
  `shell`/`start`/`send`/`wait` rows of the taken batch are reported by the job rule inside the
  resumed batch (`resume_job_call`, `in_batch` exclusion `reaper.rs:75-91`), the rest by injection
  as today.
- **`reap_jobs` at boot.** `jobs_open(Scope::Here)` and `WHERE engine = '' OR engine = ?me` (**new
  column**, §10): a row this engine did not spawn is never signalled, whatever its session's owner
  is now; a `running` row of a session owned here with `engine != me` (a takeover happened and the
  taker crashed before settling) is settled by the same `settle_transferred` path, not by the
  reaper's kill.
- **Test.** T6: a `shell` job started on A, session taken by B mid-batch → B's `jobs()` lists it
  `lost` with the owner-changed reason; A's process group is still alive after a *takeover* (B never
  signalled it) and dead after a *transfer* (A's `KillSession`); unit
  `reaper_ignores_foreign_engine_rows` (`reaper.rs` tests beside `reaper_kills_leaked_group`
  `:265`).

### 7.2 PTYs (D6, D14)

- **Today.** `pty_screens` is engine-local and unfenced (`0033:94-108`, `capture.rs:16`,
  `dur.rs:1183-1207`; R2 §1.4): a taker rendering `screen()` for a replicated lost PTY has no page
  (R1 §8.10), and the old owner can keep writing screens for a session it no longer owns (R2 §10.7).
  `lost_page` (`pty.rs:1162-1200`) prints *"[process gone — the engine restarted at hh:mm:ss; this
  is the last screen captured at … (revision N)]"*.
- **After.** `pty_screens` joins `ATTACHED` (`capture.rs:17-34`, one line, W1 contract) — the
  snapshot builder already filters every attached table by its session column
  (`crates/sessionsync/src/snapshot.rs:79-84`), and the FK to `jobs` holds on replicas because the
  job row precedes its screen in commit order. The fence now applies: only the owner writes screens
  (R2 §10.7 closed), and the write cadence (≤ 1 per `pty_snapshot_secs` = 5 s per live PTY,
  `mod.rs:35,49`) is the same order as `assistant_partial` upserts 109 §3.5 accepted. The transfer
  path snapshots every live PTY of the session before the kill (`KillSession`, §7.1), so B has the
  last page. `lost_page` gains the `LostReason` wording: *"[process gone — <session> moved from A at
  hh:mm:ss; this is the last screen captured on A at … (revision N)]"*; `SendInput` to a lost PTY
  keeps its "the terminal is gone (…); last screen follows" reply (`pty.rs:1476-1490`) with the same
  note.
- **Mechanism.** Existing: snapshot timer and `snapshot_to_store` (`pty.rs:642-668`), `Reaped` →
  `lost` map, `lost_page`. New: the attach line, the `LostReason` field, the wording arm.
- **Test.** T6 (PTY half): `start("bash -l")` on A, take by B → `screen()` on B returns the page
  with the moved-from-A banner; unit `pty_screen_rows_replicate` (inverts
  `journal_rows_replicate_but_screens_do_not`, `dur.rs:1825-1874`, which is deleted, not re-pinned).

### 7.3 Questions and waits (D7)

- **Today.** Rows replicate (`capture.rs:29-30`); the actors that answer are in-memory per engine:
  `Approval.pending/honoured` (`approval.rs:139-141`), `AskUser.pending` (`ask.rs:170`),
  `Agents.waits` (`sup.rs:174, 273`), `REARMED`/`REPUBLISH` statics (`waits.rs:34-41`,
  `registry.rs:1385-1398`) — R2 §5.2. `rebuild_waits` runs at boot only, over the global open set
  (`waits.rs:101-120`): a replica re-arms the owner's approvals and peer waits (R1 §8.4).
  `answer_ask`/`resolve_approval` RPCs are id-keyed into the local actors and not routed
  (`api.rs:3533-3583`; R1 §8.11): a question raised on A cannot be answered from B's front door.
  Attention `asks` come from the in-memory `PendingAsks` (`attention.rs:1016-1019`), not the table.
  The old owner's `settle()` after a take is refused `NotOwner` and swallowed (`approval.rs:162-176`
  → `journal.rs:17-21`).
- **After.** Re-arm at resume time, per session: `rebuild_waits_for(session)` (**new entry point**,
  `waits.rs`; the body of `rebuild_waits_with` `waits.rs:110-186` for one session) runs on the new
  owner as the first step of `resume_turn` for `cause ∈ {transfer, takeover}` — approvals: `pending`
  of a `planned` call → `question_resolve(Superseded)` (the gate asks again when the call runs,
  `registry.rs:1576-1601`), `answered` → `HonourAnswer` (`approval.rs:115-141`); then the resumed
  batch re-invokes every `started` question call through `resume_question_call` → `ResumeQuestion` →
  `registry::republish` → `Republish` with the **same id** and `deadline − now`
  (`resume_calls.rs:194-241`, `ask.rs:340-385`; handoff and secret tools via `take_republish`
  `roster/handoff.rs:189`, `secrets/tools.rs:209`), which re-inserts into B's `AskUser.pending` so
  `PendingAsks` and the desktop card show it on B's clients. The boot-time `rebuild_waits` becomes
  `Scope::Here` (§5.1 item 7). The old owner's maps die with the actor in §6.1 step 5 (`AskUser`
  entries are keyed by id and settle on the stopped call's `Drop`; approvals the same — existing
  behaviour, `ask.rs:189-204`, `approval.rs:162-176`), and any late `settle()` write is refused by
  the fence as today. Under D5's no-auto-resume case the rows stay `pending` on B until the user
  presses Resume — the Attention item for the takeover is the one card the user sees, and that is
  the design's intent (the question belongs to a turn the user has not yet chosen to continue). RPC
  routing for answers is §9.2 (D18).
- **`waits(peer_reply)` across a transfer** — a real limit, named here and in §12 Q7:
  `rearm_peer_wait` needs `exchange_receipt(id)` (`waits.rs:188-200`) from `peer_exchanges`, an
  **engine-local** table (68 decision 8); on B the exchange A recorded does not exist, so the wait
  is not re-armed and `resume_wait_call` → `ResumeWait` → "no row" → `interrupted_note`
  (`waits.rs:316-399`, `resume_calls.rs:249-305`). The reply itself is not lost: it is addressed to
  `session:<parent>` and `Agents::deliver` on the replying engine forwards it to the owner
  (`sup.rs:4355-4361`, 69 P), where it lands as an ordinary `<message>` input. The interrupted note
  for this case says so: *"waiting for a reply was interrupted by the transfer from A; the reply, if
  any, arrives as a message"* (wording only, `resume_calls.rs`).
- **Test.** T5: `ask_user` pending on A, take by B → `attention_list` on B lists `ask:<same id>`;
  `answer_ask` from B's client settles the call on B; A's copy of the row shows `answered` after
  replication; unit `rebuild_waits_for_one_session_leaves_others` (beside
  `superseded_on_reask_and_answered_row_honoured`, `waits.rs:517`).

### 7.4 ACP children and spawn waits (D6, D13)

- **Today.** `waits(child_result)` has no production writer (R2 §1.3; readers `acp_resume.rs:44-46`,
  `waits.rs:281, 355`) and ACP children journal no `turn_steps` (R2 §7), so `respawn_acp_children`
  (`acp_resume.rs:24-59`) finds nothing in production and an ACP child's interrupted turn is
  invisible to `open_turns`. `respawn_acp_child` (`:63-121`) gates on the local `agents.yaml` entry
  only (`:77-84`) — a replica with the same harness name would respawn another machine's child
  against an absent `cwd` (R2 §7 case A, §10.9). `spawn_subagent` is **non-blocking**
  (`crates/agent/src/peers.rs:248-266`: returns "Spawned … Its result will arrive as a
  `<message>`"); the child's result reaches the parent as a `result:<turn>` exchange via
  `child_finished` → `hand_off` (`sup.rs:1745`, `:1770`) and, across engines, `Agents::deliver` →
  `remote_owner` → `forward_peer` (`sup.rs:4355-4361`). `send_message{await_reply}` to a child is a
  `peer_reply` wait (`peers.rs:494-510`, §7.3). Children are not co-owned with their parent —
  `set_owner` moves one row (`store.rs:928-944`; R1 §8.12).
- **The lazy path that must keep working.** When any input reaches an ACP child session with no live
  actor, `Agents::session` → `acp_session` (`sup.rs:934-1113`) → `AcpChild::new(.., resume =
  store.acp_session(id), ..)` → `Link::open` tries `session/load <stored id>` in `cwd` and, on
  failure with the transport up, falls back to `session/new` with `resumed = false`
  (`crates/acp/src/harness.rs:484-561`, `:514-522`); `Connected` records *"previous harness session
  not restored; starting fresh"* and a pending `Respawn` composes the delta handoff
  (`acp_child.rs:1553-1627`, `:1148-1157`; `compose_handoff`
  `crates/agent/src/durability/handoff.rs:218-300` never fails). This design does not gate
  `acp_session` on ownership: a child owned here is legitimately loaded lazily, and a child owned
  elsewhere is never reached through this path — `Agents::deliver` forwards to its owner first
  (`sup.rs:4355-4361`) and the RPC front door forwards writes to a session with `agent` set
  (`forward.rs:162-175` desk branch). Only the *trigger* changes.
- **After.** (1) **The row is written** (**new seam**, W3, `sup.rs` `SpawnChild` handler): on
  success it writes `waits { session: parent, tool_call_id: <spawn call id>, kind: ChildResult,
  target: child, deadline: None, payload: {"agent": <name or null>} }` through `Store::wait_put`
  (`dur.rs:1057-1074`); it is cleared by `wait_clear` when the child's `result:` exchange is
  consumed by the parent (`child_finished`/`hand_off`, `sup.rs:1745-1770` area) or the child is
  deleted. Because the parent's turn is usually *finished* by then (the spawn returned), the trigger
  must not go through `open_turns`: `Store::child_waits(Scope::Here)` (**new**, `dur.rs`) selects
  `waits WHERE kind = 'child_result'` joined to `sessions` on the parent for the scope, and
  `respawn_acp_children` iterates that — §12 Q8 records that 109 §7.3 and D13 assumed the parent's
  turn is open. (2) **The gate**: `respawn_acp_child` requires, in order, the child's `row.here`
  (its session is owned here — a child whose owner engine is gone is *not* respawned elsewhere), the
  local harness entry (as today, `:77-84`), and `row.cwd` is a readable directory here; a refusal
  logs once. (3) **On a parent transfer** (`cause ∈ {transfer, takeover}`), `resume_turn`'s
  per-session pass walks the parent's `child_waits`: child owned by a *reachable* engine → nothing
  (its result will be forwarded, 69 P — existing mechanism); child owned by the engine the parent
  was taken from *and that engine is unreachable* (takeover) → the wait is settled as lost:
  `wait_clear` + an injected `<message from="session:<child>">` whose body is *"<child> was running
  on <A>, which is unreachable; its result is lost. "* followed by `compose_handoff(store, child,
  handoff_tokens)` when `row.cwd` is a readable directory here (the worktree is on a shared or
  synced path), else the plain note — delivered through `Agents::inject` (`sup.rs:1302-1306`,
  existing). A `send_message{await_reply}` to that child in the taken batch is the §7.3 peer-wait
  case.
- **Test.** Unit `child_result_wait_written_and_cleared` (`sup.rs` tests beside
  `peer_messages_follow_the_owner_and_replies_round_trip` `:12026`), `respawn_decision_table`
  (`acp_resume.rs:146`) gains rows for `!here` and unreadable `cwd`; T2 variant with a native child:
  the child's result spawned on A reaches the parent on B after the take (`two_engines()` helper
  `sup.rs:11936-11962`).

## 8. Devices, desks, children (D8, D9)

**Devices (D8).** A paired device talks to the **hub** over the relay
(`crates/relay/src/main.rs:1-3`, `crates/link/src/mesh_uplink.rs:1-11`) and its requests dispatch
into the hub's own `Methods` (`crates/link/src/conn.rs:471-476`) — the hub is the front door, the
device never addresses an engine (R1 §5). Reads (`load_session`, `list_sessions`, `attention_list`)
are answered from the hub's copy: after D2 `Loaded.resume` carries `owner`/`here` and the device
banner reads *"running on A"* for a session the hub does not own; `SessionSummary.running` is the
hub's process-local flag and stays `false` for it, which `discover_peers` already renders as
`PeerState::Remote` (`sup.rs:4700-4703`). Actions go to the owner by the existing forwarding
(`forward::route`, `forward.rs:182-228`) — with `resume_turn`, `answer_ask`, `resolve_approval`
added to the routed set (D18, §9.2). "Continue here" from a device is `session_take(session,
'user')` (`OwnerOfflineCard.tsx:30-32`), which takes to the engine the device is attached to — the
hub — exactly as 68/69 built it (69 §8 invariant 3 "session owned by A (the hub)"); D4/D5 then apply
on the hub: `cause = takeover, reason = user` → resume now, and the device sees `TurnResumed{cause:
takeover, from: A}` over the hub's event stream. Hub failover (69 §2) re-homes device connections,
not ownership: a session owned by the old hub is reached from the new hub as a non-owner until the
user types (take-on-write for a user session, `forward.rs:162-175` `Route::Take`) — and that take is
`TakeReason::Write`, so by D5 it does **not** auto-resume the old hub's open turn: the device gets
the Attention item. §12 Q10 asks whether a hub-failover take by a *typing user* should count as
user-initiated.

**Desks (D9).** A desk is owned by the engine whose roster opened it (`roster::open_desk` →
`CreateSession` → `owner = me`, `crates/roster/src/lib.rs:364-400`, `store.rs:992-1003`);
`desk_session(agent)` is per-engine (`store.rs:1051-1058`) and writes to a desk owned elsewhere are
forwarded, never taken (`forward.rs:169-174`). The one desk transfer is 67 §7.5's orphan sweep:
`MeshTaker` takes the lost engine's checkout session with `reason = orphan`
(`crates/tasks/src/ledger.rs:85-103`) and the roster routes the `Resume` wake into it
(`roster/src/lib.rs:1318-1324`). That is a system takeover under D5: no automatic resume of the open
turn; the wake's input arrives, `settle_open_turns` answers the dangling batch from the journal and
supersedes the turn (`loop_.rs:3250-3304`), and the task continues from the wake's text — which is
68 §4.4's intended semantics ("the progress note remains the fallback when the owner died before
shipping its tail"). The roster's live-child judgement reads the owner's `list_agents` and is
unchanged. Out of scope: wakes and claims in `tasks.sqlite`.

**Children (D9, D6).** Children are created on the engine that spawns them (`store.rs:992-1003`) and
are not moved by a parent's take (R1 §6, §8.12). After a parent transfer: native children keep
running on A and their `result:` exchanges reach the parent on B via `Agents::deliver` →
`remote_owner` → `forward_peer` (`sup.rs:4355-4361`; 69 §8 invariant 11 "peers follow the owner");
ACP children likewise (§7.4). A child whose engine is gone is reported lost to the parent (§7.4 step
3); it is never respawned on B. The parent's `running` glyph for children (109 LIVE) is per engine:
B's sidebar shows the children as remote rows, not as running subagents — §12 Q11.

## 9. RPC, events and UI (D11)

Every changed type, field, event, string and its TS-binding impact. Bindings are generated by `just
ts-desktop` (`justfile:94-97`: `bindings/` → `crates/desktop/ui/src/rpc/bindings/`); every
`#[serde(default)]` field below is backward-compatible on the wire — old engines omit it, old
clients ignore it.

### 9.1 Types and events (agent `lib.rs`, `durability/mod.rs`)

| item | today | change | TS impact |
|---|---|---|---|
| `ResumeCause` | — | new enum `crash \| shutdown \| update \| transfer \| takeover` (`mod.rs`, §6.3), `ts(export)` | new `ResumeCause.ts` |
| `ResumeState` (`lib.rs:929-948`) | `turn, kind, interrupted_at, resumed, report` | `+ owner: Option<String>` (`sessions.owner` when non-empty), `+ here: bool` (default `true`), `+ cause: Option<ResumeCause>` (from the `resume_reports` row or, for an open turn, derived as §6.3's boot rule); all `#[serde(default)]`, `ts(optional)` for the options | `ResumeState.ts` gains three fields |
| `AgentEvent::TurnResumed` (`lib.rs:2551-2564`) | `session, turn, kind, interrupted_at, resumed_at, partial_dropped` | `+ cause: ResumeCause`, `+ from: Option<String>` (the previous owner for `transfer`/`takeover`) | `AgentEvent.ts` variant grows; `reduce.ts:666-680` reads them |
| `AgentEvent::ResumeReport` | counts | unchanged (`lost_jobs` now populated by the §7.1 path — the zeros R2 §1.5 recorded become real for transfers) | none |
| `AgentEvent::TurnSuspended` | `session, turn, reason: String` | unchanged; `reason` gains the value `"release"` | none |
| `ResumeReport` row/type | `replayed, interrupted, lost_jobs, children_*` | unchanged shape; `resume_reports.cause` column (§10) rides `ResumeReportRow` | none on `ResumeReport.ts` |
| `SessionOwnerChanged` (`lib.rs:2447-2458`) | `session, owner, epoch, here` | unchanged | none |
| `FrontDoorError` (`forward.rs:53-62`) | `OwnerUnreachable{owner, last_seen}` | `+ SessionDraining { session, reason, next_owner: Option<String> }` with code `SESSION_DRAINING = -32011` beside `OWNER_UNREACHABLE = -32010` (`forward.rs:42`) | `FrontDoorError.ts` gains a variant; `lib/owner.ts` `ownerUnreachable()` gets a sibling `sessionDraining()` |
| `AttentionItem` (`attention.rs`) | kinds incl. `Resume` (`resume:<s>:<t>`) | unchanged kind; `ResumeNeed.detail` (`attention.rs:198-208`) carries the takeover text *"taken from <A>; resume?"* | none |
| supervisor messages | `ResumeTurn{session, turn}` (`resume.rs:31-37`), `OpenTurns` (`drain.rs:54-68`), `UnloadSession` | `ResumeTurn{.., cause}`, `OpenTurns(Scope)`, `+ DrainSession{session, reason, deadline}` (§6.1), `+ Jobs::KillSession{session}` (tools) | internal |

### 9.2 RPC routing per method (D18)

`forward::route(ext, session, method, params)` (`forward.rs:182-228`) is the one mechanism: `Here` →
local, `Take` (user session) → `TakeSession{Write}` then local, `Forward(owner)` (desk) → one
JSON-RPC line over `theuth/rpc/1`, `Forwarded` marker on the owner side. R1 §2 lists the 27 routed
methods; the ones this design touches:

| RPC | today | after | why |
|---|---|---|---|
| `load_session` (`api.rs:3222-3241`) | local, `authorize` only | **local, display-only** — the reply's `Loaded.resume.owner/here` names the owner (D2/D8) | a read; routing it would make every replica view depend on the owner being up, and 68 §5 ruled reads local |
| `list_sessions`, `list_agents`, `attention_list` (`api.rs:5893-5895`) | local | local; `attention_list`'s `resume:` items are owner-scoped by construction (§5.1 item 13) | reads |
| `resume_turn` (`api.rs:5909-5917`) | local: `OpenTurn` → `ResumeTurn` on the local `Agents` even on a non-owner | **routed**: `route(ext, session, "resume_turn", ..)`; `Route::Take` for a user session means the press *takes* the session here (`Write`) — and by D5 a `Write` take does not auto-resume, so the handler then sends `ResumeTurn{cause: Takeover}` explicitly: the user pressed Resume, which is the user's decision; `Route::Forward` for a desk sends it to the owner, where it runs with the boot-rule cause; owner unreachable → `-32010` card as for any write | an action; it must run on the owner (I1) |
| `answer_ask` (`api.rs:3558-3583`), `resolve_approval` (`:3533-3556`) | local, id-keyed into `AskUser`/`Approval` | **routed**: the session comes from the replicated row — `question_get(id)` (`dur.rs:1026-1035`) → `session_id` → `route(ext, session, method, params)`; no row → local as today (a pre-0033 question or a non-journaled kind); `Route::Take` for a user session is **not** taken here — an answer must reach the actor that holds the oneshot, which is the owner's; so for `Route::Take` the handler forwards exactly as for `Forward` (to the owner) and answers `-32010` if unreachable | R1 §8.11; D18's "answers MUST reach the owner" |
| `session_take` (`api.rs:3296-3323`) | local | unchanged (it *is* the transfer); its reply `SessionOwner{here: true}` is followed on the taker by the §6.2 step 9 resume | — |
| `send_input`, `steer`, … (27 routed) | `route` | unchanged; a `-32011 SessionDraining` reply is new for a session mid-release (§6.1) | — |
| `pty_screen`, `list_jobs` | local, in-memory actor | local; after a transfer the taker's `Jobs` actor holds the `lost` rows (§7.1) so `list_jobs` shows them and `pty_screen` serves the replicated page | — |
| `sessions_status` (68 §14) | local | unchanged | — |

### 9.3 Strings and surfaces (D11)

**Desktop chat banner** (`ResumeBanner.tsx:7-18` `resumeText`, state `sessions.ts:156-165`): the
`ResumeBanner` state gains `owner?: string`, `here: boolean` (default `true`), `cause?:
ResumeCause`, `from?: string`, copied from `Loaded.resume` in `resumeFromLoaded`
(`reduce.ts:467-476`) and from `TurnResumed` in `apply()` (`reduce.ts:666-680`). Wording, first
match wins:

| state | line |
|---|---|
| `!here`, kind `interrupted` | *Running on **A*** |
| `!here`, kind `suspended`, reason `release` | *Moving to **B**…* (seen on A's clients between steps 3 and 11 of §6.2) |
| `!here`, kind `suspended`, other reason | *Paused on **A** — resumes there* |
| `here`, cause `transfer`, `resumed` | *Taken from **A** at hh:mm — resumed here (N tool calls replayed, …)* |
| `here`, cause `takeover`, `resumed` | *Taken from **A** at hh:mm — resumed here; A may still have been working, its unshipped work was discarded* |
| `here`, cause `takeover`, `!resumed` | *Taken from **A** at hh:mm — resumes when you continue here* (the Resume button beside it is the existing `resume_turn` call, `hud/queries.ts:197`) |
| `here`, cause `crash`/`shutdown`/`update` | today's four strings unchanged (`ResumeBanner.tsx:9-17`) |

**Owner-unreachable card** (`OwnerOfflineCard.tsx:8-12`, `:30-32`): title unchanged (*Owned by A,
last seen 3m ago.*); when `Loaded.resume` shows an open turn the body adds *"A was mid-turn.
Continue here and resume — A may still be working; whatever it did after the last synced step will
be discarded."* and the button reads **Continue here and resume**; the click is the same
`session_take(session, 'user')` + resend, and the engine does the resume (D5). **Attention item**
for a system takeover: kind `Resume`, title *"<session title> was taken from A"*, detail
*"resume?"*, button → `resume_turn`. **Sidebar owner chip**: unchanged (68 §10). **Composer**:
`-32011` → inline *"moving to B — retrying…"* with one automatic retry after 2 s, no card.

**TUI**: the existing divider (`crates/tui/src/model.rs:6388-6405`) gains the owner name — `resumed
after transfer from A` / `resumed after takeover from A` as `label`, and the status note *"taken
from A at hh:mm — resumed"*; `owner_offline_line` (`model.rs:11402-11405`) appends *" — a turn was
running on <owner>; /take resumes it here"* when the loaded session has an open turn; nothing more.

**Model-facing**: `<engine_restart at after kind="transfer|takeover" from="A" replayed
interrupted/>` (§6.3); job/PTY notes per §7.1–7.2; the child-lost message per §7.4; the peer-wait
interrupted note per §7.3. All other 109 §4.3 wording unchanged.

## 10. Store and migration

**Migration `0038_mesh_durability.sql`** (the latest is `0036_double.sql`; `0033`/`0035` are 109's)
— two columns, no new table:

```sql
-- 139 D14: which engine spawned the process; the reaper signals only its own.
ALTER TABLE jobs ADD COLUMN engine TEXT NOT NULL DEFAULT '';
UPDATE jobs SET engine = (SELECT owner FROM sessions s WHERE s.id = jobs.session_id);
-- 139 §6.3: why the turn was resumed, beside 109's kind.
ALTER TABLE resume_reports ADD COLUMN cause TEXT NOT NULL DEFAULT '';   -- crash | shutdown | update | transfer | takeover; '' = pre-139 row
```

Both run before the capture session exists (`store.rs:539-540`: "DDL on an attached table would
leave the session unusable"), like 0033/0035 did. The backfill makes `engine` = the row's session
owner at migration time (D14); `''` on a lone engine matches `me = ''` the way `here` does
(`store.rs:468`). `jobs` is already attached (`capture.rs:31`), so from the next write on the column
replicates; a replica that has not migrated yet cannot apply a changeset with the extra column —
this is the same constraint 0033/0035 had (every engine of a mesh must run the migrated binary
before the first post-migration write ships), §12 Q12.

**`capture::ATTACHED`** (`capture.rs:17-34`): `+ ("pty_screens", "session_id")` → 17 entries; the
doc comment's "except `pty_screens`" clause and `0033:94-95`'s "Engine-local" comment are deleted;
`dur.rs:1183-1184`'s doc line is rewritten.

**Scope API** (`dur.rs`, W1 contract; §5.1):

```rust
pub enum Scope { Here, All }
impl Store {
    pub async fn open_turns(&self, scope: Scope) -> Result<Vec<(SessionId, TurnId, DateTime<Utc>)>>;
    pub async fn open_turn(&self, session: SessionId) -> Result<Option<(TurnId, DateTime<Utc>)>>;
    pub async fn jobs_open(&self, scope: Scope) -> Result<Vec<JobRow>>;           // + WHERE engine IN ('', me) for Here
    pub async fn child_waits(&self, scope: Scope) -> Result<Vec<WaitRow>>;        // kind = child_result, scoped by the parent's owner
    pub async fn journal_prune(&self, scope: Scope, keep_days: u32) -> Result<u64>;
    pub async fn jobs_settle_transferred(&self, session: SessionId, me: &str, now_ms: i64) -> Result<Vec<JobRow>>; // §7.1, no signal
}
```

`Scope::Here` expands to `JOIN sessions s ON s.id = <table>.session_id AND (s.owner = '' OR s.owner
= ?me)` bound to `self.me` (`store.rs:700-702`); `Scope::All` adds nothing. `JobRow` gains `engine:
String`; `job_put` writes `self.me` into it (the `tools::Journal` `job()` seam passes the row,
`journal.rs`, so `StoreJournal` stamps the engine — the tools crate never learns the engine id).
`ResumeReportRow` gains `cause: ResumeCause`. `spared_child_results`
(`store/communication.rs:1421-1432`) needs no scope: `peer_exchanges` is engine-local. No change to
`WriteTx::commit`, `session_epochs`, `session_log` or the fence.

## 11. Waves

Contract-first (the 2026-10 wave procedure, as 109 §12): one contract omp **C** lands the types,
signatures, migration, stub files and every mechanical call-site change so the tree compiles and the
four waves code against a fixed API; then **W1 ∥ W2 ∥ W3 ∥ W4** on disjoint files; then **Z** writes
the matrix; a **closer** integrates. Two waves never touch the same file; where a wave needs a line
in another wave's file, C has already put the call there as a stub and the owning wave keeps it.
Every wave's brief carries this table verbatim. Order of landing: C → (W1 ∥ W2 ∥ W3 ∥ W4) → Z →
closer.

### C — contract

| | |
|---|---|
| **owns** | `crates/agent/migrations/0038_mesh_durability.sql` (new, §10); `crates/agent/src/store/durability.rs` (`Scope`, `open_turns(Scope)`, `open_turn`, `jobs_open(Scope)` with the engine filter, `child_waits(Scope)`, `journal_prune(Scope, keep_days)`, `jobs_settle_transferred`, `JobRow.engine`, `ResumeReportRow.cause`, `pty_screen_put` doc line; the zero-arg forms **deleted**; test `open_turns_here_excludes_foreign_owners` + `jobs_open_here_filters_by_engine` in its test module); `crates/agent/src/store/capture.rs` (`+ pty_screens`, doc comment); `crates/agent/src/durability/mod.rs` (`ResumeCause` + `as_str/parse`); `crates/agent/src/durability/journal.rs` (`StoreJournal::job` stamps `engine = store.engine()`); `crates/agent/src/lib.rs` (`ResumeState.{owner,here,cause}`, `TurnResumed.{cause,from}`, `+ AgentEvent::ResumePending{session, turn, from}` in `session()`); `crates/rpc/src/forward.rs` (`SESSION_DRAINING = -32011`, `FrontDoorError::SessionDraining`); `crates/tools/src/jobs.rs` **types only**: `KillSession{session}` message + handler stub (no-op), `LostReason::{Restart, OwnerChanged{engine}}`, `LostJob.reason` (default `Restart`); new stub files `crates/agent/src/supervisor/transfer.rs` (`DrainSession` message + `pub async fn drain_session(..)` that today calls `UnloadSession`; `pub fn release_window(cfg)`; `pub async fn on_resume_here(agents, session, cause, from)` no-op) and `crates/agent/src/supervisor/child_waits.rs` (`child_spawned`, `child_result_delivered`, `settle_on_transfer` no-ops); one-line hook calls at the named insertion points: `sup.rs` `SpawnChild` success arm → `child_waits::child_spawned`, `sup.rs::hand_off` → `child_waits::child_result_delivered`, `resume.rs::resume_turn` after `resume_plan` → `transfer::on_resume_here`; `Resume { .., cause }` (`loop_.rs:1287-1292`) and `ResumeTurn { .., cause }` (`resume.rs`) fields with every caller passing the §6.3 boot rule; every `open_turns()`/`jobs_open()` caller updated to the §5.1 scope; `agent` Cargo feature `test-support` exposing `loop_::durability_tests::{BatchFake, Harness}` and `supervisor::tests::agents_on` as `pub mod testing` (for Z); `just ts-desktop` regenerated bindings committed |
| **non-goals** | any behaviour beyond the scope filter and the stamps: no drain change, no resume-on-take, no UI text, no reaper logic |
| **gates** | `cargo build --workspace`; `cargo test -p agent --lib -- store::durability supervisor::` ; `cargo test -p sessionsync -p rpc -p tools --lib`; `cargo clippy --workspace --tests -- -D warnings`; `just ts-desktop` + `bun run check` in `crates/desktop/ui` |
| **DONE** | the tree compiles with no zero-arg `open_turns`/`jobs_open` left (`git grep -n 'open_turns()' crates` → nothing); single-engine tests unchanged in count; `0038` applies to a copy of a live store; stub files exist with the signatures above and doc comments naming their wave |

### W1 — OWNER-SCOPE (D1, D2, D13 gate, D17)

| | |
|---|---|
| **owns** | `crates/agent/src/supervisor/resume.rs` (`resume_turn`: `ensure!(row.here)` before `self.session`, cause → `Resume`, `rebuild_waits_for` + `transfer::on_resume_here` ordering; `OpenTurn` via `open_turn`); `crates/agent/src/supervisor/drain.rs` (`OpenTurns(Scope)`; `Drain` both loops `Scope::Here`; `resume_state` fills `owner`, `here`, `cause`); `crates/agent/src/supervisor/waits.rs` (`rebuild_waits` → `Scope::Here`; `rebuild_waits_for(session)`; `deliver_spared_results` subtraction over `Scope::Here`, forwarding for a non-`here` recipient via `Agents::deliver`); `crates/agent/src/supervisor/acp_resume.rs` (iterate `child_waits(Scope::Here)`; gates `row.here` + local harness + readable `cwd`; `respawn_decision_table` rows); `crates/roster/src/attention.rs` (`OpenTurns(Scope::Here)`; `ResumePending` → `reports` insert with the takeover detail; `TurnResumed`/`TurnFinished` → `reports.remove(session)`); `crates/schedule/src/lib.rs` (`OpenTurns(Scope::Here)`); `crates/agent/src/store.rs` (the one `info!` on the backfill count, `store.rs:552-558`) |
| **non-goals** | the drain body, `UnloadSession`, `Take::run`, anything in `loop_.rs`, the reaper, tools, RPC, UI |
| **gates** | `cargo test -p agent --lib -- supervisor::resume supervisor::drain supervisor::waits supervisor::acp_resume`; `cargo test -p roster -p schedule --lib`; clippy on the crate |
| **DONE** | with two stores where B holds A's open turn, B's `resume_scan`, `rebuild_waits`, `respawn_acp_children`, `Drain` and `OpenTurns` touch nothing of A's (unit tests in each file using `Store::open(dir, Some(EngineId("b")))` + a seeded row with `owner = "a"`, the shape of `store.rs:5985-6020`); `resume_state` returns `owner: Some("a"), here: false` for it; the attention inbox lists no `resume:` item for it and does list one after a `ResumePending` |

### W2 — RELEASE-DRAIN + RESUME-ON-TAKE (D3, D4, D5, D15, D16, D19)

| | |
|---|---|
| **owns** | `crates/agent/src/supervisor/transfer.rs` (the `DrainSession` body per §6.1 steps 1–5; `release_window`; `on_resume_here` stays a dispatcher that calls W3's `child_waits::settle_on_transfer` and `reaper::settle_transferred` — those bodies are W3's); `crates/agent/src/supervisor.rs` (`UnloadSession` → `tell_cancel` + `stop_gracefully` + `wait_for_shutdown`; `draining_sessions` set and the `-32011` refusal in the input handler; the D15 `journal_prune(Scope::Here, keep_days)` loop in `supervisor::start`; `Jobs::KillSession` call site from `DrainSession`); `crates/agent/src/loop_.rs` (`loop_.rs:4530` draining condition; `TurnResumed{cause, from}` publish at `:4429-4438`; `restart_note` cause spelling + `from`; `report()` writes `cause`); `crates/sessionsync/src/owner.rs` (`release` → `drain_session` + `flush`; `Take::run`: delete the taker-side `unload` `:93`, add step 9 — `open_turn` → `ResumeTurn{cause}` or `ResumePending` + `append_local_note` by the §6.3 policy table; `demote` clears `draining_sessions` via a `transfer::` fn); `crates/agent/src/loop_/durability_tests.rs` (new scenarios `release_drain_writes_suspended_or_finished`, `cancel_while_draining_is_a_suspension`) |
| **non-goals** | store SQL, the reaper's logic, tools, RPC routing, UI strings, the attention actor |
| **gates** | `cargo test -p agent --lib -- loop_::durability_tests supervisor::`; `cargo test -p sessionsync --lib`; clippy |
| **DONE** | `owner.rs` tests: `take_from_a_reachable_owner_moves_the_tail_and_the_epoch` (`:483`) asserts the `suspended{release}` row is on B before B's `set_owner` and that B received a `ResumeTurn{cause: Transfer}`; `stale_owner_demotes_with_its_unshipped_rows_orphaned` (`:641`) still passes with 0 orphans in the transfer case; a `Write`/`Orphan` takeover emits `ResumePending`, a `User` takeover emits `ResumeTurn{Takeover}`; `UnloadSession` on a live turn always lands the cancel (test: 100 iterations, no uncancelled task) |

### W3 — TRANSFER-RESOURCES (D6, D7 wiring, D13, D14)

| | |
|---|---|
| **owns** | `crates/agent/src/durability/jobs.rs` (reaper: `jobs_open(Scope::Here)` + engine filter already in SQL; `settle_transferred(store, session, from) -> Reaped` building `LostJob{reason: OwnerChanged}` from `jobs_settle_transferred` rows + replicated screens; `lost_body` wording by `LostReason`; tests `reaper_ignores_foreign_engine_rows`, `settle_transferred_never_signals`); `crates/agent/src/supervisor/child_waits.rs` (`child_spawned` → `wait_put(ChildResult)`, `child_result_delivered` → `wait_clear`, `settle_on_transfer` per §7.4 step 3 with `compose_handoff`); `crates/agent/src/loop_/resume_calls.rs` (peer-wait transfer note; job note shows the owner-changed reason when `row.engine != me`); `crates/tools/src/jobs.rs` (`KillSession` handler body — the per-session `KillAll`; `Reaped` handler honours `LostReason`; `ListJobs` unchanged); `crates/tools/src/pty.rs` (`lost_page`/`SendInput` wording by `LostReason`); `crates/agent/src/durability/handoff.rs` only if `compose_handoff` needs a "cwd unreadable" short-circuit (today it never fails, `:218-300`) |
| **non-goals** | the drain sequencing (W2 calls `KillSession`; W3 makes it do the right thing), `acp_resume.rs` (W1), store SQL (C), UI |
| **gates** | `cargo test -p agent --lib -- durability:: supervisor::child_waits loop_::resume_calls`; `cargo test -p tools --lib -- --test-threads=1 jobs:: pty::`; clippy |
| **DONE** | a `jobs` row with `engine = "a"` on an engine `me = "b"`: the reaper neither adopts nor kills it; `settle_transferred` settles it `lost` with the owner-changed body; `KillSession` writes `lost` rows and screens before the signal (the shape of `killall_writes_lost_before_kill`, `tjobs.rs:2674`); `SpawnChild` writes a `child_result` wait that `hand_off` clears (`child_result_wait_written_and_cleared`); `settle_on_transfer` injects the lost note with handoff sections when `cwd` exists and without when it does not |

### W4 — RPC/UI (D11, D18)

| | |
|---|---|
| **owns** | `crates/rpc/src/api.rs` (`resume_turn` through `forward::route` with the §9.2 `Take` rule; `answer_ask`/`resolve_approval` → `question_get` → `route`, forward-not-take; `durability_wire_shapes` test `:6621-6661` extended with the new fields; `-32011` mapping); `crates/desktop/ui/src/**` (`store/sessions.ts` banner fields; `store/reduce.ts` `apply()` `TurnResumed` + `resumeFromLoaded` copy `owner/here/cause/from`; `components/chat/ResumeBanner.tsx` §9.3 table; `components/chat/OwnerOfflineCard.tsx` body + button; `lib/owner.ts` `sessionDraining()`; the composer's `-32011` retry; vitest `reduce.test.ts`, `ResumeBanner.test.ts`); `crates/tui/src/**` (`model.rs:6388-6405` divider + note with the owner; `owner_offline_line` suffix; tests `model.rs:16250-16319`, `ui.rs` divider test) |
| **non-goals** | engine behaviour of any kind; `forward.rs` (C); `attention.rs` (W1) |
| **gates** | `cargo test -p rpc -p tui --lib`; `bun run check`; `bunx vitest run src/store src/components/chat`; clippy |
| **DONE** | `resume_turn` on a non-owner reaches the owner (rpc test with the `Forwarded` marker, the shape of `forward.rs:652` tests); `answer_ask` for a question whose row names a foreign session is forwarded and answered `-32010` when the owner is down; every §9.3 banner line has a vitest case; TUI divider test covers `transfer`/`takeover` |

### Z — TESTS (D12)

| | |
|---|---|
| **owns** | `crates/sessionsync/tests/mesh_durability.rs` (new): two data dirs, two `Store::open(.., Some(EngineId))`, two `Agents` via `agent::testing::agents_on`, the sessionsync in-process `Loop`/`Switch` transport from `owner.rs`'s tests (hoisted into `crates/sessionsync/src/testing.rs`, Z-owned, `#[cfg(any(test, feature = "test-support"))]`), the 109 `BatchFake` provider; `justfile` (`durability-mesh` recipe: the in-process matrix by name, plus the optional scripted two-data-dir smoke `scripts/durability-mesh-smoke.sh` built on `68-two-engine-smoke.md` §1 and `109-as-built-W6.md` §8, **not CI**); `docs/design/139-smoke.md` (evidence) |
| **non-goals** | fixing engine code — a failing row goes back to the owning wave as a fix round (109 W6 rule) |
| **gates** | `just durability-mesh`; `just durability` still green |
| **matrix** | **T1** replica boots while the owner is mid-turn → B publishes no `TurnResumed`, writes nothing, A's turn finishes with the uninterrupted transcript. **T2** take while A is reachable mid-batch (`BatchFake` holding call 2 of 3) → `suspended{release}` on B before B's epoch row; B `TurnResumed{cause: transfer, from: a}`; A demotes with `session_orphans` = 0; transcripts equal modulo the `<engine_restart kind="transfer">` note; the mutating fake call ran exactly once across A and B; the native-child variant delivers the child's result on B. **T3** A partitioned (Switch cut) mid-batch, user `session_take(.., "user")` on B → `TurnResumed{cause: takeover}`; A's `started` mutating call reported interrupted, read-only re-run; when the Switch reconnects A demotes and its tail is in `session_orphans`. **T4** A partitioned, `send_input` on B (→ `Route::Take`, `Write`, after `takeover_secs` in config = 0 s for the test) → `ResumePending` on B, no `TurnResumed`; `resume_turn` on B → `TurnResumed{cause: takeover}`. **T5** `ask_user` pending on A, take → B's `attention_list` has `ask:<same id>`; `answer_ask` on B settles the call. **T6** `shell` job + `start` PTY on A, take → B's `list_jobs` shows both `lost` with the owner-changed reason; `pty_screen` on B returns the replicated page with the moved-from banner; A's process group dead (transfer) / alive (takeover). **T7** replica's `load_session` → `resume.owner == "a"`, `here == false`; `resume_turn` on B with A up is forwarded (A's `ResumeTurn` seen) and with A down returns `-32010`. **T8** (D16) `send_input` on A for the session during its release drain → `-32011`. **T9** (D15) `journal_prune` on A deletes a finished turn's rows and B's copy loses them through the log; B's own tick deletes nothing of A's |
| **DONE** | nine rows green in-process in under two minutes; the smoke script runs the T2/T3/T6 rows against two real engines on one Mac with evidence in `139-smoke.md` |

### Closer

Integrates the five branches, runs `just durability` + `just durability-mesh` + `cargo test
--workspace`, and applies the doc edits this design owes: `68-sessions.md` §3 (the attached-table
list — R1 §8.14), `68-contract.md:46` (same), `109-durability.md` §3.5 (`pty_screens` now attached)
and §14 ("resuming an in-flight turn on another engine" moves from Rejected to "built by 139"), §15
Q7 (answered), `00-architecture.md` if it names `open_turns`. Writes `docs/design/139-as-built.md`
in the shape of `109-as-built-*.md`. Rebuilt engine and desktop are owed after C lands (every
running engine predates `0038`).

## 12. Open questions and inconsistencies

Nothing here is resolved silently. Q-items need a ruling; the ledger proves every recon gap has a
home.

### 12.1 Questions and inconsistencies (with cites)

- **Q1 — D1's list vs the brief's own wording of `Scope::Here`.** D1 says consumers are "scoped to
  sessions this engine owns (`sessions.owner = self`)". The store's `here` is `owner = '' OR owner =
  me` (`store.rs:468`) and the fence treats `''` as ours (`store.rs:237-242`). This design uses the
  `here` form (`'' OR me`) so a lone engine without a mesh identity (`sup.rs:8094` "sessions stay
  owned by ''") keeps resuming its own turns; the literal `= self` would make 109 inert on every
  engine that has no identity file. Default: `here` form. Needs a nod.
- **Q2 — D4's cause list has no home for `parent_gone` and `drain`.** `Drain` reasons today are
  `shutdown | update | parent_gone` (the callers: `crates/theuth/src/main.rs:1723-1728`,
  `crates/rpc/src/api.rs:4875-4880`, `crates/update/src/lib.rs:663-666`) and the loop writes `drain`
  (`loop_.rs:4803`). D4's enum is `crash | shutdown | update | transfer | takeover`. §6.3 maps
  `parent_gone` and `drain` → `shutdown`; `restart_note` today spells `kind="shutdown"` for every
  `Suspended` plan (`loop_.rs:6994-6997`), so no model-facing wording changes for them. Default:
  fold; or add `parent_gone` as a sixth value if the UI should say "the app closed".
- **Q3 — `Agents::inject`/`route` do not check ownership (R2 §6, `sup.rs:1302-1306`).** Jobs'
  `Reaped` deliveries, reminders and spared results reach `tell_input` through `inject` and would
  spawn an actor for a non-owned session. After D1 the *sources* are scoped (reaper, spared results,
  reminders are per-session rows this engine wrote), so the path is not reached for foreign sessions
  in practice; but `rearm_reminders` (`sup.rs:2270`) was not audited by R1/R2 and `reminders`
  replicates (`capture.rs:24`). **Likely live hazard**: a replica's `rearm_reminders` fires the
  owner's reminder into a session it does not own. Proposed smallest guard: `inject` →
  `remote_owner(to)` → forward as `Agents::deliver` does (`sup.rs:4355-4361`) instead of spawning;
  one `if let` at the top of `inject`. Not in D1–D19; needs a ruling to add to W1.
- **Q4 — the release window vs the taker's timeout.** D3 says the owner drains with "the take's
  `release_wait_secs` as the window". The taker waits `release_wait_secs` for `Released`
  (`owner.rs:119`, `:129-131`) *and then* up to another `release_wait_secs` for the cursor
  (`:138-154`). An owner that drains the full window then flushes answers after the taker gave up.
  §6.1 rules `release_wait_secs − 1 s` on the owner's own config and keeps the wire
  `Msg::Release{session}` unchanged. Two engines with different `sessions.release_wait_secs` break
  the arithmetic. Alternatives: (a) carry the window in `Msg::Release` (new `Msg` variant tag to
  stay postcard-compatible, 69 §8 added tags that way), (b) raise the taker's first wait to
  `release_wait_secs + drain_secs`. Default: §6.1's; needs a ruling if (a) is preferred.
- **Q5 — D16 vs 109 §9.2 step 1 and the existing test.** D16 says inputs during a drain are
  "refused/queued with the 109 §9 wording". 109 §9.2 step 1 says inputs *are accepted* (row
  committed, `input_accepted` journaled, no turn starts), and `durability_tests.rs:1349`
  `draining_session_opens_no_model_call` pins the as-built form (turn opens, writes
  `input_accepted`, `suspended` at the first round). §6.1 refuses only during the **per-session
  release drain** (the fence moves) and keeps 109's acceptance for the global drain (the fence does
  not move). If D16 is meant literally for the global drain too, the test and 109 §9.2 step 1 change
  and I-(c) needs rewording.
- **Q6 — the owner backfill is unlogged and unreplicated (D17, R2 §10.11).** `store.rs:552-558` runs
  before capture; a restored or copied `sessions.sqlite` opened by a second engine claims every `''`
  row locally, and both engines then believe they own it; neither fence notices because the claim
  never ships. Smallest guard: refuse the backfill when `session_log` already has rows from
  *another* origin (`SELECT 1 FROM session_log WHERE origin != me LIMIT 1` — the log carries the
  origin per 68 §2) and log *"N unowned sessions left unclaimed: this store has replicated history;
  run `t sessions claim` to adopt them"*. Not in D17 ("no new behaviour unless the recon shows a
  live hazard"); the hazard needs a copied file, so it is a judgement call. Default: log only.
- **Q7 — `waits(peer_reply)` cannot re-arm across engines.** `rearm_peer_wait` reads
  `peer_exchanges` (`waits.rs:188-200`), engine-local by 68 decision 8. §7.3 settles the call as
  interrupted and relies on 69 P forwarding for the reply. Full fidelity would need `peer_exchanges`
  (or its receipts) to replicate — a 68 decision reversal. Default: interrupted + forwarded reply.
- **Q8 — D13's trigger assumes the parent's turn is open.** 109 §7.3 / D5 (109) and D13 speak of "a
  parent's tool call waits on the child"; `spawn_subagent` returns immediately (`peers.rs:248-266`)
  and the parent's turn normally finishes before the child does, so a `child_result` wait found only
  via `open_turns` fires only for the `send_message{await_reply}` shape. §7.4 iterates
  `child_waits(Scope::Here)` directly. Side effect: `waits_of(session, turn)` JOINs `tool_calls`
  (`dur.rs:1090-1102`) and `cut_journal_in` deletes waits of cut calls (`dur.rs:184-204`) — a
  `child_result` row outlives its turn and may be deleted by a compaction cut before the child
  finishes. Proposed: `cut_journal_in` skips `kind = 'child_result'`; needs a ruling (it is a 109
  §3.6 retention change).
- **Q9 — attention `reports` never clears (R2 §10.12, `attention.rs:887, 1429`).** W1 adds
  `reports.remove(session)` on `TurnResumed`/`TurnFinished` because the `ResumePending` item reuses
  the map; this fixes a 109 gap as a side effect. Flagging it because it is outside D1–D19.
- **Q10 — hub failover takes are `Write`, so D5 blocks auto-resume for a typing user.** After a hub
  change (69 §2) a device user typing into a session owned by the old hub triggers `Route::Take`
  with `TakeReason::Write` (`forward.rs:201-221`). D5 classes that as system-initiated → Attention
  item, no resume — even though a human just typed. Options: the composer sends `session_take(..,
  "user")` first when the loaded session has an open turn (UI change, W4), or `Route::Take` from an
  RPC with a real client peer (not `RpcPeer::internal()`) counts as `User`. Default: as ruled (D5);
  the item appears beside the user's own message.
- **Q11 — a transferred parent's children show as remote, not running.** `running` is process-local
  (`sup.rs:2144-2149`, 109 LIVE); B's sidebar renders A's children as `PeerState::Remote` rows
  (`sup.rs:4700-4703`). The parent's "N subagents running" glyph is therefore off on B while they
  run on A. Out of D1–D19; a follow-on if it matters.
- **Q12 — a mixed-version mesh cannot apply `0038` changesets.** A `jobs`/`resume_reports` changeset
  with the new column fails `sqlite3changeset_apply` on an unmigrated replica (same as 0033/0035
  had; `apply.rs:52-53` would return an error and the row would be retried forever). 68 has no
  version gate on `theuth/sessions/1`. Default: operational rule "upgrade every engine before the
  first post-0038 write"; a schema version in `Hello` is a 68 follow-on.
- **Q13 — Z's in-process harness needs agent test code exported.** D12 prefers in-process; R1 §7
  shows the sessionsync replica already runs in-process (`Loop`/`Switch`, `owner.rs:483-700` tests)
  and R2 §9 shows the 109 fakes are `cfg(test)`-private in
  `crates/agent/src/loop_/durability_tests.rs:43-61` and `vision_fallback_tests.rs:337-446`. C's
  `test-support` feature (§11) is the smallest bridge; if the fakes' private dependencies (`Harness`
  holds `Store`, `Registry`, provider plumbing) make the re-export a refactor, Z falls back to D12's
  scripted smoke for T1–T7 and keeps the unit tests. Decision point at the end of C.
- **Q14 — `jobs.engine` default `''` on a lone engine vs a later identity.** A store written with
  `me = ''` and later opened with an identity has `jobs.engine = ''` rows that the backfill does not
  touch (it rewrites `sessions.owner`, not `jobs.engine`). `'' OR me` matches them (§10), so they
  are treated as this engine's — correct for the single-machine upgrade and wrong only in the Q6
  copied-file case.
- **Q15 — `resume_turn` RPC with `Route::Take` resumes on a `Write` take.** §9.2 has the handler
  send `ResumeTurn{Takeover}` after a `Write` take because the user pressed Resume. Strictly, D5
  keys on `TakeReason`, and the take was `Write`. The design treats the RPC's intent as the user's.
  Flagged as an interpretation.
- **Q16 — docs drift already on record.** `68-contract.md:46` lists nine attached tables and
  `68-sessions.md` §3 was never updated with the journal tables (R1 §8.14);
  `109-durability.md:1224-1227` lists cross-engine resume under Rejected. The closer owns these
  edits (§11); listed so they are not lost.

### 12.2 Recon gap ledger

Every R1 §8 and R2 §10 item, where it is fixed or why it is not.

| recon item | fix | where |
|---|---|---|
| R1 §8.1 `open_turns()` store-wide | `Scope` on every reader | C + W1 (§5.1) |
| R1 §8.2 `resume_scan` resumes foreign turns under `auto` | `Scope::Here` + `ensure!(row.here)` | W1 (§5.1 items 1–2) |
| R1 §8.3 `reap_jobs` kills replicated pgids | `jobs.engine` + engine filter | C + W3 (§7.1) |
| R1 §8.4 `rebuild_waits` re-arms foreign waits | `Scope::Here`; `rebuild_waits_for` | W1 (§7.3) |
| R1 §8.5 attention `resume:` items for every engine; `resume_turn` unrouted | `OpenTurns(Scope::Here)`; `resume_turn` routed | W1 + W4 (§5.1 item 13, §9.2) |
| R1 §8.6 nothing runs after a take | §6.2 step 9 | W2 |
| R1 §8.7 `unload` race, `cancelled` not `suspended` | drain for the ownership path; `stop_gracefully`; draining-cancel = suspension | W2 (§6.1) |
| R1 §8.8 no local fence during release; orphans not merged | `draining_sessions` refusal closes the window; orphans stay unmerged (68 §4.3, by design) | W2 (§6.1, §6.2 "fence interplay") |
| R1 §8.9 `Loaded.resume.resumed` is the serving engine's flag | `owner`/`here` fields; `resumed` meaningful only when `here` | C + W1 + W4 (§5.2, §9.3) |
| R1 §8.10 `pty_screens` not replicated | joins `ATTACHED` | C (§7.2) |
| R1 §8.11 questions answered only on the owner | `answer_ask`/`resolve_approval` routed via the row's session | W4 (§9.2) |
| R1 §8.12 children not co-owned | **not changed**: results follow the owner (69 P); co-ownership is a 68 change — §8 "Children", Q11 | — |
| R1 §8.13 children/jobs/PTYs die unseen on the demoted owner | `KillSession` on transfer; `settle_transferred` + child-wait settlement on takeover | W2 + W3 (§7.1, §7.4) |
| R1 §8.14 docs list nine tables | closer edits | closer (Q16) |
| R1 §8.15 no two-store journal test | Z matrix | Z |
| R2 §10.1 replica spawns an actor for the owner's turn | as R1 §8.1–2 | W1 |
| R2 §10.2 refused writes are warn-and-continue, tools still run | **not changed**: unreachable once D1 holds; the fence stays the backstop. A hard abort on `NotOwner` in the loop would be a 109 change — not ruled | — (Q1 context) |
| R2 §10.3 take never resumes; `settle_open_turns` supersedes silently | §6.2 step 9; `off` mode keeps the supersede semantics on purpose | W2 (§6.3) |
| R2 §10.4 `UnloadSession` bypasses the drain | §6.1 | W2 |
| R2 §10.5 drain writes/publishes for foreign turns | `Scope::Here` in both loops | W1 (D16) |
| R2 §10.6 reaper kills unrelated local groups, injects into foreign sessions | `engine` filter; `Scope::Here` | C + W3 |
| R2 §10.7 `pty_screens` unfenced, old owner keeps writing | attached → fenced | C (§7.2) |
| R2 §10.8 `child_result` never written; ACP children not in `open_turns` | writer added; trigger via `child_waits`; ACP children still journal no `turn_steps` — **not changed** (109 §7.3 gap, out of D13's scope; the respawn path does not need it) | W3 (§7.4), Q8 |
| R2 §10.9 respawn gated on harness entry only | `row.here` + readable `cwd` | W1 (§7.4) |
| R2 §10.10 `Loaded.resume`/`resume_turn`/`OpenTurns` served locally on a non-owner | display-only with owner named; `resume_turn` routed; `OpenTurns(Scope)` | W1 + W4 |
| R2 §10.11 backfill claims every `''` row | logged once; guard proposed | W1 (D17), Q6 |
| R2 §10.12 `resume_reports` zeros; attention `reports` never cleared | `lost_jobs` populated on transfer (§7.1); `reports.remove` | W3 + W1, Q9; `children_*` counts **still zero** (109 W4 gap, not ruled) |
| R2 §10.13 in-memory mirrors rebuilt from global reads | `rebuild_waits_for` on the owner; old owner's late writes refused as today | W1 (§7.3) |
| R2 §10.14 `journal_prune` has no caller | hourly tick on the owner | W2 (D15, §5.3) |
| R2 §10.15 inputs not refused during a drain | refused during the release drain; accepted during the global drain | W2 (§6.1), Q5 |

## 13. Revision history

- **v0** — 2026-10-08, theuth orchestrator from recon R1/R2 at `07f8e1f3` (R2 cites at `17769a5c`,
  verified unchanged). Rulings D1–D19 of the 03:22 brief and its Addendum A1 (03:50) written as
  decisions; §12 lists sixteen open points and the thirty-item recon ledger. No code changed.
- **Revision 1** — 2026-10-08 04:15, orchestrator rulings on §12 Q1–Q16 and the migration number (0038), appended below.
- **Revision 2** — 2026-10-08 11:05, HARNESS-ENVELOPE: the Q3 follow-on (harness-made inputs for a session owned elsewhere are forwarded, reminders follow the owner), rulings R2-D1–D6 and what R2 built, appended below. Migration `0040_harness_outbox.sql`.

## Revision 1 — orchestrator rulings on §12 (2026-10-08 04:15)

Rulings taken by the orchestrator (Sky delegated the flow at 03:25: "coordinate with the peers, get it done with omp agents, get it into main"). Each binds the wave named in §11.

- **Q1** — `Scope::Here` is the store's `here` form (`owner = '' OR owner = me`), never the literal `= self`. (C)
- **Q2** — fold: `parent_gone` and `drain` map to `shutdown`; no sixth cause. (C, W2)
- **Q3** — add the `inject` owner guard to W1: `inject` → `remote_owner(to)` → forward as `Agents::deliver` does, never spawn an actor for a non-owned session; `rearm_reminders` runs under the same guard. (W1)
- **Q4** — keep §6.1's owner-side window (`release_wait_secs − 1 s`) and ALSO raise the taker's first wait to `release_wait_secs + drain_secs`; the wire `Msg::Release` is unchanged; a mesh runs one `sessions.*` config (documented in the as-built). (W2)
- **Q5** — D16 is narrowed to the per-session release drain (the fence moves); the global drain keeps 109 §9.2 step 1 (inputs accepted, no turn starts) and `draining_session_opens_no_model_call` stays as is. (W2)
- **Q6** — log only (D17 as written); the copied-file guard is a follow-on. (W1)
- **Q7** — `waits(peer_reply)` across engines: settle as interrupted + forwarded reply (the default). (W3)
- **Q8** — `cut_journal_in` skips `kind = 'child_result'` (a 109 §3.6 retention change, accepted). (C)
- **Q9** — accepted as W1's side effect.
- **Q10** — a `Write` take requested by an RPC from a real client peer (not `RpcPeer::internal()`) counts as **User** (a human typed: resume now); internal/system takes (desk wakes, hub failover plumbing) stay system → `ResumePending`. (W2 policy table, W4 for the client-peer flag if the handler lacks it)
- **Q11** — out of scope (follow-on).
- **Q12** — operational rule, recorded in the as-built: upgrade every engine of a mesh before the first post-migration write ships.
- **Q13** — decision point at the end of C as written: try the `test-support` feature; if the fakes cannot be exported without a refactor, Z keeps the unit tests and runs T1–T7 as the scripted two-engine smoke.
- **Q14** — accepted.
- **Q15** — the `resume_turn` RPC's intent is the user's: `ResumeTurn{Takeover}` after its `Write` take. (W4)
- **Q16** — the closer owns the doc edits listed in §11.
- **Migration number** — `0037` is taken by design 137 (in flight, lands before 139): this design's migration is **`0038_mesh_durability.sql`**; every `0037` in this document reads `0038`. C verifies the next free number on main AND in the in-flight branches at commit time.

## Revision 2 — HARNESS-ENVELOPE: the Q3 follow-on (2026-10-08 11:05)

Rulings taken by the orchestrator at 11:05 for the one open follow-on of the as-built (§7 first bullet): Revision 1 Q3 made `Agents::inject` *drop* (with a `warn!`) a harness-made input — a settled job's `<job_result>`, a fired `<reminder>`, a lost child's result — addressed to a session another engine owns, because the only cross-engine primitive was the peer message on `theuth/rpc/1` (`forward_peer` → `PeerEnvelope { from: SessionId, .. }`), whose receiver applies peer-message semantics (`peer_exchanges`, receipts, `Policy::route`). Harness bodies have no sender session. `rearm_reminders` ran at boot only, so after a take the new owner never armed the session's reminders and the old owner's timers kept ticking. Built by R2 (`139-as-built-R2.md`); cites are of the R2 tree.

- **R2-D1 Harness envelope on the sessions wire.** Two new frames on `theuth/sessions/1`, appended to `Msg` (`sessionsync/types.rs`; tags 9 and 10, postcard-compatible): `Harness { session, kind, origin, id, text, wrapped, scope, at }` and `HarnessAck { id, error }`. `kind` is the `from` label `inject` already carries verbatim (`job j3`, `reminder r1`, `session:<child>`) — the brief's enum `{job_result, reminder, spared_result, other}` collapses into it, nothing reads the discriminant. `origin` is the minting engine; `id` a sender-minted UUID the receiver dedups on. Idempotency survives a restart through an engine-local `harness_seen(id, at)` table; a replicated table is not allowed for this (the fence refuses captured writes for sessions owned elsewhere), so both tables live in `sessions.sqlite` **outside** `capture::ATTACHED` (`0040_harness_outbox.sql`, `store/harness.rs`). The ack is the brief's "on success delete the row": a frame written to a stream is not a delivery.
- **R2-D2 `inject` for a remote owner enqueues.** `Agents::inject` (`supervisor.rs`, the remote branch) writes a `harness_outbox(id, session, kind, origin = me, text, wrapped, scope, at, attempts, last_error)` row and wakes the shippers (`Store::harness_watch`); the body gets one line inside its closing tag — `(produced on <me> after the session moved to <owner>)` — so the model and the UI read one element and the brief's "finished on <engine> after the transfer" is in the message. The receiving owner (`sync.rs::receive_harness`) checks the session is known here, records `harness_seen`, and tells `Agents::HarnessDeliver`, which runs the **same** `inject`: owned here → admitted `Automatic` and routed; moved on meanwhile → `remote_owner` → a new outbox row to the next owner (a fresh id; the bounce-back case A→B→A is not a dedup loss). Unknown session → `HarnessAck { error: "unknown session" }`, the sender keeps the row with `last_error`; no supervisor on the receiver (a pairing run) → `"no supervisor here"`, the `seen` mark is undone so a resend is delivered. Never take-on-write. The sender's `scope` travels but the receiver delivers unscoped: a request scope is the sender's ledger's (`network.scopes` is in-memory and engine-local), and an unknown scope would have *dropped* the body as "scope closed".
- **R2-D3 Retry.** The sync stream (`sync.rs::run`) flushes the outbox after its `Hello` exchange (the peer (re)connected), on every enqueue (`harness_watch`) and every 60 s (`HARNESS_RETRY`, a `tokio::time::interval`) — `flush_harness` re-resolves the owner per row from the `sessions` row at every attempt and sends only the rows this stream's peer owns; a row whose session came back here is handed to the local supervisor; a row older than `sessions.harness_outbox_ttl_secs` (default 7 days, `SessionsConfig`, `harness.md`) is dropped with one `warn!` and a local-only `append_local_note` on the sender's copy (as 68's orphan note; the owner's next message replaces it). `harness_seen` is pruned past the same TTL. With no stream at all nothing flushes — the first stream to open does.
- **R2-D4 Reminders follow the owner.** The taker tells `Agents::RearmReminders { session }` right after `set_owner`/`set_fence` in `owner::Take::take` — not in `transfer::on_resume_here` as the brief placed it: `on_resume_here` runs only inside `resume_turn`, so a take with no open turn (T10/T10b's shape) would never arm them, and arming in both places would double-arm (`RearmReminder` does not dedup). `rearm_reminders` became `rearm_reminders_for(Option<SessionId>)` (boot: `None`). The old owner stops its timers without touching the rows through a new `tools::CancelReminders { session }` tell (`reminder.rs`; `close_reminders` would publish `ReminderCancelled` → `delete_reminder`, which on the release path runs *before* the fence moves and would replicate the delete to the taker): sent from the release drain (`transfer::drain_session`, step 4 beside `KillSession`) and from `transfer::demoted` (the takeover path, which has no drain). A reminder that still fires on the old owner in the race window goes through `inject` → the envelope.
- **R2-D5 Finished-job bodies after a transfer.** Holds for the **takeover** path only: on a connected transfer the release drain's `KillSession` (§7.1 D6/D14) kills the job, so there is no later real result — the taker's `settle_transferred` row `lost` is final. On a takeover the old owner's process keeps running (a takeover signals nothing, T3); when it settles after the demote, `<job_result>` → `inject` → envelope → the new owner, as a second message naming the original job id and the engine it ran on. The taker's `jobs` row is not un-lost. A result that settles *before* the demote lands (A still believes it owns) is injected on A and becomes A's orphan (68 §4.3) — the window is the reconnect, not the whole partition.
- **R2-D6 Scope.** No UI change; TUI none; RPC none (engine-to-engine only); Attention none. One config key (`sessions.harness_outbox_ttl_secs`), two engine-local tables, two wire frames.
- **Smoke rows.** T10 (connected transfer): `start` PTY + `remind_me_in(30)` on A, `session_take(user)` on B → the reminder fires on B at ~30 s from the replicated row, once, not late; A's timer stopped ("reminder timer(s) stopped"), A's PTY group dead (drain), A's outbox empty. T10b (takeover): PTY `sleep 75` + `remind_me_in(90)` on A; A partitioned (SIGSTOP), B takes, A resumed → A demoted while the PTY still runs; at ~75 s the real `<job_result exit="0">` naming A arrives on B over the envelope exactly once — as did the PTY's `<job_idle>` nudge at 60 s, every harness-made input travels the same way (A's outbox empty, B's `harness_seen` one id per envelope); the reminder fires on B at ~90 s; A never fires. The brief's T10 (a `shell` whose real result reaches B after a connected transfer) cannot hold under §7.1's drain kill; T10b is the envelope's live row.

