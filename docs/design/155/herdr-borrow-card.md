# herdr → theuth: what to learn and borrow (2026-10-09)

**herdr** (herdrdev/herdr, v0.9.3, Apache-2.0, ~43 K stars, YC + $6M seed, one founder plus bots) is a single Rust binary: a background terminal server and a TUI client that own the terminals of 23+ coding agents (Claude Code, Codex, OpenCode, omp, …). It marks each pane **blocked / working / done / idle / unknown**, keeps processes running when the client closes, resumes agent sessions after a restart, lists agents across SSH machines, and lets agents drive it through a CLI and socket API. getone.one ("One") only works on top of it.

## Two bets
- **herdr — the terminal is the interface.** Own the PTY, read the live bottom of the screen against per-agent rules, accept hook reports. That way it can supervise any agent without wrapping it.
- **theuth — the protocol is the interface.** ACP children and theuth sessions report exact state (ACP already has `RequiresAction`), and typed approvals and asks become attention items. Terminals exist for the model.
- **What follows:** herdr's best ideas land at theuth's edges. That means PTY jobs, harnesses that don't speak ACP, notification rules, resume handles and worktrees.

## Axis by axis
| Axis | herdr | theuth today | Live plan |
|---|---|---|---|
| A1 State detection | TOML rules per agent; "blocked" only on recognised UI; typed fallback; `agent explain` | ACP: exact (141). PTY: "screen unchanged N s" only; wakes the model, not you | — |
| A2 Identity & resume | env contract, `report-agent --seq`, resume argv; incomplete hooks reduced to identity only (0.6.7) | `acp_session` + `session/load`; PTY omp resumed by hand | 146 (ACP respawn, **not on main**) |
| A3 Durability | server owns PTYs; restart loses processes; `--handoff` fd transfer (best effort) | turn journal + eager resume (109); PTY/shell jobs die **by ruling D14** | 109 §14 `t jobd` rejected for now |
| A4 Control API | 102 methods, `agent wait --until blocked`, prompt-effect gate, JSON schema | 408 RPC methods; no wait-on-state; no wire version | — |
| A5 Supervision UX | alerts only on →blocked or background finish; per-client "seen" | 14 attention kinds, native companions, HUD, channels | **149 tonight** (One parity) |
| A6 Multi-machine | SSH panes, one combined list, cached state dimmed | iroh mesh, replication, VMs; **no host field on agents** | 149 F2 needs it |
| A7 Terminal engine | libghostty-vt (Zig, 5 K-line FFI); per-client render deltas | vt100 grid per PTY + xterm.js; one size for all viewers | — |
| A8 Worktrees & plugins | engine-managed worktrees; argv plugins | worktree writer only in autoresearch; ~26 worktrees cut by hand | t3code list item, unbuilt |
| A9 Practice | AGENTS.md, triage/repro/audit skills, schema diff test | 3.4 K tests, 507 design docs; no AGENTS.md or CHANGELOG | — |

## What herdr's users notice
- **Screen detection keeps breaking, even with rules and `explain`.** Status stuck on working (#198). A title rule outvoting a visible question (#4573). Nix-wrapped Claude invisible (#803). State tied to the pane, not the process (#4732). Codex's daemon breaking identity (#4649).
- **Speed matters more to them than detection.** A 60 Hz cap is the top-reacted issue (#1134). The server burned 10–13 cores with ~20 idle panes (#4506).
- **Integrations from agent authors:** five agents report to herdr natively. OSC 9;4 progress is the only signal agents send their terminal that a program can read. Claude Code emits it only to allow-listed terminals, and Codex doesn't emit it at all.

## Borrow — now (small to medium; each answers a recorded pain)
1. **Blocked/working detection for PTY jobs (M).** Per-program rules over named screen regions. "Blocked" only on recognised UI; otherwise idle with a reason. A foreground-process check first (`tcgetpgrp`), then a 3×100 ms idle recheck and a 3 s startup grace. Keep OSC titles and progress instead of stripping them. **Lands:** `crates/tools/src/pty.rs` next to `Idle::saw`, as a new `Notice::JobState` and an attention item.
2. **Wait on state, and confirm a prompt landed (S–M).** `wait_state(id, until=blocked|idle|done)` that ends if the agent in that slot changes. `send()` reports *stalled* when nothing moves within 5 s, which is different from a timeout. Prompting an already-blocked agent is refused. **Lands:** beside `Wait` in pty.rs, plus a `wait_peer` for children.
3. **Trust tiers per harness (S).** Only harnesses with a complete event stream own their state. The others contribute identity and a resume handle only. That is herdr's 0.6.7 reversal for Claude/Codex/Copilot. **Lands:** `acp::agents::Entry`.
4. **When to alert (S–M).** The engine decides *whether*; each surface decides *how*. Alert only when an agent becomes blocked or finishes out of view, never for the view you're in. Re-check state just before delivering. **Done ≠ idle**, with a "seen" flag per client. **Lands:** roster attention and the companion, so it's input for **149**.
5. **One agent list across machines (S–M).** Add a host field to `AgentInfo`, dim cached state when a link drops, and never let a reconnect steal your selection. Reset the reconnect backoff only after 60 s healthy. **Lands:** `crates/agent/src/lib.rs`, so it's 149's F2 frame.

## Borrow — next (medium)
6. **Native resume for PTY agents.** Every PTY job gets a `THEUTH_*` env contract. The agent reports its resume command, or a rule harvests it from the screen, into `sessions.pty_resume`. On boot, theuth runs it again, deduplicated on (source, agent, kind, value, cwd). The process still dies, so **D14 stays intact**. This is your 109 choice of "journal + native resume" extended to PTY agents, and it ends the hand-run `omp --resume`.
7. **Engine-managed worktrees.** Lift `autoresearch::git` into `spawn_subagent{workspace}`. From herdr: never delete the branch; refuse a dirty removal and ask again; recover leftover checkouts.
8. **Compatibility and schema.** A committed JSON schema checked by a failing test, capability flags, and explicit restart reasons (keep running / hand off / stop required). This covers engines, desktop and companions drifting across releases.

## Practice (small, no code)
9. A root `AGENTS.md`, repo skills for triage, throwaway repro (on a disposable engine) and pre-release audit, a "what survives" table in 109's as-built doc, and a patch-governance file for vendored crates.

## Conflicts with your rulings
- herdr's server that owns the processes, and `update --handoff`, are the job-adoption / `t jobd` idea you rejected in **109 (D14)**. The handoff passes terminal handles to the new server in seven steps, with a rollback. herdr itself loses processes on a server restart, and its handoff is Unix-only, opt-in and "best effort". **Not recommended.** If you ever reopen it, the cheapest step is passing PTY handles to the new engine on `t update` (L).

## Do not borrow
The Ghostty/Zig terminal engine. The 6.4 K-line hook/screen arbiter. Screen regexes as the main source for ACP harnesses. Resume commands typed into a shell. 100 ms polling waits. herdr's unsigned remote rule catalog. Unsandboxed plugins and a marketplace. Overriding `TERM_PROGRAM` (it turns Claude Code features off, #4748).

## Order
- **Tonight:** items 4 and 5 as input to the 149 session (design now, build overnight).
- **Next design:** PTY-STATE = items 1, 2, 3 and 6 in one wave (`crates/tools` pty, agent peers, acp agents, store).
- **Later:** item 7 (worktrees), item 8 (compatibility), item 9 (practice) whenever.

## Open questions
1. Keep jobs non-durable (D14), or reopen it for update-only handle transfer? Recommended: keep.
2. Run Claude Code and Codex as PTY agents with detection rules (identity-only hooks), or wait for ACP adapters?
3. Own `THEUTH_*` contract (recommended) or also speak herdr's `pane report-agent` for the five agents that adopted it?

---
Sources: `~/.theuth/briefs/2026-10-09/herdr/`: theirs-core.md (453 lines, 334 cites), theirs-surface.md (457, ~290), ours.md (742, 99), web.md (184, 27 sources). Cite check: 33 re-opened in both trees; 26 held as cited, 6 held a few lines away, 1 unconfirmed. Knowledge: `facts/theuth-borrow-list-from-herdr-2026-10-09`, `facts/theuth-agent-runtime-seams-vs-herdr-2026-10-09`, `facts/herdr-agent-terminal-runtime-recon-2026-10-09`, `facts/agent-terminal-state-signals-osc-9-4-titles-hooks-2026-10-09`.
