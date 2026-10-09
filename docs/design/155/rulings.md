# 155 ACTIVITY — rulings (Sky, 2026-10-09)

Source: Sky's chat with the orchestrator (session 01a11dfc-9e80-7cf2-8dfb-16f78fae8eed), 16:12–19:05; go at 19:05 "build it, omp subagents". Knowledge: decisions/theuth-design-155-activity-rulings-2026-10-09.

Sky's stance (his words): "really what we're betting on is ACP not driving everything over a terminal … the PTY mode for running coding agents … is kind of just like a crutch or a compatibility mode … I don't want to invest too much in it … I definitely don't want to end up having to write custom plugins for every other harness." And: "I definitely like the idea of the stalled ACP child alerting the parent."

| # | Ruling | Changed from recommendation? |
|---|---|---|
| D1 | One engine **activity monitor** per ACP child (and the PTY job signals below) feeds three consumers: the stalled wake, wait-on-state, and the alert rules. | no |
| D2 | **Stalled ACP child wakes its PARENT ONLY** — a new input to the parent session, like `<job_idle>` today; the parent decides whether to escalate to Sky. No attention item for Sky in v1. | no (recommended) |
| D3 | Stall thresholds: **no session update within 60 s of a prompt being delivered**, or **180 s of silence mid-turn with no tool call in progress**; both adjustable in config; one wake per stall episode, re-armed by the next update. | no (recommended) |
| D4 | **Wait on a child's state**: an orchestrator can wait until a child is `blocked`, `idle`, `done` or `stalled`; the wait is pinned to that child's identity (a replacement cannot satisfy it) and ends with `not_running` if it goes away; optional timeout. Non-blocking registration that wakes the caller (theuth style), surviving restarts like other waits (109). | no |
| D5 | **PTY busy/quiet from generic signals only**: screen changes (spinners included), child processes under the PTY, CPU of the process tree, and reaction to input (after `send()`, output or tree activity must start within a few seconds, else "no reaction"). `<job_idle>` carries **why** it fired. No per-harness rules, no plugins, no network signal in v1. | no |
| D6 | **Alert rules for Sky — the engine decides WHETHER, clients decide HOW**: alert only when an item needs Sky or a session finished while unseen; never for the session in view on an active client; re-check just before delivery (short delay); **done ≠ idle** with a per-device seen bit; every alert says what triggered it. macOS companion + desktop UI consume it; the Windows companion is untouched in this wave (follow-up brief). | no |
| D7 | **Capability entry per ACP harness, as data** (in the agents entry): which states the harness reports reliably (permission requests, turn end, resumable session). Incomplete harnesses do not drive alerts. No plugins. | no |
| D8 | The local decision model's reasons ("an error is showing", "it is asking you something") are **not in v1** — evidence: docs/design/155/eval-decision-model.md (zero-shot 42–49 %). | no (recommended) |
| D9 | Out of scope: process survival across restarts (109 D14 stands), per-harness screen rules or extensions, the model deciding state, engine-managed worktrees (a cleanup routine separately), network-traffic signals. | no |
| D10 | Built by **omp ACP workers** (spawn_subagent agent omp), contract-first. | Sky's words |
