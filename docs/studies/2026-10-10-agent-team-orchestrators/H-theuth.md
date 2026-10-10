# theuth today — the team-of-agents inventory on the eleven axes (main b4e5833a, 2026-10-10)

Read-only recon of `/Users/Sky.Moore/Develop/theuth`. The brief named main b4e5833a. While I worked, Sky's checkout moved to
c22556ea (4 commits: meeting/notebook/CodeMap UI only, 7 files under `crates/desktop/ui`). No team seam changed, so every cite
below is the same at both commits. Seven untracked files under `docs/design/` (cross-platform and scratch recon) are Sky's and
were not read. On main the newest design is 156 (meeting view). 157/158 are not committed. Cites are `theuth/<path>#L<a>-L<b>`.
Live state under `~/.theuth` is reported as formats and counts only.

## Executive summary
1. **What it is.** theuth is a Rust meta-harness: one engine per machine in a leaderless mesh. An *agent* is a markdown file
   on a roster, and a *desk* is that agent's long-lived session. Work lives on a SQLite task ledger with wakes, checkout and
   dispositions. Routines are schedule files. Workers are native sessions or external harnesses over ACP. It has the
   Paperclip control-plane primitives plus things Paperclip lacks: mesh placement, sandbox VMs, the owner's *double*,
   packets/mini-boards for decisions, an attention inbox, knowledge + dreaming, sensors and autoresearch.
2. **Idea 1: the wake ledger is solid; wake *delivery* breaks under real use.** Every 2026-09-24 Paperclip borrow landed in
   code: checkout+epoch, the disposition checklist, coalesced wakes, no-progress back-off, the write cap, scoped wakes,
   pending hires, budget pause and the field log. The recurring pain is cold desks, wakes stuck behind an in-memory claim
   after restarts, escalate refused by session identity, and prompt-only rules.
3. **Idea 2: the human is the bottleneck by construction.** 78 of the 80 live tasks name `user` as reviewer. 41 sit in
   `review`, and nothing on the board has moved since 2026-10-06 18:34. Review routing and batching matter more than any
   new primitive.
4. **Idea 3: the A3/A4 gaps are the code-team ones.** There is no goals tree, external trackers are links only, there are
   no engine-managed per-task worktrees and no PR/CI → task loop, a run carries no cost, and wakes have no claim time. All
   are cheap to add on the existing seams.
5. **Verdict:** theuth is ahead on *where and how* agents run (mesh, ACP, VMs, the double, channels, knowledge). It is
   behind the code-team control planes on the *work-to-merge loop*. Fix delivery and review flow before adding features.

## Size and shape of the team seams
Counts: `wc -l` of `*.rs` per crate and `#[test]`/`#[tokio::test]` attributes (inline test modules included). Nothing was built or run.

| crate | role in the team | .rs lines | files | tests |
|---|---|---:|---:|---:|
| roster | agents, desks, budgets, hires, attention, alerts, hand-offs, pins | 11,254 | 15 | 79 |
| tasks | ledger, wakes, checkout, dispositions, escalate, packets, placement, activity | 14,381 | 12 | 100 |
| schedule | routines: cron/every/at, `until` loops, sensors, `agent:` fires | 8,549 | 5 | 77 |
| mesh | iroh endpoint, peers, stability-elected hub lease | 4,677 | 10 | 28 |
| acp | ACP client half, harness registry (omp, opencode, codex-acp) | 5,105 | 14 | 47 |
| double | owner's decision model answering approvals/asks (126) | 13,993 | 20 | 79 |
| agent | sessions, supervisor, subagents, peers, activity monitor (155) | 109,873 | 100 | 617 |
| a2a / nodes / replica | A2A boundary / paired remote harnesses / LWW+HLC replication | 3,785 / 1,662 / 3,990 | 4/6/11 | 24/8/22 |
| sandbox | make-engine over SSH/Docker + sandbox VMs (79) | 10,743 | 22 | 48 |
| decision / dreaming / knowledge | local classifier / nightly learning / OKF bundle | 16,599 / 10,819 / 9,081 | 31/18/8 | 109/61/55 |
| tools / rpc / surface / autoresearch | tool registry+policy / engine RPC / UI surface / study loop | 23,402 / 16,108 / 13,905 / 27,951 | 25/13/17/25 | 193/59/97/176 |

The six team-core crates (roster, tasks, schedule, mesh, acp, double) total **57,959 lines and 410 tests**. Three files are
very large: `crates/tasks/src/ledger.rs` (6,237 lines), `crates/roster/src/attention.rs` (3,602) and
`crates/roster/src/lib.rs` (3,563).

**Live state (formats and counts only).**
- `~/.theuth/agents/`: 5 files. The only frontmatter keys used are `title`, `model` and `capabilities` (5/5 each). None sets
  `reports_to`, so all five report to the default `theuth`. None sets `budget`, `tools`, `mcp`, `effort`, `paused` or
  `runs_in`. The governance knobs exist but the live team does not use them.
- `~/.theuth/schedules/`: 2 files, both `every:` and `quiet`. One is desk-routed (`agent:`). The other is a sensor watcher
  (`when:` + `max_runs`/`cooldown`/`fire`).
- `~/.theuth/tasks.sqlite` (8 migrations): 80 tasks. By state: backlog 24, review 41, blocked 10, cancelled 3, done 2.
  - Owners: 79 owned by agents (6 distinct agent owners, `theuth` included), 1 by `user`. Reviewer `user` on 78/80.
  - Labels and links: 3 project labels, 27 unlabelled. 10 tasks carry blockers, 8 a parent, 77 a `done_when`, 50 external
    links. `harness` is NULL on all 80.
  - Recency: last change 2026-10-06 18:34. The census is identical to the 2026-10-07 one recorded in
    `facts/theuth-task-board-store-tasks-sqlite`.
  - Other tables: 1,382 `task_events` (wake 351, comment 264, state 195, note 124, disposition 80, created 80, checkout 79,
    blockers 72, brief 38, owner 36, external 36, done_when 25, title 2), 16 `wakes` rows, 31 `packets`, 807 `activity`.
  - Wakes outnumber state changes 1.8:1, and comments almost match wakes: coordination is chatty.

## A1 Agent model & identity
**What an agent is.**
- An agent is a file `agents/<name>.md`: front matter plus a markdown charter. It parses to `AgentFile`
  (`theuth/crates/roster/src/types.rs#L210-L253`). Fields:
  - identity and line: `title`, `reports_to` (default `theuth`), `capabilities` ("when I am relevant", read by others when
    handing off);
  - runtime: `model`, `effort`, `tools` risk ceiling (`read_only|mutating|destructive`), `mcp` allowlist;
  - money and state: `budget` (`none` | `N CUR/month`), `paused`, `proposed_by`;
  - placement and memory: `runs_in` pin (`any|vm|vm:<name>`), `memory` mode and `view_bytes` (137 ENDLESS).
- The charter body is the instructions. `roster.sqlite` holds `agents`, `episodes`, `proposals`, `budget` and
  `budget_override` (`theuth/crates/roster/src/store.rs#L1-L2`).
- **No harness binding on the agent.** This is deliberate (67 decision 1, Sky 2026-09-25: "an agent uses a harness, not
  1:1"). The harness and engine are chosen per *task* by placement (`theuth/crates/tasks/src/place.rs#L1-L14`). The desk
  then spawns there (`theuth/crates/roster/src/desk.rs#L65-L73`). Paperclip binds `adapterType` 1:1 per agent; theuth is ahead here.

**Identity in the prompt.** `desk::identity` replaces theuth's base prompt with "You are ‹Title› (‹name›) … you report to
…", followed by the charter, capabilities, journal and the desk *manual* (`theuth/crates/roster/src/desk.rs#L174-L232`).
This fixed the 2026-09-25 bug where an infra desk answered "I'm theuth"
(`decisions/theuth-desk-protocol-employee-rulings-2026-09-25`). Children get a `child_identity`: "working for ‹Title› on task T".

**Lifecycle.** Active or paused, plus "proposed" for an agent-proposed hire awaiting approval. Desks open eagerly only for
files that are valid, unpaused and approved, and only on the engine holding the hub lease
(`theuth/crates/roster/src/lib.rs#L461-L477`). `theuth`'s own desk opens lazily on the first wake, so a one-laptop user with
no agent files gets no desk session at all.

**Hiring and onboarding.** There are three paths:
1. **Chat.** `agent_draft` shows cards (Hire / Hire paused / Replace / Discard). `agent_put` writes the file after the
   user presses a button. Any desk other than theuth's is refused and must use `agent_propose`.
2. **UI.** A seven-step wizard: `STEPS = ['identity','role','runtime','access','budget','charter','review']`
   (`theuth/crates/desktop/ui/src/components/agents/wizard.ts#L5-L6`).
3. **Agent proposal.** `agent_propose` writes the file with `proposed_by`. That files a "Hire agent …" task owned by the
   manager (`theuth/crates/roster/src/proposals.rs#L6-L42`). The proposed agent gets no desk until approved.

**Templates, export/import:** none found beyond the file itself. A file can be copied between machines; the
`replica`/mesh sync covers `roster.sqlite` tables, not the `agents/` files. No template catalog or icon picker; the latter
was a recorded deviation from Paperclip (`decisions/theuth-agents-ui-copy-paperclip-2026-09-25`). No `agent_rename`.

**Gaps vs the field.** Missing:
- a role taxonomy (Paperclip's ceo/cto/…);
- per-agent skills bundles (skills are global under `~/.theuth/skills`);
- agent templates/marketplace;
- versioned charters (agent files are not under git the way knowledge is);
- per-agent memory beyond the desk journal and the session's endless memory.

## A2 Organisation & authority
**Org chart = reporting lines.** One manager per agent (`reports_to`), root `theuth`. The UI draws it as Paperclip's
OrgChart: cards plus SVG elbow edges (`theuth/crates/desktop/ui/src/components/agents/OrgChart.tsx#L20-L23`).

**Delegation:**
- child tasks with `parent` plus blocker links;
- `task_reassign` with a thread line "reassigned to X: why" and a `Reassigned` wake;
- the **fit check**. On every `assigned`/`reassigned` wake the desk reads a `<roster>` block of active agents and
  capabilities, and hands over if another agent fits better (`theuth/crates/roster/src/desk.rs#L65-L69`). Paperclip
  deliberately puts no roster in the prompt and never auto-reassigns. theuth deviates on Sky's ruling that routing is
  theuth's job.

**Escalation:**
- `escalate` blocks the task on `decision:<id>`, wakes the manager and waits live; a late answer still clears the blocker
  (`theuth/crates/tasks/src/escalate.rs#L1-L5`);
- escalating to the user requires a *packet*: summary ≤ 120 words, a mini-board, options, `if_wrong`
  (`theuth/crates/tasks/src/packet.rs#L1-L7`);
- the manual forbids a second ask on a task already waiting on the user (`theuth/crates/roster/src/desk.rs#L82-L84`).

**Authority is keyed on the actor string.** Any actor without the `agent:` prefix is "the top of the line"
(`theuth/crates/tasks/src/ledger.rs#L1619-L1622`). In real use this refused escalations from desk sessions that had been
re-rooted as `user` (see "behave in anger").

**Permissions:**
- per-agent `tools` ceiling and `mcp` allowlist (`theuth/crates/roster/src/types.rs#L224-L228`);
- a `read_only` owner is always placed on the native harness, because an external harness cannot enforce the ceiling
  (`theuth/crates/tasks/src/place.rs#L10-L12`);
- the cross-task write cap: hired agents write freely to their own reporting line and are capped elsewhere; theuth and
  the user are uncapped (`theuth/crates/tasks/src/ledger.rs#L463-L500`).

**Visibility and isolation.** Knowledge concepts carry `private|team|shared` visibility; `team` means you, your managers
and the agents reporting to you. On the board, no task ACL was found: `tasks_list` filters only by owner, state, engine and
project (`facts/theuth-task-board-store-tasks-sqlite`). There is no multi-company/workspace key on `AgentFile` or `Task`:
one roster per mesh.

**Where humans sit.** Two places:
- the user is the default reviewer when no reviewer is set and the agent has no manager (`reviewer: None` means the
  owner's `reports_to`, `theuth/crates/tasks/src/types.rs#L211-L212`; the send-back guard falls back to
  `Owner::User`, `theuth/crates/tasks/src/ledger.rs#L894-L900`);
- the user is the top of every escalation line and the approver of hires.

Live: 78/80 tasks name `user` as reviewer.

**Ahead:** chat-first drafts (`task_draft`/`agent_draft`/`routine_draft` before anything is written) and the double (A6).
**Gap:** no board/human role per *member* (one owner). No delegated reviewer pools. No multi-tenant isolation.

## A3 Work model
**Task record:**
- the record is `Task` (`theuth/crates/tasks/src/types.rs#L197-L263`);
- states `backlog | todo | doing | review | blocked | done | cancelled`;
- exactly one `owner` (`agent:<name>` | `user` | `system`) and an optional `reviewer` (`None` means the owner's manager);
- `blockers` of three kinds (task ids, `decision:<approval>`, `person:<name>`) plus the computed inverse `blocks`;
- `parent` (delegation tree), `project` (a flat label), `where`/`harness` placement and `external` URLs;
- a `thread` document (59), the current `checkout`, routine provenance `schedule`, back-off counters, `done_when`,
  attachments, and a `priority` only the user sets.

**State machine in code, not prompts.** `guard()` (`theuth/crates/tasks/src/ledger.rs#L890-L925`):
- a backlog task is started first (`todo`); `todo → doing` happens only through `task_checkout`;
- review send-back belongs to the reviewer;
- `blocked → todo/doing` needs an empty blocker list *and* the owner;
- `doing → review` is the owner's move.

`allowed_moves()` lists the legal targets in every refusal (`theuth/crates/tasks/src/ledger.rs#L1193-L1199`).

**Acceptance and review.**
- `done_when` was added by 70 DESK-PROTOCOL. A desk moving a task to `done` must give `DoneBecause`: `criteria_met |
  intent_met | follow_ups_filed{tasks}` (`theuth/crates/tasks/src/types.rs#L424-L441`).
- Self-review is allowed only against explicit `done_when` (`decisions/theuth-desk-protocol-employee-rulings-2026-09-25`).
- Reviews and decisions for the user carry a packet with a mini-board (`theuth/crates/tasks/src/packet.rs#L1-L7`).
  `review → done` can carry a `next` pick from the packet's offered next moves (`theuth/crates/tasks/src/ledger.rs#L1201-L1206`).

**Dependencies:** yes, as blockers. Two wakes close the loop: `BlockersResolved` (every blocker resolved) and
`ChildrenCompleted` (every child done or cancelled) (`theuth/crates/tasks/src/types.rs#L291-L295`). Live: 10 of 80 tasks
carry blockers and 8 have a parent.

**Goals/projects/milestones:** *not found*. `rg -i goal` over `crates/tasks/src` and `crates/roster/src` returns nothing.
A project is a label shared with sessions (`theuth/crates/tasks/src/types.rs#L220-L221`). `tasks.project_order` plus the
user's priority make one rank for the inbox and for wake dispatch (`theuth/crates/tasks/src/rank.rs#L1-L3`,
`theuth/crates/tasks/src/config.rs#L26-L27`). There is no goal tree (Paperclip has company → team → agent → task), no
milestone and no roll-up.

**Planning/decomposition:** done by prompting, not by structure:
- the desk manual tells the desk to file child tasks with `task_create {parent}` or spawn children, at most
  `roster.max_rounds` (default 3) rounds per task per episode (`theuth/crates/roster/src/desk.rs#L75-L81`);
- chat work goes through `task_draft` first (`theuth/crates/tasks/src/tools.rs#L192-L199`).

**External trackers as the queue:** *no, by stated design.* "External tickets (Jira, GitHub) are links on the task, never
the source of truth" (`theuth/crates/tasks/src/tools.rs#L198`). Live: 50 of 80 tasks carry external links, mirrored by
hand. Jira/GitHub MCP tools exist for agents to call; they do not sync the board.

**Routines create work.** A schedule with `agent:` creates one task per fire, owned by that agent
(`theuth/crates/schedule/src/file.rs#L394-L397`). The unique `(schedule_name, schedule_fire_ms)` index makes a re-fire
coalesce (`theuth/crates/tasks/src/types.rs#L233-L235`).

**Gaps:**
- goals/milestones;
- tracker import/sync;
- task templates;
- estimates or story points;
- recurring-task semantics other than routines;
- one owner only (no pair/assist);
- the backlog→todo start is open to any actor (`theuth/crates/tasks/src/ledger.rs#L891`), so "desks never self-start"
  is a prompt rule (`decisions/sre-desks-do-not-self-start-backlog-tasks`).

## A4 Execution & runtime
**Desk = long-lived session + wakes.**
- Raising a wake: `Ledger::wake` refuses a reason that does not apply to the task's state. Otherwise it upserts a `wakes`
  row keyed `(agent, task, reason)`, increments `count`, and writes "woke X (reason)" into the thread
  (`theuth/crates/tasks/src/ledger.rs#L392-L420`; key and count `theuth/crates/tasks/src/types.rs#L364-L382`).
- Dispatch runs every 1 s and only on the hub-lease holder:
  - an agent with any claimed wake is `busy` and gets nothing new;
  - it logs "wakes held: desk still serving an earlier wake" once per blocking wake;
  - it picks by (rank, answer-class, age) (`theuth/crates/tasks/src/ledger.rs#L1919-L1975`).
- The wake text scopes the turn: "Do not switch to another task until this wake is handled", plus "(Raised N times.)" and a
  `<task …>` block (`theuth/crates/roster/src/desk.rs#L233-L262`).

**Turn judgement.** On `TurnFinished` the roster runs `judge_turn` against the nudge table
(`theuth/crates/roster/src/desk.rs#L436-L467`). A turn that ends without a disposition gets one nudge. After that comes
`no_continuation` and a manager wake. A turn that did not yield (cancel, timeout, provider error) releases the wake with
a delivery back-off of 5 s doubling to 5 min, with a 5-min floor after a cancel
(`theuth/crates/tasks/src/ledger.rs#L2060-L2073`; `theuth/crates/roster/src/lib.rs#L1037-L1060`).

**Concurrency.** One claimed wake per agent at a time: one desk session per agent. Parallelism comes from the desk's
children, ACP harnesses or native subagents. Paperclip runs up to 20 concurrent runs per agent. theuth does not; it
serialises per agent and fans out under the desk.

**Placement across machines.** `place::decide`, in order (`theuth/crates/tasks/src/place.rs#L1-L14`):
1. `where`/`harness` already on the task;
2. repo paths or the project cwd narrow to engines that have them;
3. display/GPU needs;
4. `read_only` → native;
5. repo work → `omp` when advertised.

One engine left: placed. Several: `tasks.place_prefer` breaks the tie, else theuth's desk decides. The agent pin `runs_in`
places all of that agent's work on a sandbox VM; the task waits `blocked` until the VM is up
(`theuth/crates/roster/src/pin.rs#L1-L8`). Engines can be made over SSH/Docker (`theuth/crates/sandbox/src/lib.rs#L1-L4`) or paired as remote harnesses (`nodes`). Paperclip's placement is a static default environment and SSH that
copies the workspace out and back (`facts/paperclip-architecture-recon-2026-09-24`). theuth is ahead.

**Leaderless hub.** Election by stability is "a pure function every engine runs over the same inputs, so no message ever
says 'you are hub'" (`theuth/crates/mesh/src/lease.rs#L1-L4`). Desks and dispatch follow the lease
(`theuth/crates/roster/src/lib.rs#L461-L470`, `#L1279-L1291`). Records replicate by per-column LWW under an HLC
(`theuth/crates/replica/src/lib.rs#L1-L5`).

**Harness integration.** ACP client: the `agents.yaml` loader and public registry; one external harness is bound to one
ACP session (`theuth/crates/acp/src/lib.rs#L1-L6`). The registry fixture lists codex-acp and opencode
(`theuth/crates/acp/src/registry.rs#L313-L316`), and omp has its own module (`crates/acp/src/omp/`). Plus native theuth subagents, and PTY jobs for TUIs.

**Liveness, stalls, crash reconciliation:**
- **Orphan sweep** every 30 s. A `doing` checkout is lost when its engine has been unreachable for `orphan_secs` (120), or
  when it is stale: no progress note *and* no activity in its session tree for `stale_secs` (900), and the liveness probe
  says nothing runs now. Orphaning bumps `epoch` and wakes the owner with `resume`
  (`theuth/crates/tasks/src/ledger.rs#L2120-L2200`; defaults `theuth/crates/tasks/src/config.rs#L30-L44`).
- **Checkout epoch.** A resumed task's stale writer loses (`theuth/crates/tasks/src/types.rs#L174-L195`).
- **Restart.** A desk that reopens tells `ReleaseClaims(session)`, and the sweep un-claims wakes held by a session that is
  not the agent's desk (`theuth/crates/roster/src/lib.rs#L591`; `facts/theuth-desk-wake-protocol-code-map-2026-10-05`).
- **Child stalls.** The 155 activity monitor keeps a pure per-child `ChildActivity` with stall rules and state words an
  orchestrator can wait on (`theuth/crates/agent/src/activity.rs#L1-L6`).
- **Gaps:**
  - The `wakes` table has no claim timestamp (live schema: `id agent task reason key count claimed_by created_ms hlc`), so
    "claimed too long" cannot be queried.
  - Serving state, retry maps and child run-state are in memory. That caused the 2026-10-05 restart failure
    (`facts/theuth-orphaned-wake-claim-blocks-desk-after-restart-2026-10-05`).
  - External omp ACP workers die with an engine restart and are revived by hand
    (`procedures/theuth-recover-omp-acp-workers-after-engine-restart-by-pty-resume-2026-10-07`).

**Workspaces:** *no engine-managed per-task worktree.* `rg worktree` over tasks/roster finds nothing. The only rule is
prompt text in the base frame: "a separate target directory per worktree (deleted when the worktree is merged)"
(`theuth/crates/agent/src/frame.rs#L77-L78`). The t3code borrow list ranked "engine-managed worktrees (workspace as a
session field, cleanup policy, overlap guard)" as *next* (`facts/theuth-borrow-list-from-t3code-2026-10-03`). It has not
landed.

**Triggers:**
- clock: `cron`/`every`/`at`;
- loops: `until` shell checks with `max_runs`;
- **sensors** (`when:` Python checks with `fire`/`for`/`cooldown`/`mode`/`memory`/`expires`;
  `theuth/crates/schedule/src/file.rs#L401-L420`). Installed sensors: `pr_watch.py` (PR babysitter), `fs_exists.py`,
  `rss.py`, `dreaming.py`;
- per-fire `approvals`, `run_timeout` and `quiet` (`theuth/crates/schedule/src/file.rs#L376-L397`).

## A5 Coordination between agents
**Shared ledger, not a chat room.** The coordination substrate is `tasks.sqlite`: tasks, field-level `task_events`, wakes,
packets and threads. Its rows replicate across the mesh (`_replica_log` triggers, `facts/theuth-task-board-store-tasks-sqlite`).
The board is the one place agents learn about each other's work.

**Messaging, three channels:**
1. **Task threads** (59 documents). `task_note` appends blocks. Notes wake the owner, the reviewer and anyone mentioned
   (test `comment_wakes_owner_reviewer_and_mentions`, `theuth/crates/tasks/src/ledger.rs#L4100`). Mentions need no `@`:
   "a word that is an agent's name" (`theuth/crates/tasks/src/ledger.rs#L1543`). That is why a supervisor note naming
   five desks woke all five (`facts/sre-supervisor-note-naming-desks-wakes-them-2026-10-05`).
2. **Peer messages** (design 50). `send_message` to `session:`/`peer:`/`parent`/child names, receipts `accepted`, with a
   chain bound on peer-triggered continuations. `CHAIN_MAX = 256` (`theuth/crates/agent/src/communication.rs#L96`),
   enforced in the supervisor (`theuth/crates/agent/src/supervisor.rs#L1872-L1879`). The 2026-10-06 knowledge records a
   32-continuation refusal; the code now says 256. Trust the code.
3. **Child results** land as `<message>` input in the desk session itself, and the manual requires a follow-through
   (`theuth/crates/roster/src/desk.rs#L75-L81`).

**Handoffs.**
- Agent → agent: `task_reassign` (thread line plus a `Reassigned` wake) and the fit check.
- Agent → human: `desktop_handoff` writes a replicated `handoffs` row, which becomes an attention item until Done/Cancel
  on any engine (`theuth/crates/roster/src/handoff.rs#L1-L5`).

**Locks.** One checkout per task, bound to (session, engine, epoch). A second checkout gets "conflict: checked out by …
(epoch n)"; an orphaned checkout can be retaken with `epoch+1` (`theuth/crates/tasks/src/ledger.rs#L1407-L1440`). There
is no file/path lock, merge queue or conflict handling. Code conflicts are left to git and to the humans landing waves.

**Who decides who works next:**
- the ledger: dispatch by rank, then answer class, then age, one wake per agent;
- the desk: fit check and reassign;
- the manager: starting backlog tasks is by prompt convention only (A3).

No auction, voting or consensus. There is no swarm topology beyond the tree: manager desk → hired desks → their children.

**Ahead:**
- peers work across machines (mesh) and across harnesses (ACP children report back into the desk);
- A2A at the network boundary (`theuth/crates/a2a/src/lib.rs#L1-L4`).

**Gap:**
- no structured mail with read receipts that *starts* a cold desk: `accepted` is not acted
  (`facts/theuth-cold-desk-peer-message-queued-until-turn-2026-10-06`);
- mention parsing is name-anywhere;
- no merge-queue/conflict coordination for code teams.

## A6 Human in the loop & governance
**Approvals:**
- every tool call has a risk (`read_only | mutating | destructive`); `tools.policy` allows, asks or denies;
- the `Approval` actor publishes `ApprovalRequested` and waits for a `Resolve` from any UI (`theuth/crates/tools/src/approval.rs#L1-L2`);
- per-agent ceilings (`tools`, `mcp`) narrow it further;
- routines can run with `approvals: false` (`theuth/crates/schedule/src/file.rs#L392-L393`).

**The double (126) — unique in the field as far as this study goes.** A local model of the owner's own decisions
subscribes to `ApprovalRequested`/`AskRequested`, predicts, logs a prequential row, and at L2/L3 answers through the same
messages a UI sends. "Nothing inside the agent loop knows it exists" (`theuth/crates/double/src/lib.rs#L1-L7`). It is
gated per class: each class has a max rung, and L2 on a mutating class is an explicit opt-in
(`theuth/crates/double/src/types.rs#L290-L297`). So a team of desks can keep moving while the owner is away, under
a measured trust ladder.

**Decisions as packets.** An escalation to the user must carry a one-paragraph summary, a mini-board with a fixed
skeleton, options with exactly one recommended, pointers, and `if_wrong` (cost of a wrong answer)
(`theuth/crates/tasks/src/packet.rs#L1-L7`; desk rules `theuth/crates/roster/src/desk.rs#L85-L95`). The refusal text is
the recipe, so a desk retries once inside the same turn (`facts/theuth-mini-board-refusal-is-the-teaching-path-2026-10-06`).

**Attention inbox.** One derived list (`theuth/crates/roster/src/attention.rs#L1-L12`):
- permission prompts, pending `ask_user`, artifacts shown;
- tasks blocked on a decision/person, tasks in review with the user as reviewer;
- proposed hires, budgets at 80 %/paused, unreachable peers, unresolved hand-offs, secret requests.

Only dismiss/snooze is stored, and a newer version resurfaces. The engine decides *whether* an item alerts (per-kind class,
`roster.alert_delay_ms`); clients decide *how* (`theuth/crates/roster/src/alert.rs#L1-L7`). Surfaces:
- desktop HUD and `/attention`;
- macOS and Windows companions (`crates/desktop/companion`, `companion-windows`);
- Slack/Signal/Telegram channels;
- iOS (`tauri.ios.conf.json`).

**Interrupts.** `escalate` waits live and parks. A user line typed into a desk chat starts a turn. Peer messages and
revocations reach a busy child only at its next turn boundary (`facts/sre-desk-orchestration-lessons-2026-10-05`).

**Audit.** The field-level `task_events` (actor, field, before, after; `theuth/crates/tasks/src/ledger.rs#L406-L419`) is
tailed into an append-only `activity` table. Roster hires, pauses and budget thresholds plus schedule fires are added
(`theuth/crates/tasks/src/activity.rs#L1-L9`).

**Gaps:**
- review is a human bottleneck: 78/80 reviewer = user, 41 waiting;
- the attention "Unblock" action calls `task_move(todo)`, which the ledger refuses while blockers exist
  (`facts/theuth-attention-unblock-refused-while-blockers-exist-2026-10-08`; guard `theuth/crates/tasks/src/ledger.rs#L902-L918`);
- no policy-as-code beyond risk tiers (e.g. "agents may not push to main" is prompt text).

## A7 Cost, budgets & runaway control
- **Budgets.** Per agent, monthly only (`Budget::None | Monthly{amount, currency}`, `theuth/crates/roster/src/types.rs#L97-L108`).
  - Spend is fed from session cost deltas by a monotone tracker (`theuth/crates/roster/src/budget.rs#L43-L61`).
  - At 80 %: an activity row plus a task for the manager. At 100 %: unless an override exists for the month, the agent is
    paused, desks are re-synced (the desk closes), and an override task is filed
    (`theuth/crates/roster/src/lib.rs#L1423-L1475`; thresholds `theuth/crates/roster/src/budget.rs#L18-L41`).
  - Un-pausing lifts the cap for the month.
- **Write cap.** 20 distinct tasks per rolling 24 h outside the agent's reporting line
  (`theuth/crates/tasks/src/config.rs#L36-L37`). theuth's desk, the user's sessions and everything they spawn are exempt
  (`theuth/crates/tasks/src/ledger.rs#L463-L500`). The exemption is Sky's ruling after the cap locked out his own theuth
  desk (`decisions/theuth-write-cap-and-agent-sessions-rulings-2026-10-05`).
- **No-progress back-off.** Two consecutive wakes that change no field start a back-off of 120 s doubling to 1800 s. Any
  progress resets it, and so does a human send-back (`theuth/crates/tasks/src/ledger.rs#L2075-L2093`; test
  `no_progress_backoff_and_human_reset` at `theuth/crates/tasks/src/ledger.rs#L3830`).
- **Other loop defences:**
  - delivery back-off after a non-yielded turn (5 s → 5 min);
  - one nudge per turn, then `no_continuation` plus a manager wake (`theuth/crates/roster/src/desk.rs#L436-L467`);
  - `roster.max_rounds` = 3 child rounds per task per episode (`theuth/crates/roster/src/desk.rs#L77`);
  - a peer chain bound of 256 continuations since the last external input;
  - routine `max_runs`/`until`/`run_timeout`;
  - one ask per stuck task (`escalate_refuses_the_same_actors_second_ask_to_the_user`, `theuth/crates/tasks/src/ledger.rs#L5924`).
- **Kill switch.** Pause an agent (Agents page switch / `agent_pause`). Pausing closes the desk and holds wakes. Cancel
  jobs. A team-wide STOP is done by convention (`procedures/theuth-manager-desk-handling-wakes-mentions-answers-supervisor-fires-2026-10-05`).
- **Model routing for cost.** Per agent `model`/`effort` in the file. Without `model:` the user's subagent default applies
  (148 E3, `theuth/crates/roster/src/desk.rs#L53-L58`). The local `decision` model (57) does bulk classification at zero
  API tokens (`theuth/crates/decision/src/lib.rs#L1-L5`).
- **Cost attribution.** `model_calls` rows per session (`theuth/crates/agent/migrations/0015_session_lifecycle.sql#L18`)
  roll into per-agent monthly spend.
  - *Not found:* attribution to task, project or goal, and budget scopes for company, project or lifetime (Paperclip has
    both, `facts/paperclip-architecture-recon-2026-09-24`).
  - *Not found:* cancellation of queued work at the cap. Wakes are held by the paused gate, not cancelled.
- **Live:** none of the five agent files sets a `budget`, so the auto-pause has never been exercised on the live roster.

## A8 Memory, knowledge & learning
- **Per-agent memory.**
  - A desk journal file `roster/journal/<name>.md`, written at episode close as Done / Open / Next / Notes, ≤ 8 KiB
    (`theuth/crates/roster/src/desk.rs#L17-L18`, `#L35-L38`). It is re-injected at the head of the next episode with the
    charter and manual (`theuth/crates/roster/src/desk.rs#L174-L206`).
  - The desk session's own 137 ENDLESS summary-tree memory, with per-agent `memory` mode and `view_bytes`
    (`theuth/crates/roster/src/types.rs#L241-L250`).
- **Shared knowledge.** An OKF bundle with FTS5 search, its own git history (gix) and read-at-commit
  (`theuth/crates/knowledge/src/lib.rs#L1-L9`). `<knowledge>` hints are injected per turn. Concepts carry
  `private|team|shared` visibility.
- **Learning from outcomes.** Dreaming (136) is a nightly routine:
  - it tidies knowledge and mines the day's friction into *cases*;
  - it proposes bounded skill edits behind a *replay gate*, with a journal and undo (`theuth/crates/dreaming/src/lib.rs#L1-L5`).

  The double learns the owner's approvals (126). Autoresearch (140) runs measured experiment loops with a ratchet
  (`theuth/crates/autoresearch/src/lib.rs#L1-L5`).
- **Skills.** Global markdown skills and skill repos, loaded on demand. *Not found:* per-agent skill sets in `AgentFile`
  (A1).
- **Context engineering for desks:**
  - the identity-first frame (charter + capabilities + journal + manual);
  - a `<roster>` block on assigned wakes;
  - the task `<attached …>` references (design 66, `theuth/crates/tasks/src/types.rs#L255-L259`).
- **Evals.** Replay of cases (dreaming), prequential scoring (double), autoresearch benches. *Not found:* an eval of desk
  behaviour itself, e.g. "disposition rate per wake" or "reviews bounced".
- **Ahead of the field:** knowledge plus dreaming plus double together form a learning loop most control planes do not
  have. Paperclip's memory is "not built" (`facts/paperclip-architecture-recon-2026-09-24`).

## A9 Observability & UX
- **Agents** (`/agents`, `/agents/new`, `/agents/$name/$tab`; components in `crates/desktop/ui/src/components/agents/`):
  - the org chart with live session dots, a list and a Hiring inbox;
  - the seven-step wizard;
  - detail tabs Overview · Charter · Runtime & Access · Runs · Tasks
    (`theuth/crates/desktop/ui/src/components/agents/AgentDetail.tsx#L39-L45`).
- **Tasks board** (`/tasks`, `Board.tsx`):
  - columns by state; cards show reviewer chips, blocked/waiting badges and a progress dot while checked out;
  - drag between Backlog and Todo, plus a Start button (`theuth/crates/desktop/ui/src/components/tasks/Board.tsx#L299-L300`);
  - task detail with the thread and packets.
- **Routines** (`/routines`, `/routines/new`, `/routines/$name/$section`): RoutinesView, RoutineDetail, RoutineChat.
- **HUD at `/`**: the attention inbox plus a BOARD panel (every task in every project by state, after the 128 R6 fix).
  `/attention/$id` shows the decision screen. There is an activity feed, and per-child activity state words (155).
- **Transcripts:** every desk and child is a session (`/c/$session`). The sidebar Desks group nests each desk's live
  subagents (`facts/theuth-desktop-agents-page-and-desks-sidebar-seams-2026-10-05`).
- **Surfaces:**
  - desktop (Tauri, macOS/Windows/Linux) and iOS;
  - macOS and Windows companions, TUI, web;
  - Slack/Signal/Telegram, voice/talk.
- **Onboarding to a first useful result.** A one-laptop user with no agent files sees no desks at all
  (`theuth/crates/roster/src/lib.rs#L464-L466`). The team appears only after hiring in chat or the wizard.
- **Gaps:**
  - no per-agent metrics (wake → disposition latency, bounce rate, cost per task);
  - no wake/claim timeline;
  - the Runs tab lists episodes without cost or outcome (*cost per run not found* in `episodes`:
    `theuth/crates/roster/src/store.rs#L95-L119`);
  - no traces beyond Grafana MCP access;
  - the 2026-10-07 HUD "everything 0" bug shows how easily these views can lie
    (`facts/theuth-hud-outcomes-panel-counted-only-todays-movers-2026-10-07`).

## A10 Extensibility & integration
- **Harnesses:** ACP client and registry (codex-acp and opencode in the registry fixture, `theuth/crates/acp/src/registry.rs#L313-L316`;
  omp via `crates/acp/src/omp/`).
  External agents run as children placed per task. Remote harnesses are reached through `nodes` (paired machines).
- **MCP.** Servers are JSON files under `~/.theuth/mcp/`, hot-reloaded, with per-server/tool risk gating. Agents get an
  `mcp` allowlist (`theuth/crates/roster/src/types.rs#L226-L228`). Live MCP tools include Jira/Confluence, GitHub,
  Grafana/Argo, Slack.
- **A2A.** An optional loopback listener, outbound clients, peer/export bindings; off by default
  (`theuth/crates/a2a/src/lib.rs#L1-L4`).
- **Engine API.** JSON-RPC (`crates/rpc`). Tasks and roster are actor messages with ts-rs exported types (`#[cfg_attr(feature = "ts"…)]`
  throughout `types.rs`).
- **Webhooks:** *not found* as inbound triggers. The equivalents are sensors (`pr_watch.py`, `rss.py`, `fs_exists.py`)
  and channels.
- **Plugins:** plugin crates on the runtime bus (double, dreaming, autoresearch are each "a plugin crate"). Skills and
  skill repos. Apps (OpenUI).
- **Gap:** no adapter SDK for third parties to add a harness without ACP. ACP *is* the adapter contract, which is a
  reasonable choice. No inbound webhook → task. No tracker sync adapters.

## A11 Engineering, licence & traction
- **Stack:**
  - Rust workspace, 50 crates, ~558k lines of `.rs`;
  - desktop UI in TypeScript/React (Tauri): 1,535 files, ~136k lines, 257 UI test files;
  - SQLite everywhere (`tasks.sqlite`, `roster.sqlite`, `sessions.sqlite`);
  - iroh mesh, kameo actors, ts-rs bindings.
- **Process:** 575 files in `docs/design` (numbered designs plus as-built records).
- **Cadence:** 3,436 commits since the first on 2026-09-09; 2,689 since 2026-09-24 (the Paperclip recon). Authors: Sky
  Moore 3,400; theuth worker identities 36.
- **Licence:** no LICENSE file at the repo root. The remote is `BaseDatum/Theuth`: **private**, 0 stars, created
  2026-09-22 (GitHub MCP `search_repositories`). No public traction or issue tracker. The "users" are Sky and his agent
  teams, and failures are recorded in the knowledge base rather than in issues.
- **Tests:** team-core 410 test attributes, whole tree far more. Ledger tests cover the transition table, checkout
  conflict and epoch, dispatch ordering, back-off, write cap, escalate paths and packet caps
  (`theuth/crates/tasks/src/ledger.rs#L2751`, `#L3482`, `#L3752`, `#L3830`, `#L3964`, `#L5138`-`#L6169`).
- **Maturity:** a month old and moving fast. Several core files are god-files (ledger 6.2k, attention 3.6k, roster lib 3.6k
  lines). It is single-owner by design.

## Paperclip borrows of 2026-09-24 — landed or not
Source list: `facts/paperclip-architecture-recon-2026-09-24` ("Worth borrowing") plus the 2026-09-25 UI rulings.

| # | Borrow | Status | Where (cite) | Deviation from Paperclip |
|---|---|---|---|---|
| 1 | Checkout lock | **Landed** | `theuth/crates/tasks/src/ledger.rs#L1407-L1440`, `theuth/crates/tasks/src/types.rs#L174-L195` | bound to (session, engine, **epoch**), not to a run id header on every mutation; orphaned checkouts can be retaken |
| 2 | Final-disposition checklist | **Landed** | `theuth/crates/tasks/src/types.rs#L399-L441`, `theuth/crates/roster/src/desk.rs#L436-L467` | five dispositions (adds `delegated`, `continuing`) + `DoneBecause`; enforced by a turn-end nudge table, not only a skill |
| 3 | Coalesced wake queue | **Landed** | `theuth/crates/tasks/src/types.rs#L364-L382`, `theuth/crates/tasks/src/ledger.rs#L392-L420` | key `(agent, task, reason)` + `count`; no claim timestamp |
| 4 | No-progress back-off | **Landed** | `theuth/crates/tasks/src/ledger.rs#L2075-L2093`, `theuth/crates/tasks/src/config.rs#L38-L39` | 120 s doubling to 1800 s after 2 no-change wakes; human send-back resets |
| 5 | Cross-task write cap | **Landed, amended** | `theuth/crates/tasks/src/ledger.rs#L463-L500`, `theuth/crates/tasks/src/config.rs#L36-L37` | 20 tasks per rolling 24 h; reporting line free; theuth + user + their spawn uncapped (99 WRITE-CAP-LINE) |
| 6 | Scoped wake | **Landed** | `theuth/crates/roster/src/desk.rs#L233-L248` | "Do not switch to another task until this wake is handled"; plus a `<roster>` block, which Paperclip omits |
| 7 | Pending-approval hires | **Landed** | `theuth/crates/roster/src/types.rs#L235-L236`, `theuth/crates/roster/src/lib.rs#L475`, `theuth/crates/roster/src/proposals.rs#L8-L42` | hire = a task owned by the manager + an attention item; plus chat `agent_draft` cards |
| 8 | Budget auto-pause | **Partial** | `theuth/crates/roster/src/lib.rs#L1423-L1475`, `theuth/crates/roster/src/budget.rs#L18-L41` | per-agent monthly only; no company/project/lifetime scopes; queued wakes held, not cancelled; unused live (no agent has a budget) |
| 9 | Field-level activity log | **Landed** | `theuth/crates/tasks/src/ledger.rs#L406-L419`, `theuth/crates/tasks/src/activity.rs#L1-L9` | `task_events` field/before/after + derived `activity` feed with roster/schedule records |
| 10 | Org chart + hiring wizard UI | **Landed** | `theuth/crates/desktop/ui/src/components/agents/OrgChart.tsx#L20-L23`, `theuth/crates/desktop/ui/src/components/agents/wizard.ts#L5` | no harness field per agent, no rename, no Activity tab, no icon picker (recorded deviations) |
| 11 | Routines | **Landed, extended** | `theuth/crates/schedule/src/file.rs#L367-L420` | `agent:` fire = task per fire (coalesced); plus `until` loops, sensors, `approvals`, `runtime`, `quiet` |

**Not copied, on purpose:**
- agent ↔ adapter 1:1 binding (theuth places per task);
- server-authoritative workspace;
- server-initiated runs (theuth desks are long-lived sessions on the lease holder);
- company-wide visibility without routing (theuth routes by capabilities).

**Still missing from the Paperclip list:**
- goals tree;
- per-agent concurrent runs;
- run-id header on mutations;
- budget scopes and cost attribution per issue/project;
- "auto-repair capped at 5 then board escalation". theuth has one nudge then a manager wake, which is close in spirit.

theuth's own gate for reviews is "a review that goes nowhere twice → blocked + manager woken"
(`decisions/theuth-desk-protocol-employee-rulings-2026-09-25`).

## How the team features behave in anger
Taken from the knowledge base (2026-09-25 → 2026-10-10). The real-use record is mostly the five SRE desks on the DR
backlog (2026-10-05/06) plus smokes. Recurring pain points, most damaging first:

1. **Cold desks do not act on peer messages.** `send_message` to five desks returned `accepted` for all five. Two desks
   that had been idle since the night stop never turned. Only a board wake (a `task_note` on a task they own) or a line
   typed by Sky started them. A fed `todo` task can sit unclaimed for 28 minutes.
   `facts/theuth-cold-desk-peer-message-queued-until-turn-2026-10-06`,
   `facts/theuth-cold-desk-ignores-peer-message-use-task-thread-2026-10-06`.
2. **Stale/stuck wakes after restarts.** After the 2026-10-05 restart, `judge_turn` treated every pre-restart child as
   live, because run-state lives in memory. The judgement deferred forever, the claim stayed, and `dispatch()` skipped
   theuth's and nikolai's desks as `busy`. Desk-routed supervisor fires reached the ledger but did nothing. Fixed by
   WAKE-LIVE-CHILD (D1–D7: live = Running/Awaiting/Approval now, release with back-off, held-wake logging).
   `facts/theuth-orphaned-wake-claim-blocks-desk-after-restart-2026-10-05`,
   `decisions/theuth-desk-liveness-rules-wake-live-child-2026-10-05`, `procedures/theuth-diagnose-desk-receiving-no-wakes`.
3. **The orphan sweep stole checkouts from busy desks.** A desk idle between two omp children running `pulumi preview` was
   orphaned twice (epoch reached 5). All five SRE desks were orphaned in the same minute. The fix: the liveness probe in
   the sweep (`theuth/crates/tasks/src/ledger.rs#L2120-L2127`). `decisions/theuth-desk-liveness-rules-wake-live-child-2026-10-05`.
4. **`escalate` refused with "you are the top of the line".** Two causes:
   - from a desk session that predates the engine upgrade (`task_inbox` also empty);
   - from desk sessions where Sky had typed into the desk chat, re-rooting the session as actor `user`.

   Code: any non-`agent:` actor is the top (`theuth/crates/tasks/src/ledger.rs#L1619-L1622`). Workaround: restore the
   blocked state, then have the manager raise the packet.
   `facts/theuth-escalate-top-of-line-refusal-old-desk-session-2026-10-06`,
   `facts/theuth-desk-session-rerooted-as-user-after-human-chat-line-2026-10-06`,
   `facts/theuth-escalate-packet-shapes-and-chat-session-refusal-2026-10-06`.
5. **Supervisor routines.** v1 timed out (too many tool calls per fire). v2 was desk-routed and silently did nothing
   (head-of-line blocked, cause 2). The working shape is lean: ≤ 6 calls, deltas only, a 6-minute cadence with a 5-minute
   cap. A supervisor note that *named* desks was a mention storm: 5 extra wakes, each reply re-woke the manager, ×3.
   `procedures/theuth-supervisor-routine-for-agent-desks`, `facts/sre-supervisor-run-lessons-2026-10-05`,
   `facts/sre-supervisor-note-naming-desks-wakes-them-2026-10-05`.
6. **HUD zeros.** The OUTCOMES panel counted only tasks that moved *today*, so an unmoved board read "everything 0". Fixed
   as 128 R6 BOARD. `facts/theuth-hud-outcomes-panel-counted-only-todays-movers-2026-10-07`.
7. **The write cap locked out the manager.** theuth's own desk hit the cap. Ruling: theuth, the user and their spawn are
   exempt; hired agents write freely to their line. `decisions/theuth-write-cap-and-agent-sessions-rulings-2026-10-05`.
8. **"Unblock" does nothing.** On every surface (desktop, both companions) it calls `task_move(todo)`, which the ledger
   refuses while blockers exist. `facts/theuth-attention-unblock-refused-while-blockers-exist-2026-10-08`.
9. **Prompt-only rules drift.**
   - Desks self-started backlog tasks twice in one night, against the rule; there is no code guard
     (`theuth/crates/tasks/src/ledger.rs#L891`). `decisions/sre-desks-do-not-self-start-backlog-tasks`.
   - A desk let a child touch `~/.theuth/mcp`, and a revocation arrived 3 minutes late.
     `facts/sre-desk-orchestration-lessons-2026-10-05`.
10. **Identity and config seams.**
    - A hired desk answered "I'm theuth" (fixed by the identity-first frame).
      `decisions/theuth-desk-protocol-employee-rulings-2026-09-25`.
    - An Agents-page model change did not reach the open desk, because PutAgent updates the read model before reconcile
      (fixed in 148). `warnings/theuth-roster-putagent-updates-read-model-before-reconcile`.
    - A scratch engine with `hub: never` opens no desks. `warnings/theuth-scratch-engine-mesh-hub-never-opens-no-desks`.
11. **Restarts cost children.**
    - Desk children show `stored` after a restart and must be revived by a message.
      `procedures/theuth-desk-child-liveness-after-engine-restart`.
    - omp ACP workers die and are resumed by hand in PTYs.
      `procedures/theuth-recover-omp-acp-workers-after-engine-restart-by-pty-resume-2026-10-07`.
    - A chat-owned task also woke the desk (duplicate work risk). `procedures/desk-duplicate-wake-on-chat-owned-task`.
12. **Cost leak.** The theuth desk's endless compactor, running on Haiku 5.5, retried a deterministic Anthropic 400 about
    every 10 s (title of `facts/theuth-anthropic-400-thinking-enabled-haiku-5-5-loop-2026-10-10`, not opened). No loop
    defence covers a background model call that fails the same way forever.
13. **The board stalls when Sky stops.** Live: since 2026-10-06 18:34 nothing has moved. 41 tasks wait in `review` and 78/80
    name `user` as reviewer. The machinery is idle because every review routes to one human (counts from `tasks.sqlite`).

**Pattern.** The ledger rules held. Failures cluster at the *edges*:
- delivery to a session whose state is in memory;
- identity derived from the session root;
- rules that live only in prompts;
- a single human reviewer.

## Map to the three questions
### Q1 What do they offer versus theuth?
theuth's column for the comparison table. The competitor reports fill "theirs".

| Axis | theuth has | theuth lacks | theuth ahead |
|---|---|---|---|
| A1 | file agents, charter, model/effort/tools/mcp/budget/pin/memory; chat + wizard + propose hiring | templates/catalog, role taxonomy, per-agent skills, rename, versioned charters | harness chosen per task, not bound per agent |
| A2 | reporting lines, fit-check routing, escalate with packets, write cap, ceilings | multi-company isolation, task ACLs, reviewer pools; authority keyed on actor string | chat-first drafts; double answers as owner |
| A3 | 7 states, one owner, reviewer, blockers (3 kinds) + inverse, parent tree, `done_when` + DoneBecause, packets, rank | goals/milestones, tracker import/sync, templates; backlog start unguarded | packets/mini-boards; code-enforced transition guard |
| A4 | wake ledger, dispatch on lease holder, nudge table, orphan sweep with liveness probe, epoch, placement rules, VMs, ACP | per-task worktrees, PR/CI loop, wake claim time, durable child state, auto-revival of ACP workers, per-agent parallel runs | leaderless mesh + placement + sandboxes + sensors |
| A5 | ledger + threads + peer messages + checkout lock | cold-desk-starting mail, merge queue, `@`-only mentions | cross-machine, cross-harness peers; A2A |
| A6 | risk policy, approvals, attention inbox, alerts, companions, channels, audit log | review flow (human bottleneck); a working Unblock | the double; packets with `if_wrong` |
| A7 | monthly agent budgets with pause, write cap, back-offs, chain bound, max_rounds | budget scopes, per-task cost, guard on background-call loops | local decision model for cheap classification |
| A8 | desk journal, endless memory, knowledge + git history, dreaming, double, autoresearch | per-agent skills, desk-behaviour evals | the whole learning loop |
| A9 | org chart, board, routines, HUD, attention, transcripts, many surfaces | per-agent metrics, wake timeline, run cost | breadth of surfaces (companions, channels, voice, iOS) |
| A10 | ACP, MCP, A2A, RPC, skills, plugin crates, sensors | inbound webhooks → task, tracker adapters | ACP as the adapter contract |
| A11 | 558k-line Rust + 136k-line TS, 3.4k commits in a month, many tests | public release, licence, users beyond Sky | velocity |

### Q2 What can we learn / borrow?
For the `ours` report these are the *slots* where a competitor technique would land. Each names the theuth mechanism a
borrow would replace or extend, and the cost of being wrong. The other eight reports supply the competitor mechanisms.

1. **Durable run/claim records** (Paperclip-style runs table; Symphony/Gastown-style persisted agent state). Replace the
   in-memory `serving`/retry/child maps with rows: claim time, run id, last heartbeat. Seam: `wakes`
   (`theuth/crates/tasks/src/types.rs#L364-L382`), `judge_turn` (`theuth/crates/roster/src/lib.rs#L1040`). *Cost if
   wrong:* low; it is additive, plus one migration.
2. **Mail that wakes.** Any message addressed to an agent becomes a ledger wake, not queued session input. Seam: the
   `send_message` intake (`theuth/crates/agent/src/supervisor.rs#L1872-L1879`) → `Ledger::wake`. *Cost:* medium; more
   wakes, so it needs the back-off.
3. **Workspace per task** (multica, agent-orchestrator, vibe-kanban, Symphony style): an engine-created worktree and
   branch recorded on the task, a cleanup policy and an overlap guard. Seam: `Task` + `place.rs` + spawn cwd. *Cost:*
   medium; disk use and stale worktrees.
4. **Tracker as queue** (Symphony polls Linear; others GitHub issues). Optional import plus state mirror, keeping theuth
   the source of truth for the agent loop. Seam: `external` (`theuth/crates/tasks/src/types.rs#L227-L228`) and
   `tools.rs#L198`. *Cost:* medium; two-way sync conflicts.
5. **PR/CI → task feedback.** Reuse `pr_watch.py` as an engine-owned watcher per task with a PR link. It wakes the owner on
   red CI or review comments, and moves the task to review on green. Seam: schedule sensors + `WakeReason`.
   *Cost:* low–medium.
6. **Goals tree.** Project and goal rows with `done_when` and roll-up. Seam: `project` label → record; `rank.rs`. *Cost:*
   low if optional.
7. **Budget scopes and cost attribution.** Charge `model_calls` to the checked-out task; add project/lifetime budgets.
   Seam: `budget.rs` + the `model_calls` migration. *Cost:* low.
8. **Authority from identity, not session root.** Resolve the caller's agent from the desk mapping. Seam:
   `theuth/crates/tasks/src/ledger.rs#L1619-L1622` + `CallerOf`. *Cost:* low.

### Q3 How should theuth improve?
Ranked by value ÷ effort. Each names its landing seam.

1. **Guard backlog→todo in code** (manager/theuth/user only, or a per-project flag). Seam: `guard()`,
   `theuth/crates/tasks/src/ledger.rs#L891`. Effort XS. Value: retires a recurring prompt rule.
2. **Fix Attention "Unblock".** Make it a composite: clear resolvable blockers, or open the decision. Seam:
   `crates/roster/src/attention.rs` + the HUD and companion actions + `ledger.rs#L902-L918`. Effort S. Value: a broken
   button on every surface.
3. **Add `claimed_ms` to `wakes` and a "wake held > N min" attention item.** This turns the 30-minute forensic chain into a
   badge. Seam: `crates/tasks/src/store.rs` (migration 0009), `attention.rs`. Effort S.
4. **Escalation authority from the desk identity.** A desk session re-rooted by a typed human line must still escalate as
   its agent. Seam: `ledger.rs#L1619-L1622`, roster `CallerOf`/`IdentitySource`. Effort S.
5. **Messages to a cold desk raise a wake.** Seam: agent communication intake → roster `DeliverWake` via a `Mention` or
   `Manual` wake. Effort S–M. Value: removes pain point 1.
6. **Review flow.** Changes:
   - default reviewer = the owner's manager desk, with the user only when `if_wrong` is high;
   - a batched review digest packet ("approve N low-risk reviews");
   - double-assisted review classes.

   Seam: `ledger.rs` effective reviewer, `packet.rs`, `crates/double`. Effort M. Value: the live board is frozen on this.
7. **Durable child/run state** so restarts neither blind `judge_turn` nor kill ACP workers (auto-resume omp sessions).
   Seam: `crates/agent` supervisor + `crates/acp`; design 146 RESTART-CLARITY is adjacent. Effort M–L.
8. **Per-task worktree.** Seam: `Task` + `place.rs` + spawn cwd + cleanup in the sweep. Effort M. Value: high for code
   teams; matches the tier-1 field.
9. **PR/CI watcher bound to a task** (point 5 above). Seam: `crates/schedule` sensors + `tasks`. Effort M.
10. **Per-agent run metrics.** Episode cost, wakes per disposition, bounce rate in the Runs tab. Seam: roster `episodes` +
    `model_calls` + `AgentDetail.tsx`. Effort S–M.
11. **Goals/projects as records with roll-up**, and **tracker import**. Seam: `crates/tasks` store + Board UI. Effort M each.
12. **A loop guard for background model calls** (a deterministic error retried forever). Seam: the endless compactor pump
    in `crates/agent`. Effort S. Value: pain point 12.

## Their weaknesses and failure modes
These are theuth's own, from code smells, knowledge warnings and design caveats:
- **In-memory truth on a durable ledger.** Desk serving state, delivery retries, the `delivered` map and child run-state
  live in actor memory (`facts/theuth-desk-wake-protocol-code-map-2026-10-05`). Every restart is a reconciliation event,
  and two of the worst incidents came from it.
- **Head-of-line blocking by design.** One claimed wake per agent (`theuth/crates/tasks/src/ledger.rs#L1922-L1926`). A turn
  that never finishes holds every later wake for that agent. The "wakes held" log line is the only signal.
- **Identity by string.** `agent:` prefix checks decide authority (`theuth/crates/tasks/src/ledger.rs#L1619-L1622`).
  Mentions match any word equal to an agent's name (`theuth/crates/tasks/src/ledger.rs#L1543-L1548`).
- **Prompt-carried protocol.** Much of the employee behaviour lives in the desk manual, a long block at the head of every
  episode (`theuth/crates/roster/src/desk.rs#L51-L95`):
  - never work in the desk session;
  - fit check first;
  - one escalation per stuck task;
  - packet shapes.

  Refusal texts teach the rest. That is cheap and flexible, but it drifts (self-started backlog, a child touching config).
- **The human is the default reviewer.** 78/80 live tasks. The system works only as fast as Sky reviews, and the board
  has been frozen since 2026-10-06.
- **Governance unused live.** No `budget`, `tools`, `mcp` or `reports_to` on any of the five live agent files. The cap,
  ceiling and line features are untested on the real roster.
- **God-files:** `ledger.rs` 6,237 lines, `attention.rs` 3,602, roster `lib.rs` 3,563.
- **Code-team loop missing.** No worktree per task, no PR/CI → task, no merge queue. External trackers are links only.
- **Single-tenant, single-owner.** No company/workspace isolation and no task ACLs. Fine for Sky, but a ceiling for a team
  product.
- **Visibility can lie.** HUD zeros, `accepted` receipts that do not mean acted, and a wakes table without claim time make
  "is anyone working?" hard to answer.

## Cites self-check
All cites below were re-opened at the end of the run: one cell read each range with `read_file(offset, limit)` and
checked it for the quoted keywords. **59/59 held.** Most load-bearing:

| Cite — result | Cite — result |
|---|---|
| `theuth/crates/tasks/src/types.rs#L174-L195` — held | `theuth/crates/tasks/src/types.rs#L364-L382` — held |
| `theuth/crates/tasks/src/types.rs#L399-L441` — held | `theuth/crates/tasks/src/ledger.rs#L392-L420` — held |
| `theuth/crates/tasks/src/ledger.rs#L463-L500` — held | `theuth/crates/tasks/src/ledger.rs#L890-L925` — held |
| `theuth/crates/tasks/src/ledger.rs#L891` — held (start is unguarded) | `theuth/crates/tasks/src/ledger.rs#L1407-L1440` — held |
| `theuth/crates/tasks/src/ledger.rs#L1619-L1622` — held | `theuth/crates/tasks/src/ledger.rs#L1919-L1975` — held |
| `theuth/crates/tasks/src/ledger.rs#L2060-L2073` — held | `theuth/crates/tasks/src/ledger.rs#L2075-L2093` — held |
| `theuth/crates/tasks/src/ledger.rs#L2120-L2127` — held | `theuth/crates/tasks/src/ledger.rs#L1543-L1548` — held |
| `theuth/crates/tasks/src/config.rs#L30-L44` — held | `theuth/crates/tasks/src/tools.rs#L198` — held |
| `theuth/crates/tasks/src/place.rs#L1-L14` — held | `theuth/crates/roster/src/types.rs#L210-L253` — held |
| `theuth/crates/roster/src/lib.rs#L1423-L1475` — held | `theuth/crates/roster/src/lib.rs#L461-L477` — held |
| `theuth/crates/roster/src/lib.rs#L1037-L1060` — held | `theuth/crates/roster/src/desk.rs#L233-L262` — held |
| `theuth/crates/roster/src/desk.rs#L436-L467` — held | `theuth/crates/roster/src/desk.rs#L51-L95` — held |
| `theuth/crates/roster/src/attention.rs#L1-L12` — held | `theuth/crates/mesh/src/lease.rs#L1-L4` — held |
| `theuth/crates/double/src/lib.rs#L1-L7` — held | `theuth/crates/agent/src/communication.rs#L96` — held |
| `theuth/crates/schedule/src/file.rs#L394-L397` — held | `theuth/crates/desktop/ui/src/components/agents/wizard.ts#L5-L6` — held |

The other 29 re-opened cites also held: packet, escalate, activity, budget, proposals, journal, alert, pin, handoff,
replica, dreaming, knowledge, autoresearch, supervisor chain check, the 155 activity monitor, frame worktree rule, sensors,
ACP registry, approval actor, OrgChart, AgentDetail, Board, roster episodes, the `model_calls` migration, and the test-name
anchors (`#L3830`, `#L4100`, `#L5924`).

**Corrected while writing:**
- **HEAD drift.** The brief named b4e5833a; the checkout is now c22556ea. The diff touches only 7 desktop meeting,
  notebook and CodeMap files, so every team-seam cite is valid at both commits.
- **Peer chain limit.** `facts/theuth-cold-desk-peer-message-queued-until-turn-2026-10-06` says 32 continuations. The
  code says `CHAIN_MAX = 256`. Trusting the code.
- **ACP harness list.** The registry fixture names codex-acp and opencode only; omp is wired through its own module.
  Wording corrected.
- **Line ranges added.** Three module-doc cites (sandbox, acp, a2a) were given ranges `#L1-L4` / `#L1-L6` / `#L1-L4`.

**Not verified:**
- the Haiku 400-loop fact: title only, the entry was not opened;
- whether a paused agent's queued wakes are dropped or only held (inferred from the DeliverWake paused gate in the
  2026-10-05 code map, not re-read);
- iOS/companion feature parity.
