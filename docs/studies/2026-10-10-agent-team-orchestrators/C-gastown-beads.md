# Gas Town + Beads — Yegge's multi-agent workspace manager (recon for the agent-teams study, 2026-10-10)

Clones: `gastown` at 649b832 (main, 2026-07-23), `beads` at eb20436 (main, 2026-10-10). Read-only source recon; no builds or runs.
theuth cites are against `/Users/Sky.Moore/Develop/theuth` at c22556ea; the `ours` report is the authority on theuth.

## Executive summary
- **What it is.** Gas Town (`gt`, Go, MIT, 18.3k★) runs 20–30 CLI coding agents, mostly Claude Code, in tmux sessions with one git worktree each. A Go daemon keeps a fixed cast of LLM roles alive: Mayor, Deacon, Witness, Refinery, Polecats, Crew, Dogs and Boot (`gastown/README.md#L11-L16`, `gastown/docs/glossary.md#L24-L50`).
- **Its substrate is Beads** (`bd`, Go, MIT, 27.8k★): a Dolt-backed graph issue tracker for agents. Every piece of state is a bead: tasks, agents, mail, escalations, convoys and workflow steps (`beads/README.md#L15`, `beads/internal/types/types.go#L779-L797`).
- **Core protocol.** Each agent has a "hook" (`hook_bead` on its agent bead). GUPP says "if there is work on your Hook, YOU MUST RUN IT". The daemon mails the Witness when hooked work has not moved for 30 minutes (`gastown/docs/glossary.md#L10-L11`, `gastown/internal/daemon/lifecycle.go#L1138-L1170`).
- **Idea 1 for theuth: layered, restart-first supervision.** The chain is daemon (3 min tick) → Boot → Deacon → per-rig Witness → polecats. Witnesses are LLMs that must run a deterministic Go `gt patrol scan`. Respawns are capped and storms are broken (`gastown/internal/daemon/daemon.go#L894-L1000`, `gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L40`, `gastown/internal/witness/spawn_count.go#L66-L93`).
- **Idea 2 for theuth: Beads' work model.** It has a ready front with atomic claim-next, leases with TTL, heartbeat and granting node, reaper reclaim, typed edges (`conditional-blocks`, `waits-for`, `delegated-from`), gates, and formulas → molecules/wisps (`beads/issueops/readyclaimer.go#L5-L44`, `beads/internal/types/types.go#L57-L73`, `beads/internal/types/types.go#L1427-L1457`).
- **Idea 3 for theuth: the Refinery's bisecting merge queue.** It takes batches of ≤ 5, retries once before blaming anyone for a flaky failure, bisects to the culprit, and sends conflicts back to a worker as tasks (`gastown/internal/refinery/batch.go#L10-L35`, `gastown/internal/refinery/engineer.go#L1562-L1624`).
- **Also worth taking:**
  - rate-limit-vs-crash restart accounting (`gastown/internal/daemon/usage_limit.go#L5-L14`);
  - the cooperative turn-boundary nudge queue with TTLs (`gastown/internal/nudge/queue.go#L1-L51`);
  - an E-stop that exempts the coordinator (`gastown/internal/estop/estop.go#L1-L9`);
  - severity escalation with stale re-escalation (`gastown/docs/design/escalation.md#L41-L63`).
- **Weak where theuth is strong.**
  - No dollar budgets: cost is counted after the fact from transcripts.
  - Every agent runs `--dangerously-skip-permissions`, so there are no approvals.
  - No real UX beyond CLI, tmux and a convoy dashboard (`gastown/internal/config/roles/polecat.toml#L13`, `gastown/internal/cmd/costs.go#L72-L79`).
- **Fragile substrate.**
  - Control works by typing into TUIs: Rewind, Escape and a 600 ms readline wait, plus blind dialog dismissal that broke on a Claude Code UI change (#4818).
  - Reapers have closed live work (#4890, #4911).
  - Dolt needs 72 doctor check files (`gastown/internal/tmux/tmux.go#L1836-L1869`, `gastown/internal/witness/handlers.go#L2245-L2251`).
- **Activity.** Gas Town main has been frozen since 2026-07-23 (last release v1.2.1, 2026-06-06). PRs closed in October were closed unmerged. Energy has moved to Gas City (`gastownhall/gascity`, "orchestration-builder SDK", 1.4k★, commits today) and to Beads (v1.3.2-rc.2 today).
- **Fit with theuth.** Its substrate (tmux and per-harness hook installers for 9 runtimes) is exactly what Sky ruled out on 2026-10-09 in favour of ACP. Its *protocols* are harness-neutral and transfer.
- **Verdict.** Borrow the protocols: hook re-injection, the stall/orphan matrix, respawn caps, rate-limit pauses, the freeze switch, typed gates, task templates and a land queue. Leave the tmux/Dolt/skip-permissions machinery.

## A1 Agent model & identity
**What an agent is.** It is not one record. An agent is assembled from three things:
1. A **role definition**: an embedded TOML file per role with `role`, `scope` (town|rig), a `nudge` line, `prompt_template`, a `[session]` block (tmux name pattern, work_dir, `start_command`), `[env]` and `[health]` (`ping_timeout`, `consecutive_failures`, `kill_cooldown`, `stuck_threshold`) (`gastown/internal/config/roles/polecat.toml#L1-L23`, `gastown/internal/config/roles/witness.toml#L1-L23`). Seven roles ship: crew, deacon, dog, mayor, polecat, refinery, witness (`gastown/internal/config/roles/`).
2. A **prompt template** per role (`gastown/internal/templates/roles/*.md.tmpl`, eight including boot). `gt prime` renders it into the session at SessionStart and again at PreCompact (`gastown/internal/hooks/templates/claude/settings-autonomous.json#L107-L125`).
3. An **agent bead** in Beads with id pattern `<prefix>-<rig>-polecat-<name>`. Its columns `hook_bead` and `agent_state` drive the daemon (`gastown/internal/daemon/lifecycle.go#L1089-L1112`). Agent states: spawning, working, done, stuck, escalated, idle, running, nuked, awaiting-gate, patrolling, paused (`gastown/internal/beads/status.go#L15-L28`).

**The cast** (code-verified; glossary for wording):
- **Mayor**: one per town, the "chief-of-staff" coordinator the human talks to. Session `hq-mayor`, may run under ACP in an IDE (`gastown/internal/config/roles/mayor.toml#L1-L13`, `gastown/docs/glossary.md#L26-L27`).
- **Deacon**: town-level LLM patrol, watchdog of the rigs (`gastown/docs/glossary.md#L29-L30`). **Dogs** are its helper workers. **Boot** is the dog that triages the Deacon on every daemon tick (`gastown/internal/boot/boot.go#L1-L3`).
- **Witness**: per-rig LLM "Pit Boss" over polecats. It does no implementation (`gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L1`).
- **Refinery**: per-rig merge-queue agent (LLM session plus a Go `Engineer`) (`gastown/README.md#L94-L96`).
- **Polecat**: the worker, with "persistent identity but ephemeral sessions". It has a permanent agent bead, a "CV chain" and work history; its sandbox is a git worktree (`gastown/docs/glossary.md#L40-L41`). Names come from a pool (`gastown/internal/polecat/namepool.go`). Sandboxes are kept and reused after `gt done` (persistent polecat pool, `gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L1`).
- **Crew**: long-lived named agents, or the human's own workspace in a rig (`gastown/internal/config/roles/crew.toml#L1-L13`, `gastown/README.md#L60-L62`).
- **Overseer**: the human, as config `mayor/overseer.json` (`gastown/internal/config/overseer.go#L13-L19`).

**Harness binding.** There are 13 agent presets: claude, gemini, codex, kiro, cursor, auggie, amp, opencode, copilot, pi, omp, vibe, groq-compound (`gastown/internal/config/agents.go#L25-L54`). Each preset describes how to resume (`ResumeFlag`/`ResumeStyle`), whether `--fork-session` exists, its hooks provider and its ready-prompt prefix for tmux readiness (`gastown/internal/config/agents.go#L81-L144`). Every role's `start_command` is `exec claude --dangerously-skip-permissions` by default (`gastown/internal/config/roles/polecat.toml#L13`).

**Hiring and onboarding.** The cast is fixed by code. "Hiring" means `gt rig add` (a new project gets its own witness/refinery), `gt crew add`, and polecats auto-spawned by `gt sling <bead> <rig>` (`gastown/internal/cmd/sling.go#L53-L66`). Nobody writes a charter. Their own research notes that roles "are defined as Go templates" and that the declarative role format is the planned "Gas City" layer (`gastown/docs/research/w-gc-004-agent-framework-survey.md#L13`).

**Versus theuth.** theuth agents are user-hired files with a charter, model, tools, budget and `reports_to`. Gas Town has a hard-coded cast with operational health parameters per role. The `[health]` block, with ping timeout, consecutive failures, kill cooldown and stuck threshold *per role*, has no direct counterpart in theuth's agent file.

## A2 Organisation & authority
- **Hierarchy is topological, not an org chart.** Town (Mayor, Deacon, Dogs) over Rigs (each with Witness, Refinery, Polecats, Crew) (`gastown/docs/glossary.md#L16-L50`). Authority comes from *what each role's prompt and formula permits*, not from a `reports_to` field.
- **Supervision chain** (each level restarts the one below): daemon (Go) → Boot → Deacon → Witnesses/Refineries/Mayor (`gastown/internal/daemon/daemon.go#L894-L946`). Boot exists so that "the watchdog itself is still watching" (`gastown/docs/glossary.md#L35-L36`).
- **Escalation is severity-routed and tiered:** Agent → Deacon → Mayor → Overseer. "Each tier can resolve OR forward", and the chain is tracked in bead comments (`gastown/docs/design/escalation.md#L19-L33`). Routes per severity are configurable: CRITICAL = bead+mail+email+SMS, HIGH = bead+mail+email, MEDIUM = bead+mail mayor. There is a stale threshold (4 h) and `max_reescalations: 2` (`gastown/docs/design/escalation.md#L11-L17`, `gastown/docs/design/escalation.md#L41-L63`).
- **Workers are told to escalate, never wait.** "When blocked, you MUST escalate. Do NOT wait for human input". Escalate after more than 15 minutes stuck, then `gt done --status=ESCALATED` (`gastown/internal/templates/roles/polecat.md.tmpl#L480-L503`).
- **Vetoes.** While a Mayor ACP session is active, the witness defers automatic cleanup of zombie polecats so the Mayor can review (`gastown/internal/witness/handlers.go#L2128-L2142`). The E-stop exempts the Mayor "so it can coordinate recovery" (`gastown/internal/estop/estop.go#L1-L9`).
- **Permissions and visibility.** There are none between agents. Every agent can run every `gt`/`bd` command. Isolation is by git worktree ("ALL file edits must be within this directory", `gastown/internal/templates/roles/polecat.md.tmpl#L80`) and by `GT_ROLE` env, checked by guards such as "fail closed for gt done outside polecat worktree" (commit cc9aecb1, list_commits). Harness permissions are bypassed (`gastown/internal/hooks/templates/claude/settings-autonomous.json#L1-L10`).
- **Multi-workspace.** One Town per machine directory (`~/gt`), many rigs. Towns federate through **Wasteland** over DoltHub: post wanted items, claim work from other towns, earn portable reputation (`gastown/README.md#L115-L117`).
- **Humans.** The human is Overseer, the last escalation tier. Crew members are the human's workspaces. The Mayor is the human's single conversational entry point ("just tell the Mayor what you want", `gastown/README.md#L48-L50`).
- **Versus theuth.** theuth has explicit reporting lines, escalate-up-the-line, reviewers and approvals. Gas Town's chain is fixed by role and has no reviewer concept outside the merge gates. theuth is ahead on authority and permissions. Gas Town is ahead on severity routing, stale re-escalation and the watchdog-of-the-watchdog.

## A3 Work model
**The unit is a bead** (Beads issue). Fields include title, description, `design`, **`acceptance_criteria`**, notes, status, priority (0 = P0), type, assignee, owner, `defer_until`, `due_at`, `external_ref` and free JSON `metadata`. It also carries a persisted readiness projection `is_blocked` (`beads/internal/types/types.go#L20-L41`, `beads/internal/types/types.go#L101-L112`).
- **Statuses:** open, in_progress, blocked, deferred, closed, `pinned` (stays open indefinitely, used for hooks), `hooked` (actively claimed by a worker) (`beads/internal/types/types.go#L578-L584`).
- **Types:** bug, feature, task, epic, chore, decision, message, molecule, gate, spike, story, milestone, event (`beads/internal/types/types.go#L779-L797`). Mail, escalations and agents are beads too.
- **Typed dependency edges.** Blocking: `blocks`, `parent-child`, `conditional-blocks` ("B runs only if A fails"), `waits-for` ("fanout gate: wait for dynamic children"). Graph and audit: related, discovered-from, replies-to, relates-to, duplicates, supersedes, authored-by, assigned-to, approved-by, attests (skill attestation), `tracks` (convoy → issue, non-blocking), `until`, caused-by, validates, `delegated-from` ("completion cascades up") (`beads/internal/types/types.go#L1427-L1457`).
- **Ready work.** `bd ready` lists open issues with no active blockers, sorted by priority, hybrid or oldest (`beads/cmd/bd/ready.go#L22-L23`, `beads/cmd/bd/ready.go#L764`). `bd ready --claim` is one transaction: the ready predicate, a compare-and-set win and hydration of the won row. Its filter is "Reader.Ready's EXACT vocabulary" so a claim can never hand out work the listing did not show, and an empty front is "a NORMAL outcome, not an error" (`beads/issueops/readyclaimer.go#L5-L44`).
- **Claims and leases.** `bd update --claim` sets assignee + in_progress atomically (`beads/README.md#L73-L79`). Leases carry `lease_expires_at`, `heartbeat_at` and `lease_granted_node`; a lease is only enforceable on the replica that granted it (`beads/internal/types/types.go#L57-L73`). `bd reclaim` lets a reaper sweep stale leases back to ready, past a grace period. Each reverted row mints a new revision, so a stale holder's next write fails with a version mismatch (`beads/issueops/leasereclaimer.go#L74-L93`, `beads/CHANGELOG.md#L12-L26`). An opaque `RowVersion` token gives optimistic concurrency (`beads/internal/types/types.go#L75-L99`).
- **Gates.** `bd gate` creates async wait conditions that block workflow steps, with waiters registered on the gate bead (`beads/cmd/bd/gate.go#L26-L27`, `beads/cmd/bd/gate.go#L227-L228`).
- **Templates → work (MEOW).**
  - Beads side: a "proto" is a template epic. `pour` instantiates a persistent molecule, `wisp` an ephemeral one, `bond` combines, `squash` condenses to a digest, `burn` discards, and `distill` "extract[s a] proto from [an] ad-hoc epic" (`beads/cmd/bd/mol.go#L37-L58`).
  - Gas Town side: formulas are TOML of four types (convoy, workflow, expansion, aspect). Steps carry `needs`, `parallel` and an `acceptance` exit criterion. `pour = true` materialises steps as sub-wisps "with checkpoint recovery" (`gastown/internal/formula/types.go#L16-L36`, `gastown/internal/formula/types.go#L125-L129`). 47 formulas ship (plus one instructions file): patrols, `mol-polecat-work`, `code-review`, `rule-of-five`, `security-audit`, `mol-idea-to-plan`, and the release itself (`gastown/internal/formula/formulas/`, `gastown/RELEASING.md#L16-L20`).
- **Convoys** wrap related beads into a work order; `gt sling` creates one automatically even for a "swarm of one". The merge strategy (direct|mr|local) is stored on the convoy (`gastown/internal/cmd/sling.go#L39-L51`). `mountain` convoys auto-skip an issue after 3 failed polecats (`gastown/internal/witness/mountain.go#L1-L9`).
- **External trackers.** Beads has adapters for ado, github, gitlab, jira, linear and notion (`beads/internal/` dirs). Gas Town opens PRs through GitHub/Bitbucket providers (`gastown/internal/refinery/pr_provider_github.go`).
- **Versus theuth.** theuth tasks already have blockers (task ids, `decision:`, `person:`), a parent delegation tree, `done_when`, a reviewer, and a checkout with an `epoch` that makes a stale writer lose plus an orphan sweep (`theuth/crates/tasks/src/types.rs#L177-L195`, `theuth/crates/tasks/src/types.rs#L213-L219`). That matches Beads' claim plus RowVersion. What theuth lacks:
  1. typed edges (failure-path and fan-in);
  2. gates as first-class wait objects with waiters;
  3. a ready-front pull with atomic claim-next for pools;
  4. workflow templates that instantiate tracked task DAGs and can be distilled back from finished work;
  5. lease provenance per engine.

## A4 Execution & runtime
**Substrate.** Every agent is a CLI harness running in a **tmux session** named from its role pattern (`gastown/internal/config/roles/polecat.toml#L9-L13`). Polecats and crew work in git worktrees under `{town}/{rig}/polecats/{name}` (`gastown/internal/config/roles/polecat.toml#L11`). The Mayor alone can run under **ACP** inside an IDE: a `Propeller` drains queued nudges into the ACP session, and witnesses must use the ACP-aware `gt mayor status` (`gastown/internal/acp/propulsion.go#L130-L197`, `gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L34`). There is no cross-machine placement. A town is one machine directory, and Docker/devcontainer files exist for running the whole town in a container (`gastown/Dockerfile`, `gastown/docs/docker.md`).

**Wake model.**
- Normal wake is *event-driven*: "Normal wake is handled by feed subscription (bd activity --follow)". The daemon is "recovery-focused … the safety net" (`gastown/internal/daemon/daemon.go#L846-L852`).
- Patrol agents block in `gt mol step await-event`/`await-signal`, with persisted exponential backoff that survives context yields (HEAD commit a6aede3a). A witness waking the refinery must emit a file event "so the refinery's `await-event` unblocks instantly" (`gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L34`).
- Inside a session, work arrives by being **slung** onto the hook. `gt sling` attaches the bead and starts the agent; `gt hook` only attaches; `gt handoff` attaches and restarts with fresh context (`gastown/internal/cmd/sling.go#L94-L99`).
- **Spawn.** A polecat start gets a 60 s harness-start timeout. A startup nudge is verified after 25 s and retried at most twice (`gastown/internal/config/operational.go#L13-L23`). The session's SessionStart hook then runs `gt prime --hook`, which renders the role, the hooked bead and its formula steps (`gastown/internal/hooks/templates/claude/settings-autonomous.json#L107-L117`).

**The daemon heartbeat** (default every 3 min: `DefaultRecoveryHeartbeatInterval`, `gastown/internal/config/operational.go#L48`). Each tick runs in this order (`gastown/internal/daemon/daemon.go#L853-L1013`):
1. Skip everything if a shutdown lock or the **E-STOP** is present (`#L857-L869`).
2. Re-init the prefix registry and kill ghost sessions.
3. Ensure the Dolt server is up.
4. Ensure the Deacon, poke Boot, then do a direct Deacon heartbeat check as "belt-and-suspenders" (`#L894-L918`).
5. Ensure Witnesses, then Refineries; the refinery start is **pressure-gated** because "refineries consume API credits" (`#L920-L943`).
6. Ensure the Mayor; dogs are pressure-gated.
7. Process lifecycle requests.
8. Check **GUPP violations** and **orphaned work** (`#L967-L971`).
9. Check polecat session health, reap idle polecats, kill orphaned Claude sub-processes, prune stale branches.
10. **Dispatch queued work** through the scheduler, pressure-gated (`#L993-L1000`).
11. Rotate logs and persist `LastHeartbeat`/`HeartbeatCount` (`#L1006-L1013`).

Eleven more independent tickers run Dolt health, Dolt remotes, Dolt backup, JSONL git backup, the wisp reaper, doctor dog, compactor dog, checkpoint dog, scheduled maintenance, a main-branch test runner and the quota dog (`gastown/internal/daemon/daemon.go#L590-L716`).

**Liveness: "discover, don't track".**
- Running state comes from tmux, not from the stored `agent_state`: "Per gt-zecmc: derive running state from tmux" (`gastown/internal/daemon/lifecycle.go#L1133-L1139`). A removed stale-agent check is annotated "violated 'discover, don't track'" (`gastown/internal/daemon/daemon.go#L965`).
- **GUPP violation**: a polecat with a `hook_bead`, a live session and an agent bead not updated for more than 30 min. The daemon does not kill it; it **mails the rig's witness** a `GUPP_VIOLATION` with "Consider restarting if stuck" (`gastown/internal/daemon/lifecycle.go#L1138-L1170`, default `gastown/internal/config/operational.go#L19`).
- **Orphaned work**: a hooked bead whose session is dead. A TOCTOU re-check runs before any action (`gastown/internal/daemon/lifecycle.go#L1246-L1259`).
- The Deacon writes a **heartbeat file**. The response is two-tier: nudge at 5–20 min stale, but only if work is in flight so a sleeping await is not woken; kill and restart at ≥ 20 min; the crash-loop guard is respected (`gastown/internal/daemon/daemon.go#L1525-L1636`).
- Polecat heartbeat goes stale at 3 min (`gastown/internal/config/operational.go#L76`). Idle polecat sessions are reaped after 15 min (`gastown/internal/config/operational.go#L41`). A **mass-death** event fires on 3 deaths in 30 s (`gastown/internal/config/operational.go#L38-L39`, `gastown/internal/townlog/logger.go#L45`).
- **Restart tracker**: exponential backoff 30 s → 10 min (×2) with a 15-min crash-loop window. A separate fixed `PauseBackoff` (60 s) applies when the pane shows a Claude usage-limit signature, so quota pauses do not burn the crash budget (`gastown/internal/daemon/restart_tracker.go#L12-L52`, `gastown/internal/daemon/usage_limit.go#L5-L14`, `gastown/CHANGELOG.md#L37-L42`).

**Stall handling inside the Witness** (an LLM following `mol-witness-patrol`):
- Rule one: "You MUST run `gt patrol scan` … Do NOT improvise with `gt polecat list`, `gt peek`, or manual tmux checks". The Go scan does zombie detection, stall detection and completion discovery (`gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L40`).
- Policy is **restart-first**: dead process → restart; hung (no output 30+ min) → restart; done → leave alone; pending MR → leave alone. "Nuking only happens via explicit `gt polecat nuke` from a human or Mayor" (`gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L1`).
- In code, a zombie restart first checks whether the branch is **already merged**, squash merges included; if so it archives instead of restarting, to avoid duplicate MRs. Dirty worktrees get a cleanup wisp created *first* as an atomic interlock, then deduplicated, and the lowest wisp id wins (`gastown/internal/witness/handlers.go#L2104-L2163`).
- **Startup stalls** (pane silent > 60 s after spawn) get "blind key sequences that dismiss known blocking dialogs … without screen-scraping" (`gastown/internal/witness/handlers.go#L2245-L2251`). This broke when Claude Code pre-selected "No, exit" on its trust dialog (issue #4818, 2026-09-05, still open).

**Nudging (how agents are poked).**
- *Immediate* delivery types into the TUI. The steps (`gastown/internal/tmux/tmux.go#L1777-L1886`):
  1. a cross-process flock plus an in-process lock per session;
  2. find the agent pane;
  3. dismiss Claude Code's Rewind menu if it is open; exit copy mode;
  4. sanitise the text; `send-keys -l` in 512-byte chunks;
  5. an adaptive delay; Escape (skipped for Gemini and Copilot, where Escape cancels generation);
  6. a **600 ms** wait so readline does not read ESC+Enter as M-Enter;
  7. re-check Rewind;
  8. a *verified* submit, then SIGWINCH to wake detached panes.
- *Queued* delivery is cooperative. Nudges are JSON files in `.runtime/nudge_queue/<session>/`, "picked up by the agent's UserPromptSubmit hook at the next natural turn boundary" instead of cancelling in-flight tool calls. TTL is 30 min normal / 2 h urgent, depth is capped at 50, and stale claims are cleared after 5 min (`gastown/internal/nudge/queue.go#L1-L51`).

**Resume and context loss.**
- `gt prime --hook` runs on SessionStart **and** PreCompact. `gt mail check --inject` runs on every UserPromptSubmit for workers. `gt costs record` runs on Stop. PreToolUse guards (`gt tap guard dangerous-command|pr-workflow`) are installed per harness (`gastown/internal/hooks/templates/claude/settings-autonomous.json#L15-L146`). The same wiring exists as an **omp** extension (`gastown/internal/hooks/templates/omp/gastown-hook.ts#L7-L11`).
- `gt handoff` is "the canonical way to end any agent session". It respawns a fresh instance; `-c` collects state (hooked work, inbox, ready beads, in-progress) into the handoff mail; `--cycle` (from PreCompact hooks) forces a full respawn; "any molecule on the hook will be auto-continued" (`gastown/internal/cmd/handoff.go#L35-L65`).
- Harness session ids are resumable per preset (`ResumeFlag`, `--fork-session`; `gastown/internal/config/agents.go#L81-L105`). Account rotation resumes the same session under a new account (`ResumedSession`, `gastown/internal/quota/rotate.go#L10-L19`).

**Concurrency and capacity.**
- `scheduler.max_polecats`: -1 (default) dispatches directly; N > 0 defers, so `gt sling` writes a "sling context bead" and the daemon dispatches up to N (`gastown/docs/design/scheduler.md#L28-L38`). This is described as back-pressure against "exhausting API rate limits, memory, and CPU" (`gastown/docs/design/scheduler.md#L68-L70`).
- Batch sling has `--max-concurrent` (`gastown/internal/cmd/sling.go#L101-L107`).
- Pressure gates (load per core, free memory, max sessions) are **off by default** and never gate infrastructure agents (`gastown/internal/daemon/pressure.go#L31-L53`, `gastown/internal/config/operational.go#L53-L58`).

**Idempotency.** `gt sling` is idempotent for an unambiguous target that already holds the bead (`gastown/internal/cmd/sling_idempotency.go#L10-L16`). The scheduler skips closed or tombstoned beads (`gastown/CHANGELOG.md#L63-L65`).

**Versus theuth.** theuth runs agents as engine sessions and ACP children with an event-driven wake ledger, orphan sweep and mesh placement, so it is ahead on placement, protocol cleanliness (ACP, not keystrokes) and durability of the engine itself. Gas Town is ahead on three things:
1. a *named, layered* recovery loop with explicit thresholds per state;
2. the cooperative-vs-interrupting nudge split with TTLs;
3. rate-limit-aware crash accounting.

## A5 Coordination between agents
- **The ledger is the coordination medium.** Agents coordinate through beads, not chat. Completion is written to the agent bead (`exit_type`, `completion_time`) by `gt done` and *discovered* by the witness scan: "Beads over mail" (`gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L1`, `gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L40`).
- **Mail** is "messaging for agent communication via beads", with priorities (low…urgent) and types `task`, `escalation` and `scavenge`. Scavenge is "optional first-come-first-served work", a broadcast claim offer (`gastown/internal/mail/types.go#L1-L40`). Mail can be **hooked** as an assignment: `gt mail hook <id>` (`gastown/internal/templates/roles/polecat.md.tmpl#L279-L285`). Mail was found too heavy for routine signals. The Refinery stopped mailing `MERGE_FAILED` via the witness because that "created permanent Dolt commits for routine protocol signals" and now nudges the polecat directly (`gastown/internal/refinery/engineer.go#L1615-L1618`). Inbox reads were churning Dolt commits too (`gastown/CHANGELOG.md#L66-L67`).
- **Nudges** are ephemeral (see A4). Delivery is serialised per session with a flock to stop "concurrent nudges interleave send-keys/Enter" (`gastown/internal/tmux/tmux.go#L1778-L1790`).
- **Who decides who works next:**
  - the Mayor and humans sling;
  - the scheduler dispatches deferred work under capacity;
  - the Deacon re-dispatches recovered beads at most 3 times with a 5-min cooldown (`gastown/internal/deacon/redispatch.go#L17-L27`);
  - the witness blocks re-dispatch past `MaxBeadRespawns` and escalates to the Mayor: "the primary circuit breaker for spawn storms (clown show #22)" (`gastown/internal/witness/spawn_count.go#L66-L93`);
  - Beads offers pull-style `bd ready --claim` for anyone (`beads/issueops/readyclaimer.go#L46-L56`).
- **Locks and checkout.** Beads claims are compare-and-set with an opaque `RowVersion` (`beads/internal/types/types.go#L75-L99`) plus leases (A3). Gas Town adds file flocks (respawn state, nudge queue) and a **merge slot**: the main push lock held by one refinery or by its conflict-resolution path (`gastown/internal/refinery/engineer.go#L1030-L1075`).
- **Merge queue (Refinery).**
  - **Batch-then-bisect**: up to 5 MRs, wait 30 s to fill, "retry the full batch once before bisecting when tests fail. This avoids blaming an innocent MR for a flaky test" (`gastown/internal/refinery/batch.go#L10-L35`).
  - The bisect is recursive, carrying a known-good prefix (`gastown/internal/refinery/batch.go#L485-L598`).
  - MRs are scored and ordered, and an MR blocked by an MR outside the batch is excluded (`gastown/internal/refinery/batch.go#L55-L58`, `gastown/internal/refinery/score.go`).
  - Gates run in phases (`gastown/internal/refinery/engineer.go#L1206-L1213`).
  - **Conflicts**: `on_conflict` is `assign_back` (default) or `auto_rebase` (`gastown/internal/refinery/engineer.go#L107-L108`, `gastown/internal/refinery/engineer.go#L186`). A conflict creates a resolution task and blocks the MR. Slot timeouts silently retry. Policy-ineligible MRs are closed. PR mode waits for a human approval. A missing branch is escalated to the Mayor as possible work loss (`gastown/internal/refinery/engineer.go#L1562-L1613`). `mol-polecat-conflict-resolve` is the formula a polecat runs for it (`gastown/internal/formula/formulas/`).
- **Topology patterns.**
  - **Convoys** are fan-out work orders.
  - Formula types `convoy` ("parallel legs and synthesis") and `aspect` ("multi-aspect parallel analysis") (`gastown/internal/formula/types.go#L16-L25`).
  - `rule-of-five` and `shiny*` formulas give multi-pass review patterns.
  - **Seance**: talk to a predecessor by `claude --fork-session --resume <id>` (`gastown/internal/cmd/seance.go#L37-L62`).
- **Consensus and voting.** None. A grep for `vote|consensus|quorum` over `internal/` Go, TOML and templates finds nothing; convoy formulas use synthesis legs instead.
- **Versus theuth.** theuth has typed `send_message` with receipts, task threads and checkout. Gas Town adds:
  1. the merge queue;
  2. respawn and redispatch caps;
  3. "scavenge" broadcast offers;
  4. the explicit lesson that durable mail is too heavy for protocol pings.
  The lesson applies to theuth's own task-thread notes if desks ever chatter.

## A6 Human in the loop & governance
- **No permission model inside agents.** Role starts are `exec claude --dangerously-skip-permissions` (`gastown/internal/config/roles/polecat.toml#L13`). The installed Claude settings set `skipDangerousModePermissionPrompt` and `defaultMode: bypassPermissions` (`gastown/internal/hooks/templates/claude/settings-autonomous.json#L1-L10`). Governance is therefore *preventive guards*, not approvals:
  - PreToolUse hooks call `gt tap guard dangerous-command`, which blocks `rm -rf /`, `git push --force` (lease allowed), `git reset --hard`, `git clean`, sudo and package installs (`gastown/internal/cmd/tap_guard_dangerous.go#L13-L27`).
  - `gt tap guard pr-workflow` steers PR creation and branch creation (`gastown/internal/hooks/templates/claude/settings-autonomous.json#L15-L40`).
  - A context-budget guard script prevents context overflow (`gastown/internal/cmd/tap_guard.go#L28`).
- **Kill switch: E-STOP.** It is a sentinel file `ESTOP` at the town root. Agents are frozen with SIGTSTP, the daemon stops restarting them, and the Mayor is exempt (`gastown/internal/estop/estop.go#L1-L9`, `gastown/internal/daemon/daemon.go#L862-L869`). There is a per-rig variant too (`gastown/internal/cmd/estop.go#L168`). A `TriggerAuto` provenance constant exists (`gastown/internal/estop/estop.go#L26-L27`), but only manual activations were found (`gastown/internal/cmd/estop.go#L132`, `gastown/internal/cmd/estop.go#L168`).
- **Human gates.**
  - The merge strategy `pr` makes the Refinery wait for "a human review on GitHub" before landing (`gastown/internal/refinery/engineer.go#L1591-L1597`).
  - `gt done --status ESCALATED` marks "Hit blocker, needs human intervention" (`gastown/internal/cmd/done.go#L35-L47`).
  - Escalations reach the human via email, SMS or a Slack webhook, depending on severity (`gastown/docs/design/escalation.md#L41-L75`).
  - The daemon has a Slack notifier (`gastown/internal/daemon/daemon.go#L1705`).
  - Convoy completion fires notifications "without polling" (`gastown/CHANGELOG.md#L57-L59`).
- **Audit.** Every Beads mutation is a Dolt commit ("auto-commit per mutation", `beads/cmd/bd/compact.go#L60-L62`), so the ledger is versioned and diffable. Issues record `closed_by_session` (`beads/internal/types/types.go#L55`). Town events go to `.events.jsonl` (`gastown/internal/cmd/seance.go#L61`). The escalation chain is kept as bead comments (`gastown/docs/design/escalation.md#L33`).
- **Inbox and attention.** The human's inbox is mail to the Mayor and Overseer plus the dashboard and feed (A9). No approval queue exists, since there is nothing to approve.
- **Versus theuth.** theuth has `tools.policy` approvals, the double, ask cards, an attention inbox and mobile channels, and is clearly ahead on governance. Two things are worth borrowing: a **town-wide freeze switch that exempts the coordinator**, and **PreToolUse-style deny guards** expressed as policy (theuth's `tools.policy` already covers the guards).

## A7 Cost, budgets & runaway control
- **No money budgets.** A grep for budget/max_cost/spend limits over `internal/` finds only cost *tiers*, a disk budget for logs and time budgets. The CLI words "budget" only as a tier name (`gastown/internal/cmd/config.go#L112`).
- **Cost tiers = model routing per role.** standard (Opus everywhere), economy (Sonnet/Haiku for patrols, Opus for workers), budget (Haiku/Sonnet patrols, Sonnet workers), and two Groq presets that route patrol and utility roles to Groq Compound (`gastown/internal/config/cost_tier.go#L12-L29`).
- **Evidence for downgrades.** `gt-model-eval` is a promptfoo suite of 94 cases across deacon, witness, refinery and dog decisions. It has "Class A" neutral-evidence tests with no answer hints, built because patrol agents "burn through Opus budget" (`gastown/gt-model-eval/README.md#L3-L7`, `gastown/gt-model-eval/README.md#L47-L58`).
- **Cost accounting is after the fact.**
  1. A Stop hook runs `gt costs record`, which reads the Claude transcript and applies a pricing table "as of Jan 2025" (`gastown/internal/cmd/costs.go#L72-L79`, `gastown/internal/cmd/costs.go#L198`).
  2. It appends to `~/.gt/costs.jsonl`.
  3. A digest step folds a day into a "permanent daily digest" bead (`gastown/internal/cmd/costs.go#L93-L97`).
  4. Totals are split by role and rig (`gastown/internal/cmd/costs.go#L342-L345`).
- **Quota and rate limits.** The quota dog (default every 5 min, `gastown/internal/daemon/quota_dog.go#L16-L25`) scans sessions for hard or near rate limits and **rotates Claude accounts**. It swaps the keychain once per config dir and *resumes the same session* under the new account (`gastown/internal/quota/rotate.go#L10-L39`). Usage-limit signatures on the pane or stderr turn a "crash" into a fixed 60 s pause outside the crash budget (`gastown/internal/daemon/usage_limit.go#L5-L14`, `gastown/internal/daemon/restart_tracker.go#L34-L52`).
- **Runaway defences.**
  - Spawn-storm circuit breaker: a per-bead respawn count, then escalate to the Mayor (`gastown/internal/witness/spawn_count.go#L66-L93`).
  - Deacon redispatch: at most 3, with a 5-min cooldown (`gastown/internal/deacon/redispatch.go#L17-L27`).
  - Mass-death detection (`gastown/internal/config/operational.go#L38-L39`).
  - Restart backoff and crash-loop window (`gastown/internal/daemon/restart_tracker.go#L12-L52`).
  - Scheduler capacity (`gastown/docs/design/scheduler.md#L28-L38`).
  - Opt-in load and memory gates (`gastown/internal/daemon/pressure.go#L31-L53`).
  - `mountain` convoys skip an issue after 3 failures (`gastown/internal/witness/mountain.go#L1-L9`).
  - Workers are told every session must end in `gt done` or `bd close` precisely because a missing completion "re-dispatches it to a new polecat, causing spawn storms" (`gastown/internal/templates/roles/polecat.md.tmpl#L70-L71`).
- **Loop and no-progress.** GUPP-violation (30 min) and stalled (15 min) states (`gastown/internal/tui/feed/stuck.go#L32-L36`). The patrol idle counter `idle:N` backs off patrol cycles when the rig is quiet (`gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L96-L106`).
- **Versus theuth.** theuth has budgets with auto-pause and a spend ledger, and is ahead on money. Gas Town is ahead on *operational* runaway control:
  1. respawn and redispatch caps per work item;
  2. mass-death detection;
  3. telling a rate-limit pause from a crash;
  4. account rotation with session resume;
  5. evidence-based role downgrades.

## A8 Memory, knowledge & learning
- **The ledger is the memory.** Beads markets itself as "a memory upgrade for your coding agent". It "replaces messy markdown plans with a dependency-aware graph, allowing agents to handle long-horizon tasks without losing context" (`beads/README.md#L15`). Work state survives restarts because it lives on the hook and the bead, not in the agent (`gastown/README.md#L11-L16`).
- **Injected context.** `bd prime` emits workflow context for SessionStart hooks: about 50 tokens in MCP mode, 1–2k in CLI mode. It is "designed … to prevent agents from forgetting bd workflow after context compaction". It can be overridden by `PRIME.md` at three levels (`beads/cmd/bd/prime.go#L91-L126`). `gt prime` does the same for the role, hooked work and formula steps (A4).
- **Project memories.** `bd remember "insight"` stores key/value memories that "persist across sessions and account rotations" and are injected at prime time. An opt-in corpus **budget in chars** warns at 80% and refuses writes beyond it unless `--force` (`beads/cmd/bd/memory.go#L344-L363`). The README draws the line: project facts go in bd, "per-operator preferences in your harness's own memory" (`beads/README.md#L56`).
- **Decay.** `bd compact` does what the README calls semantic "memory decay" (`beads/README.md#L65`). The command itself calls it "permanent graceful decay": closed issues older than 30 days get summarised (about 70% reduction, original discarded), either agent-driven (`--analyze`/`--apply`) or AI-driven (`beads/cmd/bd/compact.go#L45-L58`). KRC gives TTL auto-pruning of "Level 0" operational data such as heartbeats and session events (`gastown/internal/krc/krc.go#L1-L10`). Wisps are burned after runs (`gastown/docs/glossary.md#L66-L67`).
- **Talk to the past.** `gt seance --talk <id> -p "Where is X?"` forks and resumes a predecessor Claude session to *ask* it instead of parsing logs. Sessions are discovered from `.events.jsonl` and a `[GAS TOWN]` beacon (`gastown/internal/cmd/seance.go#L37-L62`).
- **Learning from outcomes.** Polecats have a "CV chain" and work history (`gastown/docs/glossary.md#L40-L41`). The `attests` edge is "skill attestation" (`beads/internal/types/types.go#L1446`). A capability-based dispatch design proposes tasks declaring requirements and executors declaring capability profiles grounded in stamps (`gastown/docs/gas-city/crew-specialization-design.md#L1-L25`); it is design, not code here. Formulas can be *distilled* from finished ad-hoc epics (`beads/cmd/bd/mol.go#L56`).
- **Evals.** gt-model-eval (A7) is the only eval harness found.
- **Versus theuth.** theuth has a knowledge base, nightly dreaming, skills and endless memory, and is ahead on general knowledge. Gas Town and Beads add:
  1. a *char budget* on the injected memory corpus;
  2. forking a predecessor session to ask it ("seance");
  3. distil-a-template-from-finished-work;
  4. a hard split between project memory (shared, in the ledger) and operator memory (in the harness).

## A9 Observability & UX
- **CLI-first.** Everything is a `gt`/`bd` command.
- **Feed.** A real-time TUI `gt feed` of gt events (`gastown/internal/cmd/feed.go#L48`) classifies agents as GUPP-violation (> 30 min no progress with hooked work, CRITICAL), Stalled (> 15 min), Working, Idle or Zombie (`gastown/internal/tui/feed/stuck.go#L32-L36`).
- **Convoys and dashboard.** There is a convoy TUI (`gastown/internal/tui/convoy/`) and "the convoy tracking web dashboard" `gt dashboard` (`gastown/internal/cmd/dashboard.go#L30`, `gastown/internal/web/`). Work ordering for visibility: sling auto-creates convoys "for dashboard visibility" (`gastown/internal/cmd/sling.go#L37-L42`).
- **Health.** `gt doctor` has 72 check files covering beads routing, Dolt ports, stale binaries, zombie and stalled polecats, tmux socket, priming, worktree gitdir and more, many `(fixable)` with `--fix` (`gastown/internal/cmd/doctor.go#L25-L42`, `gastown/internal/doctor/`). A daemon **doctor dog** runs periodically and pours a `mol-dog-doctor` molecule when warnings appear (`gastown/internal/daemon/daemon.go#L1063-L1081`).
- **Telemetry.** OTel architecture and data model docs (`gastown/docs/design/otel/otel-architecture.md`, `gastown/docs/otel-data-model.md`) and `internal/telemetry`.
- **tmux UX.** Window tint per role and keybindings (`gastown/internal/session/window_tint.go`, `gastown/docs/design/tmux-keybindings.md`).
- **Onboarding.** `gt install` with shell integration, then "just tell the Mayor what you want" (`gastown/README.md#L48-L50`). The shell hook used to nag in every git repo and loop on Ctrl-C until 1.2.1 made the offer opt-in (`gastown/CHANGELOG.md#L14-L23`).
- **No org chart or mobile.** Remote access is tmux attach, plus Slack/email/SMS for escalations.
- **Versus theuth.** theuth has a desktop app, web, TUI, HUD, tasks board and Slack/Signal/Telegram, and is far ahead on UX. Two things are worth borrowing:
  1. the **stuck-state taxonomy** shown per agent, where *working* means producing output, distinct from *alive*;
  2. a `doctor` with named, individually fixable checks plus a periodic doctor pass.

## A10 Extensibility & integration
- **Harnesses.** 13 presets (`gastown/internal/config/agents.go#L25-L54`) and hook installers for 9 runtimes: claude, codex, copilot, cursor, gemini, omp, opencode, pi, vibe (`gastown/internal/hooks/templates/`).
- **Workflows and plugins.** Formulas plus overlays: formula directories and overlays, and the `gt formula overlay` commands (`gastown/internal/formula/overlay.go`, `gastown/docs/design/directives-and-overlays.md`). **Plugins** are dog-run maintenance jobs such as compactor-dog, github-sheriff, quality-review, stuck-agent-dog and tool-updater, with plugin runs recorded as beads (`gastown/plugins/`, `gastown/internal/plugin/`).
- **Beads surfaces.**
  - an MCP server (`beads/integrations/beads-mcp`, Python, published to PyPI);
  - a Claude Code plugin;
  - an HTTP API through `bd serve`: `/v0/beads/issues…` with capability flags and a `wire_revision` (`beads/CHANGELOG.md#L12-L26`, `beads/internal/httpapi/`);
  - a public HTTP backend with `bd connect` (beads commit 8dbbe78e, today);
  - tracker adapters for ADO, GitHub, GitLab, Jira, Linear and Notion (`beads/internal/`).
- **Other surfaces.** ACP proxy for the Mayor (`gastown/internal/acp/`). PR providers for GitHub and Bitbucket (`gastown/internal/refinery/pr_provider_*.go`). Slack webhook and SMTP/SMS escalation (`gastown/docs/design/escalation.md#L50-L60`). **Wasteland** federation via DoltHub (`gastown/README.md#L115-L117`, separate repo `gastownhall/wasteland`, 91★).
- **Not found.** No A2A; no API for other harnesses to drive Gas Town other than the CLI and Beads' HTTP/MCP.
- **Versus theuth.** theuth integrates harnesses through ACP and refuses per-harness plugins (Sky, 2026-10-09). Gas Town is the opposite model: per-harness hook installers and tmux. Beads' MCP and HTTP surface is the interesting part. A theuth desk could *use* Beads as a project ledger through MCP without theuth adopting it.

## A11 Engineering, licence & traction
- **Stack and size.**
  - Gas Town: Go, about 249k non-test lines + 226k test lines, 568 test files, 11 CI workflows including e2e, nightly integration and Windows CI (counted with `find|wc` over the clone; `.github/workflows/`).
  - Beads: Go plus Bazel, about 426k non-test + 646k test lines, 2,129 test files, 21 workflows.
  - Both are MIT. Distribution: Homebrew core, a tap and npm `@gastown/gt` (`gastown/RELEASING.md#L5-L10`).
- **Traction (GitHub MCP, 2026-10-10).**
  - gastown: 18,319★, 1,686 forks, 507 open issues; created 2025-12-16.
  - beads: 27,778★, 1,879 forks, 1,421 open issues; created 2025-10-12.
  - gascity: 1,357★, 442 forks, 1,513 open issues; created 2026-02-22.
  - gascity-packs 95★; wasteland 91★.
- **Activity (the brief's question).**
  - Gas Town main's newest commit is 649b832, 2026-07-23. The last tag is v1.2.1, 2026-06-06, after v1.0.0 2026-04-03, v1.1.0 2026-05-07 and v1.2.0 2026-05-30. `CHANGELOG.md` `[Unreleased]` is empty (`gastown/CHANGELOG.md#L8-L10`).
  - The repo's `pushed_at` of 2026-10-08 comes from branch pushes: dependabot PR #4919 opened that day; 30+ `integration/*` and `fix/*` branches exist. It does not come from main.
  - PRs closed in October (#4913, #4914, #4796) were closed **unmerged**, while community issues keep arriving (#4917 on 2026-10-06).
  - Meanwhile Beads tags rc builds weekly (v1.3.0 2026-09-15 → v1.3.2-rc.2 2026-10-10) with commits today, and Gas City has commits today.
  - Their own research doc calls Gas City "the planned declarative layer on top of Gas Town" (`gastown/docs/research/w-gc-004-agent-framework-survey.md#L13`).
  - **Reading:** Gas Town is effectively frozen and serves as the reference implementation, while the team builds Gas City and Beads. Expect fixes to land in Gas City, not here. This is inferred from GitHub data; no deprecation notice was found in `README.md`.
- **Process.** Their own agents do much of the work. Merged PRs come from `polecat/<name>/…` branches, for example PR #4568 from `Bella-Giraffety/polecat/mutant/gt-pr-sheriff-4547+current-main` (list_commits). Releases are themselves a formula: `gt mol wisp create gastown-release` (`gastown/RELEASING.md#L16-L20`). PR-sheriff evidence folders are committed to the repo (`gastown/pr-sheriff-evidence/`). The npm scope was squatted by a community member, with transfer still "pending" (`gastown/RELEASING.md#L135-L140`).
- **Business model.** None visible in these repos. `gascityinc-randy[bot]` commits on gascity hint at a company (GitHub list_commits).
- **Maturity signals.**
  - The code is full of incident tags: "clown show #22" (`gastown/internal/witness/spawn_count.go#L70`), PATCH-005, gt-xxxx ids.
  - Every daemon step carries a scar comment, for example "This prevents the daemon from fighting shutdown by auto-restarting killed agents" (`gastown/internal/daemon/daemon.go#L854-L860`).
  - A silent data split from duplicate Dolt databases was fixed in 1.2.0 (`gastown/CHANGELOG.md#L43-L48`).

## Map to the three questions

### Q1 What do they offer versus theuth?
| Axis | Gas Town + Beads | theuth (per AXES "theuth today"; `ours` report is the authority) | Gap / who is ahead |
|---|---|---|---|
| A1 Agent model | Fixed cast of 7 roles from TOML + Go templates; persistent polecat identity, ephemeral sessions; `[health]` thresholds per role; 13 harness presets | User-hired agent files with charter, model, tools, budget, `reports_to`, paused; desks | theuth ahead on definition and hiring; GT has per-role health parameters |
| A2 Authority | Topological chain (daemon→Boot→Deacon→Witness→polecats); severity escalation with stale re-escalation; Overseer = human | Org chart, reporting lines, reviewer, escalate up the line, approvals | theuth ahead; borrow severity + stale re-escalation and watchdog-of-watchdog |
| A3 Work | Beads: statuses incl. hooked/pinned, acceptance criteria, typed edges (conditional, waits-for, delegated-from), gates, ready front + atomic claim-next, leases with TTL/heartbeat/node, RowVersion, formulas→molecules/wisps, convoys | Tasks board: states, blockers (task/decision/person), parent tree, `done_when`, reviewer, checkout with epoch + orphan sweep | Parity on claim safety; GT ahead on edge types, gates, pull claim, templates |
| A4 Runtime | tmux + worktrees, daemon 3-min recovery tick + 11 dog tickers, event wake via `bd activity --follow`/await-event, GUPP/orphan detection, restart tracker, rate-limit pause, account rotation | Engine sessions + ACP children, wake ledger, orphan sweep, mesh placement, sandboxes, restart durability (109/146) | theuth ahead on substrate; GT ahead on a named recovery matrix and rate-limit-aware restarts |
| A5 Coordination | Ledger-first (beads), beads mail incl. scavenge offers, queued vs immediate nudges, respawn/redispatch caps, merge slot, bisecting merge queue, seance | `send_message` (receipts), task threads, checkout, A2A | GT ahead on merge queue and caps; theuth ahead on typed messaging |
| A6 HITL | No approvals (skip-permissions), PreToolUse deny guards, E-STOP, PR-mode human review, email/SMS/Slack escalations | `tools.policy`, double, ask cards, attention inbox, mobile channels | theuth far ahead; borrow the org-wide freeze |
| A7 Cost | Model tiers per role, post-hoc transcript costing, quota rotation, spawn-storm breaker, mass-death, scheduler cap, model-eval suite | Budgets with auto-pause, spend ledger | theuth ahead on money; GT ahead on operational runaway control |
| A8 Memory | Ledger as memory, `bd remember` + prime injection with char budget, semantic compaction, KRC TTL, seance, distil | Knowledge base, dreaming, skills, endless memory | theuth ahead; borrow char budget, seance, distil |
| A9 UX | CLI + tmux, feed TUI with stuck taxonomy, convoy TUI + web dashboard, 72-check doctor, OTel | Desktop/web/TUI/HUD, board, channels, activity monitor | theuth far ahead; borrow stuck taxonomy + doctor |
| A10 Extensibility | Per-harness hook installers (9), formulas/overlays, dog plugins, beads MCP + HTTP API, trackers, PR providers, Wasteland federation | ACP, MCP, A2A, channels, skills | Different bets; Beads' MCP is usable *by* theuth as a tool |
| A11 Engineering | ~249k + ~426k Go non-test LOC, huge test suites, MIT, 18.3k/27.8k★; GT main frozen since 2026-07-23, energy moved to Gas City + Beads | Rust, one engine per machine, mesh | GT's churn is a warning about complexity |

### Q2 What can we learn / borrow? (techniques only)
Each entry gives the mechanism, the cite, and the cost of being wrong.

1. **A durable work pointer re-injected at every session start and compaction (hook + `gt prime` on SessionStart/PreCompact).**
   - Mechanism: one field per agent names "the work I am on". It is rendered into context on every fresh session *and* after compaction, so context loss never loses the assignment (`gastown/internal/hooks/templates/claude/settings-autonomous.json#L107-L125`, `gastown/docs/glossary.md#L69-L70`).
   - In theuth: the desk manual is injected at each desk episode. The gap is re-injecting *the current checkout* after an endless/compaction fold.
   - Cost of being wrong: tiny (a few hundred tokens per fold).
2. **The GUPP rule plus a "hooked but not progressing" detector distinct from "dead".**
   - Mechanism: classify an agent by three questions: has work? session alive? progress within T? The answers map to fine, *violation* (notify the supervisor, nudge first) or *orphan* (restart or reassign) (`gastown/internal/daemon/lifecycle.go#L1089-L1153`, `gastown/internal/daemon/lifecycle.go#L1185-L1259`, `gastown/internal/tui/feed/stuck.go#L32-L36`).
   - Cost of being wrong: false positives on long builds. GT's answer is to mail the witness, not kill, and to skip the nudge when nothing is in flight (`gastown/internal/daemon/daemon.go#L1615-L1630`).
3. **Restart-first recovery with interlocks.**
   - Mechanism: never auto-delete a worker's sandbox. Before restarting, check "already merged (incl. squash)" and archive instead. For dirty state, create the cleanup record first and dedupe with a deterministic winner. Defer automatic cleanup while the coordinator is reviewing (`gastown/internal/witness/handlers.go#L2104-L2163`, `gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L1`).
   - Cost of being wrong: medium. A wrong restart duplicates work or MRs; a wrong archive loses work. The merged-check is the guard.
4. **Per-work-item respawn and redispatch caps plus mass-death detection.**
   - Mechanism: a persisted counter per bead, max N, after which escalate to the coordinator instead of redispatching. Redispatch is limited to 3 per 5-minute cooldown. 3 deaths in 30 s is an event (`gastown/internal/witness/spawn_count.go#L66-L93`, `gastown/internal/deacon/redispatch.go#L17-L27`, `gastown/internal/config/operational.go#L38-L39`).
   - Cost of being wrong: low. A cap that is too low only escalates earlier.
5. **Tell a rate-limit pause from a crash.**
   - Mechanism: classify the exit output or pane tail against provider limit signatures. Limits get a fixed pause outside the crash-loop budget; crashes get exponential backoff (`gastown/internal/daemon/usage_limit.go#L5-L14`, `gastown/internal/daemon/restart_tracker.go#L12-L52`). Optionally rotate the account and resume the same session (`gastown/internal/quota/rotate.go#L10-L39`).
   - Cost of being wrong: low. GT notes that a misclassified crash "will still recover via the fixed paused-retry delay".
6. **Cooperative versus interrupting delivery, with TTLs.**
   - Mechanism: queued messages are drained at the next turn boundary, with priority, TTL (30 min / 2 h), max depth 50 and stale-claim cleanup. Interrupting delivery is reserved for urgent cases (`gastown/internal/nudge/queue.go#L1-L51`).
   - Cost of being wrong: a dropped expired message. Make expiry visible to the sender.
7. **Ready front plus atomic claim-next (pull) over the same predicate as the listing.**
   - Mechanism: the caller names a question (filter) and the store picks, CAS-wins and hydrates in one transaction, walking past rows a racing agent took. An empty result is a normal outcome (`beads/issueops/readyclaimer.go#L5-L56`).
   - Cost of being wrong: medium. Pull changes who decides who works next, against theuth's owner-assigned model (design 67), so use it only for explicit pools.
8. **Leases with TTL, heartbeat and granting-node provenance, plus a reaper reclaim that mints a new revision.**
   - Mechanism: `lease_expires_at` + `heartbeat_at` + `lease_granted_node`. A reclaim refuses foreign-node leases unless overridden, and stale holders' writes fail on the revision (`beads/internal/types/types.go#L57-L73`, `beads/issueops/leasereclaimer.go#L74-L130`).
   - theuth's Checkout already has `engine`, `epoch` and `progress_at_ms` (`theuth/crates/tasks/src/types.rs#L177-L195`). The borrow is the *explicit TTL and grace* and the foreign-engine refusal for the mesh.
   - Cost of being wrong: low.
9. **Typed dependency edges and gates.**
   - Mechanism: `conditional-blocks` (B only if A fails), `waits-for` (fan-in on dynamic children), `delegated-from` (completion cascades up), and gate beads with registered waiters (`beads/internal/types/types.go#L1427-L1457`, `beads/cmd/bd/gate.go#L26-L27`).
   - Cost of being wrong: medium. Semantics multiply; UI and engine must agree on readiness. Beads persists an `is_blocked` projection for exactly that reason (`beads/internal/types/types.go#L37-L41`).
10. **Workflow templates that instantiate tracked task DAGs (formula → molecule/wisp), and distil-from-finished-work.**
    - Mechanism: TOML steps with `needs`, `parallel` and `acceptance` per step, plus vars. Pour (durable, checkpointed) versus wisp (ephemeral, burned) (`gastown/internal/formula/types.go#L16-L36`, `gastown/internal/formula/types.go#L125-L129`, `beads/cmd/bd/mol.go#L37-L58`).
    - Cost of being wrong: medium. Template sprawl; GT ships 47 formulas, several of them toys (towers-of-hanoi).
11. **Patrols as an LLM over a deterministic scan, plus a watchdog of the watchdog.**
    - Mechanism: the patrol formula forces `gt patrol scan` (Go: precise detection) and leaves only follow-ups to judgment. Boot ("runs fresh on each daemon tick") decides whether to wake, nudge or interrupt the Deacon (`gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L40`, `gastown/internal/boot/boot.go#L1-L3`).
    - Cost of being wrong: low. It is a routine and skill pattern, not engine code.
12. **A bisecting land queue for parallel branches.**
    - Mechanism: batch ≤ 5, retry once on red to rule out flakes, bisect with a known-good prefix, send conflicts back as new worker tasks, serialise pushes with a slot (`gastown/internal/refinery/batch.go#L10-L35`, `gastown/internal/refinery/batch.go#L485-L598`, `gastown/internal/refinery/engineer.go#L1562-L1624`).
    - Cost of being wrong: high effort; a wrong culprit verdict bounces a good branch. Retry-once mitigates.
13. **Severity-routed escalation with stale re-escalation.**
    - Mechanism: per-severity channels; an unacknowledged escalation re-escalates after 4 h, at most twice (`gastown/docs/design/escalation.md#L11-L17`, `gastown/docs/design/escalation.md#L41-L63`).
    - Cost of being wrong: notification fatigue. Cap the number of re-escalations.
14. **An org-wide freeze switch that exempts the coordinator.**
    - Mechanism: a single sentinel freezes every agent. The supervisor loop skips restarts while it is set. The coordinator stays up to steer recovery. It records manual or auto provenance (`gastown/internal/estop/estop.go#L1-L27`, `gastown/internal/daemon/daemon.go#L862-L869`).
    - Cost of being wrong: low.
15. **A char budget on injected memory, and project memory kept separate from operator memory.**
    - Mechanism: a ceiling on the corpus with a warning at 80% (`beads/cmd/bd/memory.go#L357-L363`, `beads/README.md#L56`).
    - Cost of being wrong: low.
16. **"Seance": ask the predecessor instead of reading its log.**
    - Mechanism: fork-resume a stored session read-only and put one question to it (`gastown/internal/cmd/seance.go#L37-L62`).
    - Cost of being wrong: token cost of a resumed context, and a fork that drifts from the record.
17. **Evidence before downgrading a role's model.**
    - Mechanism: fixed decision fixtures per role, with neutral-evidence "Class A" cases, compared across models (`gastown/gt-model-eval/README.md#L3-L7`, `gastown/gt-model-eval/README.md#L47-L58`).
    - Cost of being wrong: low.
18. **A named, fixable `doctor` plus a periodic doctor pass that files a record when checks warn** (`gastown/internal/cmd/doctor.go#L25-L42`, `gastown/internal/daemon/daemon.go#L1063-L1081`).
    - Cost of being wrong: check rot. GT's 72 checks include one whose `--fix` can never succeed (#4880).

**Do not borrow.**
- **Keystroke delivery into TUIs** (`gastown/internal/tmux/tmux.go#L1805-L1886`) and **blind dialog dismissal** (`gastown/internal/witness/handlers.go#L2245-L2251`; #4818). Both are contrary to Sky's ACP-first ruling.
- **Per-harness hook installers** for 9 runtimes (`gastown/internal/hooks/templates/`), which Sky ruled out on 2026-10-09.
- **`--dangerously-skip-permissions` by default.**
- **Durable mail for protocol pings** (`gastown/internal/refinery/engineer.go#L1615-L1618`).
- **Inactivity-based auto-close** (#4911, #4630, #4885).
- **Dolt as the store.** Doctor needs to check phantom, orphan and duplicate databases (`gastown/CHANGELOG.md#L43-L48`).

### Q3 How should theuth improve? (ranked by value ÷ effort)
1. **Respawn and redispatch cap per task + mass-death auto-pause.** Count wake re-deliveries and child respawns per task and per desk. After N (3), stop and escalate to the owner's manager with the history. When K sessions die within 30 s, raise one attention item and pause dispatch. *Lands:* `crates/tasks` ledger wake release/backoff (`theuth/crates/tasks/src/ledger.rs#L2064`), roster desk dispatch, attention inbox (104/132). Value high, effort low.
2. **Stuck-state taxonomy on every agent and desk row: working / stalled / hooked-not-progressing / orphan / idle.** Feed it from the checkout's `progress_at_ms` plus session liveness, and nudge before escalating. *Lands:* activity monitor (155), the HUD/board rows, the orphan sweep in `crates/tasks` (`theuth/crates/tasks/src/types.rs#L184-L190`). Value high, effort low–medium.
3. **Org-wide freeze ("E-stop") that exempts theuth's own desk.** One switch pauses all desks, routines and children, records manual or auto provenance (for example a budget breach), and resumes them on thaw. *Lands:* roster (agent `paused`), `crates/schedule`, HUD/companion, RPC. Value medium–high, effort low.
4. **Rate-limit-aware restarts.** Classify provider and harness limit errors so they get a fixed pause outside the retry budget and show "paused: quota" instead of "crashed". *Lands:* ACP child restart path (146), desk wake retry, `crates/agent` provider error mapping. Value medium, effort low.
5. **Re-inject the current checkout after every memory fold.** The desk manual already heads each episode. Add the hooked task's title, `done_when` and last progress note to the endless/compaction re-render. *Lands:* `crates/agent` endless render (137), desk manual in `crates/roster`. Value medium, effort low.
6. **Escalation severity + stale re-escalation timer.** `escalate()` gains `severity`. An unanswered escalation re-raises after T, at most twice, on a wider channel (Signal/Slack). *Lands:* `crates/tasks/src/escalate.rs`, attention inbox, channels. Value medium, effort low.
7. **Message TTL/priority for `send_message` to busy peers.** Expiry, depth cap and visible "expired unread" receipts. *Lands:* design 50 peer exchange. Value medium, effort low.
8. **Patrol skill and routine pattern.** A deterministic sensor emits a structured scan; the routine's LLM acts only on exceptions; a cheap "boot" routine checks the supervisor's last fire. *Lands:* `crates/schedule` sensors + `skills/loops.md`/`routines` skill. Value medium, effort low (docs and skill).
9. **Task templates (formula → task DAG) + distil.** A TOML/YAML template of steps with `needs`, `done_when` per step, owner role and vars. `task_draft` instantiates it; "distil" turns a finished parent and children into a template. *Lands:* `crates/tasks` + `task_draft` card + routines (a routine can instantiate a template on a clock). Value medium–high, effort medium.
10. **Typed blockers: `waits-for` (fan-in), `if-failed` (conditional) and `gate:` blockers** (timer, CI run, human). theuth's blocker grammar already has `decision:` and `person:` (`theuth/crates/tasks/src/types.rs#L213-L214`). *Lands:* `crates/tasks` types + readiness. Value medium, effort medium.
11. **Pool claim-next for explicitly pooled roles.** For example "any reviewer desk", using one transactional pick over the same predicate the board shows. *Lands:* `crates/tasks` store + design 67 amendment. Value medium, effort medium (an authority-model change; needs Sky).
12. **Land-queue skill for omp waves.** It encodes Sky's integrate/<n> practice as batch → gate → retry-once → bisect → bounce conflicts as R-messages. Start as an orchestrator skill, not engine code. *Lands:* skills + the build-lock protocol. Value medium for Sky's waves, effort medium; an engine service would be high effort.
13. **`t doctor` with named fixable checks** (vault, MCP files, agents, schedules, mesh reachability, stale binaries, sandbox runtime) plus a nightly doctor pass that files one attention item. No doctor was found in `crates/*/src` by filename grep. *Lands:* CLI + dreaming window. Value medium, effort medium.
14. **Model-downgrade evidence per agent role** using fixed decision fixtures, possibly run as an autoresearch study (140). Value medium, effort medium.
15. **Seance-style "ask the predecessor"** for native sessions: a read-only fork of a stored session answering one question. *Lands:* `crates/agent` session fork + `session_read` neighbour. Value low–medium, effort medium.
16. **Char budget on injected knowledge hints and memories**, with a warning at 80%. *Lands:* knowledge hints, dreaming. Value low–medium, effort low.

## Their weaknesses and failure modes
1. **Activity and ownership.** Gas Town main has been frozen since 2026-07-23 and October PRs are closed unmerged (A11). Users file into a repo whose fixes now land in Gas City. Open issues: 507 (GT), 1,421 (Beads), 1,513 (Gas City).
2. **TUI driving is brittle.**
   - #4818 (2026-09-05, open): "Polecat spawn quits at Claude Code trust dialog: 'No, exit' is now pre-selected, gt presses Enter". The blind dismissal (`gastown/internal/witness/handlers.go#L2245-L2251`) presses the wrong key after a vendor UI change.
   - The nudge path special-cases Claude's Rewind menu, Gemini and Copilot Escape semantics, and readline's 500 ms keyseq timeout (`gastown/internal/tmux/tmux.go#L1805-L1869`).
   - #4887: a usage-limit block "is indistinguishable from a hang in gt dog status — duration-based stuck detection terminates" the session.
3. **Reapers and janitors destroy live work.**
   - #4890: the idle reaper kills a freshly slung polecat 1 s after dispatch, reading stale last-activity.
   - #4911: reaper stale auto-close "closed 5 live bug reports in one day".
   - #4630: agent registry beads auto-closed on a 7-day clock that heartbeats do not reset.
   - #4885: `wisp_reaper` overrides a 30-day default, so live P2 issues auto-close.
   - #4909/#4908: unrendered Handlebars stripped `--dry-run`, so "gt reaper purge has always run live".
4. **State-machine wedges.**
   - #4889: `check-recovery` returns PENDING_MR forever when `active_mr` points at a GC'd wisp, "making every polecat unreusable until the rig hits its directory cap".
   - #4886: successor patrol wisp created with 0 steps.
   - #4880: a `doctor --fix` that can never succeed.
   - #4917: UTC `created_at` read as local time.
5. **Complexity and operations.**
   - About 249k lines of Go for GT and about 426k for Beads (non-test).
   - 72 doctor check files.
   - A Dolt server per town with phantom, orphan and duplicate database failure modes, including a "silent data split" fixed in 1.2.0 (`gastown/CHANGELOG.md#L43-L48`).
   - Identity is encoded in id prefixes: #4916, a hyphenated rig prefix breaks bead routing.
   - Durable mail churned Dolt commits until it was pared back (`gastown/CHANGELOG.md#L66-L67`, `gastown/internal/refinery/engineer.go#L1615-L1618`).
6. **Cost.**
   - The default tier is Opus for every role (`gastown/internal/config/cost_tier.go#L13-L14`), with always-on LLM patrols (Deacon, Witness per rig, Refinery per rig, Boot every tick).
   - There are no spend caps. Costing is after the fact, with a pricing table dated "Jan 2025" (`gastown/internal/cmd/costs.go#L198`).
   - The project's own eval suite exists because patrols "burn through Opus budget" (`gastown/gt-model-eval/README.md#L7`).
7. **Safety.** All agents run with skip-permissions and bypass mode. Protection is a short deny-pattern list (`gastown/internal/cmd/tap_guard_dangerous.go#L13-L27`).
8. **Claude-centric despite 13 presets.** Their own survey: "the role template system (`gt prime` injecting Go templates via CLAUDE.md conventions) is still deeply tied to Claude Code's priming model" (`gastown/docs/research/w-gc-004-agent-framework-survey.md#L501`). Seance needs `--fork-session` support (`gastown/internal/cmd/seance.go#L194-L195`).
9. **Docs versus code.**
   - The glossary says Boot checks the Deacon "every 5 minutes" (`gastown/docs/glossary.md#L35-L36`). The code runs Boot from the daemon heartbeat (default 3 min) with a 2-min spawn cooldown and 15-min idle suppression (`gastown/internal/daemon/daemon.go#L906-L911`, `gastown/internal/config/operational.go#L48-L50`). Trust the code.
   - The README calls hooks "git worktree-based persistent storage" (`gastown/README.md#L68-L70`). In code the hook is the `hook_bead` column on the agent bead in Dolt (`gastown/internal/daemon/lifecycle.go#L1097`); worktrees are the sandbox.

## Cites self-check
All 61 cites below were re-opened at the end with `read_file` over the exact range and a keyword match (script in the session runtime): **60 held, 1 corrected**.
Other ranges cited in the text were read while writing and not re-opened.
- `gastown/docs/glossary.md#L10-L11`: held
- `gastown/internal/daemon/lifecycle.go#L1138-L1153`: held
- `gastown/internal/daemon/daemon.go#L846-L852`: held
- `gastown/internal/daemon/daemon.go#L862-L869`: held
- `gastown/internal/daemon/daemon.go#L930-L943`: held
- `gastown/internal/config/operational.go#L48`: held
- `gastown/internal/config/operational.go#L19`: held
- `gastown/internal/config/roles/polecat.toml#L13`: held
- `gastown/internal/witness/spawn_count.go#L66-L93`: held
- `gastown/internal/witness/handlers.go#L2104-L2163`: held
- `gastown/internal/witness/handlers.go#L2245-L2251`: held
- `gastown/internal/tmux/tmux.go#L1836-L1869`: held
- `gastown/internal/nudge/queue.go#L1-L51`: held
- `gastown/internal/refinery/batch.go#L10-L35`: held
- `gastown/internal/refinery/engineer.go#L1562-L1613`: held
- `gastown/internal/refinery/engineer.go#L1615-L1618`: held
- `gastown/internal/refinery/engineer.go#L107-L108`: held
- `gastown/internal/refinery/engineer.go#L186`: held
- `gastown/internal/hooks/templates/claude/settings-autonomous.json#L107-L146`: held
- `gastown/internal/estop/estop.go#L1-L9`: held
- `gastown/internal/config/cost_tier.go#L12-L29`: held
- `gastown/internal/cmd/costs.go#L198`: held
- `gastown/internal/quota/rotate.go#L10-L39`: held
- `gastown/internal/daemon/usage_limit.go#L5-L14`: held
- `gastown/internal/daemon/restart_tracker.go#L12-L52`: held
- `gastown/internal/cmd/seance.go#L37-L62`: held
- `gastown/internal/cmd/handoff.go#L35-L65`: held
- `gastown/docs/design/escalation.md#L41-L63`: held
- `gastown/docs/research/w-gc-004-agent-framework-survey.md#L13`: held
- `gastown/docs/research/w-gc-004-agent-framework-survey.md#L501`: held
- `gastown/CHANGELOG.md#L8-L10`: held
- `gastown/CHANGELOG.md#L43-L48`: held
- `beads/issueops/readyclaimer.go#L5-L44`: held
- `beads/internal/types/types.go#L57-L73`: held
- `beads/internal/types/types.go#L1427-L1457`: held
- `beads/issueops/leasereclaimer.go#L74-L93`: held
- `beads/cmd/bd/memory.go#L344-L363`: held
- `beads/cmd/bd/compact.go#L45-L58`: **corrected**. The phrase "memory decay" is the README's (`beads/README.md#L65`); compact.go says "permanent graceful decay". A8 now cites both.
- `beads/cmd/bd/mol.go#L37-L58`: held
- `beads/cmd/bd/prime.go#L91-L126`: held
- `theuth/crates/tasks/src/types.rs#L177-L195`: held
- `theuth/crates/tasks/src/ledger.rs#L2064`: held
- `gastown/internal/tui/feed/stuck.go#L32-L36`: held
- `gastown/internal/deacon/redispatch.go#L17-L27`: held
- `gastown/internal/daemon/daemon.go#L1525-L1636`: held
- `gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L40`: held
- `gastown/internal/formula/formulas/mol-witness-patrol.formula.toml#L1`: held
- `gastown/internal/daemon/daemon.go#L906-L911`: held
- `gastown/internal/config/operational.go#L48-L50`: held
- `gastown/internal/daemon/lifecycle.go#L1097`: held
- `gastown/README.md#L68-L70`: held
- `gastown/internal/cmd/sling.go#L94-L99`: held
- `gastown/docs/design/scheduler.md#L28-L38`: held
- `gastown/internal/daemon/pressure.go#L31-L53`: held
- `gastown/internal/cmd/tap_guard_dangerous.go#L13-L27`: held
- `gastown/internal/acp/propulsion.go#L130-L197`: held
- `gastown/gt-model-eval/README.md#L3-L7`: held
- `beads/README.md#L56`: held
- `gastown/internal/mail/types.go#L1-L40`: held
- `gastown/internal/config/agents.go#L25-L54`: held
- `gastown/internal/beads/status.go#L15-L28`: held
