# 171 — CELLS: code the user runs with one press, in chat, notes and notebooks

Status: design v0 · 2026-11-03 · proposed — Sky asked for the design and ruled R1 (no licence gate; least code, highest quality); the forks F1–F6 in §0/§18 are open with their recommended defaults applied · extends 59, 109, 120, 126, 143, 144/157, 155, 163, 164, 167 (§19 lists the amendments) · recon: 171-recon-ours.md, 171-recon-theirs.md, 171-recon-field.md · mock: docs/design/assets/171-cells.html

Every `path:line` below was re-opened in `~/Develop/theuth-171` (branch `docs/171-cells`, base origin/main 6eba65b1) on 2026-11-03. nteract cites are `~/Develop/spike-171/nteract-nteract/<path>:<line>` (short form `nteract:` below) or the elements clone `nteract-elements:`. The calls this design implements are `docs/design/171-calls.md` D1–D20; the ask and the evidence are `docs/design/171-axes.md`.

## 0. Decisions in one screen

1. **One grammar (D1).** A cell is a fenced block in a runnable language whose body starts with `#|` option lines (`//|` for JavaScript); the same text works in a chat reply, `doc_insert` markdown, a note code block and an exported `.md`. A fence without the header is a snippet (copy only). Rejects a `present_cells` tool, Runme JSON in the info string, an `app` screen, and "every bash fence is runnable". Why the body: a note's code block stores one prop, `language` (`packages/documents/src/schema.ts:73`).
2. **A closed option set (D2).** `cwd name label danger after expect env python review private timeout`; `on` reserved for wave 3; unknown keys warn. Note-level defaults live in a leading ```` ```cells ```` block (writer's call W-c; notes have no front matter, §0 Calls vs tree 1). Rejects open-ended metadata. §3.2.
3. **Human press only; what you saw runs (D3).** A run starts only from an owner-only RPC sent by a desktop/web/phone client on a press or shortcut, carrying the digest of the text on screen; the engine reads the source from its own record and refuses a mismatch with "this cell changed — look again". Nothing else can start one: no tool, runtime function, MCP tool, routine, subagent, ACP child, double, guest, channel, nor the in-process owner console peer the MCP server and ACP bridge use (W-n). Rejects routing through `tools.policy` and nteract's agent-runs-cells model. Guard shape: `nteract:crates/notebook-protocol/src/protocol.rs:738-745`.
4. **Engine-owned kernels, one per (scope, spec) (D4).** Scope = chat session or document. Shell kernels are PTY jobs marked `cell` (invisible to `jobs()`, refused by `send`/`cancel_job`); `python` is a local interpreter running theuth's stdlib-only shim; `monty` is an isolated tool-less checkout; JavaScript and a Jupyter backend are wave 3. Idle shutdown after 12 h. Rejects Jupyter-for-everything and a process per run. `crates/tools/src/pty.rs:987`.
5. **The user's real environment (D5).** Kernels start from the login-shell environment captured once per engine start (refreshable); the cell's language picks the program (a `bash` cell runs bash even when the login shell is zsh); theuth rc snippets emit OSC 133 marks and OSC 7 cwd so completion and exit codes are exact. Rejects the engine's launchd env and prompt-pattern waits. `crates/platform/src/env.rs:2` (only `PATH` is probed today).
6. **Code enters by sourcing a run file (D6).** Every run writes the exact code to a file and `source`s it; a missing cwd blocks the run (never a fallback); a whole-cell run `cd`s and exports first, a line/selection run does so only when the previous run came from another cell. Rejects typing into the PTY. §5.6.
7. **Interpreters resolve deterministically (D7).** `auto`: nearest `.venv`/`venv` → uv project → pyenv/conda → `python3` on the login PATH → none; Monty when it is sufficient, checked by its type checker at presentation. The chip shows what will run and is a menu of `interpreters(cwd)`. Rejects a kernel picker separate from the project toolchain (Zed's trap, `https://zed.dev/docs/repl`). §6.
8. **Jupyter's output model; the engine cleans terminals (D8).** nbformat v4 outputs plus `exit` and `expect`. Shell runs stream live into an xterm the user can type into; when the run ends the engine renders its byte range through `vt100` into plain text for the record and the agent. Record first, broadcasts are hints; Stop never waits behind output. Rejects client-side regex cleaning. `Cargo.toml:270` (`vt100 = "0.18"`).
9. **nteract's components, theuth's tokens (D9, R1).** Adopt nteract/nteract `src/components` (production) and the elements registry where production lacks a piece; @uiw/react-codemirror (already a dependency) for the editor; xterm for live shells; HTML only inside 120's frame. New UI code is only what nothing provides: run-line, run-selection, gutter run, bash language wiring, the chat fence, the note branch. Rejects writing our own output renderers. §16.
10. **Chat placement (D10).** Cells render inline as the agent writes them; ≥ 2 cells get a runbook bar whose Run all names its boundary; a kernel strip shows whenever a kernel lives; finished cells fold; a re-presented `name` supersedes the older cell. Rejects a side panel. Mock panel A.
11. **What the agent sees (D11).** One `<cell_result>` per finished run: attributes plus emulator-cleaned output, head+tail capped at 8 KiB, vault values and credential patterns masked; `private` sends exit/expect only; `<cell_error>` once for invalid cells; read-only `cell_runs`/`cell_output`/`interpreters`. Rejects streaming partial output into context. `crates/agent/src/supervisor.rs:3093` (`job_result` is the model).
12. **Wake rule (D12, F1 a).** A result joins a running turn; otherwise it waits quietly for the next turn unless `review: true` (or the press of "Ask theuth") starts one. §9.3.
13. **Safety without confirmations (D13).** Consequence labels, danger styling, Run-all boundaries, `after` gates that show the preview's summary on the armed button, a deterministic lint, secrets substituted only at run time and scrubbed everywhere, nothing ever re-run automatically. Rejects a second confirmation dialog (`https://www.nngroup.com/articles/confirmation-dialog/`). §12.
14. **Notes (D14).** A note code block whose body starts with `#|` is a cell: no schema change, no new node, no Yjs change; `CodeBlock.tsx`'s render mounts the cell; run state lives in an engine run store, never in the Y.Doc. Rejects outputs in the CRDT. `crates/desktop/ui/src/components/editor/CodeBlock.tsx:16`.
15. **Sharing (D15).** Members edit, only the owner runs, on the owner's engine; a member-changed cell reads "Run Alice's edit"; guests see replicated latest outputs, never a Run button. Rejects edit-implies-run (`nteract:docs/adr/hosted-room-authorization.md` per 171-recon-theirs.md §3.11). §13.
16. **Surfaces (D16).** Desktop, web and phone run cells (the host chip names the engine when it is not this device); TUI shows code with "run in the app"; Slack/Telegram/Signal send code plus a "Run in theuth" link; a long run that ends unfocused notifies. §14.
17. **Audit (D17).** Every run is a durable record: presser, exact code and digest, presented digest, cwd, interpreter, env names, times, exit, expect, output digest, log path. §8.4.
18. **Prompting (D18).** A ≤ 18-line `## User cells` block in attended sessions plus a binary-owned `cells` skill; "prep it, don't apply" means present cells, sticky until lifted; today's "commands to run stay a plain code block" is amended in both places it lives. §10.
19. **Where it lands (D19).** New plugin crate `crates/cells`; the pure grammar in `crates/surface/src/cell.rs`; RPC in `crates/rpc`; delivery through `agent::HarnessDeliver`; UI in `components/cells/`. §15.
20. **Waves (D20).** W1 CHAT-SHELL, W2 PYTHON-NOTES, W3 REACH. §17.

**Forks for Sky** (default applied; §18 explains each)
- F1 Wake rule — **(a)** only `review: true` or "Ask theuth" starts a turn.
- F2 Outputs in shared notes — **(a)** the latest run's text (capped, scrubbed) replicates to members.
- F3 Phone — **(a)** the phone runs every cell, danger included, with the same label.
- F4 Editing agent cells in chat — **(a)** allowed, "edited" badge, the agent sees what ran.
- F5 JavaScript — **(a)** wave 3, node/deno/bun through the same shim protocol.
- F6 Engine restart — **(a)** 109 R1 unchanged; quit and `t update` offer "Wait for <label>" or "Stop it and quit".

**Writer's calls**
- W-a Placement: plugin crate `crates/cells` (actor `CELLS_ACTOR`), grammar and lint in `crates/surface/src/cell.rs` because agent, document, tui and the three channel crates already depend on `surface` (§15.1).
- W-b Note kernels' PTY jobs use a deterministic kernel session id (UUIDv5 of the doc id); clients attach through `cell_attach`/`cell_input`, one path for chat and notes (§5.1).
- W-c Note defaults in a leading ```` ```cells ```` block holding `#|` lines (§3.3).
- W-d Chat cells run on the session's owner (`forward::route`); note cells on the document's hub (§5.1, §13).
- W-e Python shim protocol: newline-delimited JSON on the shim's stdin and a private dup of its stdout; nine frame types (§5.8).
- W-f Run store `~/.theuth/cells/cells.sqlite` + `~/.theuth/cells/<run>/`; 30 days, latest run per cell kept forever (§8.4).
- W-g Latest-output replication: a fifth replicated table `cell_latest` in `documents/index.sqlite` (§13.2).
- W-h OSC 133/OSC 7 rc snippets per shell; marks read through `vt100::Callbacks::unhandled_osc` (§5.5).
- W-i `<cell_error>` is a pass inside `check_fences`, one round per turn, hidden on the desktop like `<surface_error>` (§9.2).
- W-j Kernel strip sits directly on the composer (transcript · dots · to-do · kernel strip · composer); in notes it is a pill in the header (§11.5).
- W-k Config keys beyond the calls (§15.6).
- W-l Prompt heading `## User cells`, not `## Cells` (§10.1).
- W-m Quiet delivery generalises 145's park: `HarnessDeliver { wake: false }`, re-parked at start from the run store (§9.3).
- W-n Cell RPCs refuse `RpcPeer::internal()` callers (MCP server 158, ACP bridge, TUI) unless forwarded (§12.1).
- W-o The block is present in attended channel sessions (Slack/Telegram/Signal), absent for unattended runs and subagents (§10.6).
- W-p Kill is enabled 5 s after Stop (mock panel B) (§7.3).
- W-q Chat identity is `(session, entry, ordinal-among-cells)` unless `name` is set (§3.4).

**Calls vs tree**
1. D2 puts note defaults in "the note's front matter block": only knowledge documents carry one (`docs/design/59-documents.md:485`, `packages/documents/src/schema.ts:4`) → a ```` ```cells ```` block (W-c).
2. 171-recon-ours.md §1.2 says streamdown passes no `meta`: wrong — 2.6.0 passes `meta?: string` (`~/Develop/theuth/crates/desktop/ui/node_modules/streamdown/dist/index.d.ts:304-316`). D1 is unaffected: notes and channels drop the info string.
3. D18 names the block `## Cells`; the runtime block already calls a Monty invocation "a cell" (`crates/agent/src/repl.rs:35`, `:81`) → `## User cells` (W-l).
4. D8 "reuse whatever emulator the PTY layer has": it is `vt100` 0.18 with 512 history rows (`Cargo.toml:270`, `crates/tools/src/pty.rs:71`), not alacritty → a second, render-only `vt100` parser with 10 000 rows per finished run (§8.3).
5. D4 "PTY on the existing layer": `StartPty` always runs `$SHELL -lc <command>` (`crates/platform/src/shell.rs:45-49`) → `StartArgs` gains an explicit `program` (§15.1).
6. D8 "reusing TerminalViewer's attach": `subscribe_pty`/`pty_input` authorize by chat session (`crates/rpc/src/api.rs:2847`); a note kernel has none → `cell_attach`/`cell_input`; `Terminal` gains a source prop and keeps 167's keys (W-b).
7. D3 "authenticated owner client": `rpc::client::Local` gives the MCP server, the ACP bridge and the TUI the owner console's peer on every call (`crates/rpc/src/client.rs:24-31`, `crates/mcp-server/src/lib.rs:6-8`) → explicit refusal (W-n).
8. D12 "delivered quietly": no quiet input exists; `inject` admits `Automatic`, which starts a turn on an idle session (`crates/agent/src/supervisor.rs:1496`, `:1540`) → W-m.
9. D11 "notebook chats get a `<notebook_changes>` notice": the notice lists document changes (`crates/agent/src/notebook_context.rs:700`); a run changes no document → a `cell` line kind is added (§9.6).
10. D16 "Run in theuth deep link": `theuth://c/<session>` exists but the route drops everything after the path (`crates/desktop/src/lib.rs:607-619`) → the route keeps `?cell=` (§14.3).
11. D19 `skills/cells.md`: skills are binary-owned constants written by `first_run` (`crates/theuth/src/main.rs:1324-1336`) → `crates/cells/cells.md` via `include_str!`, written to `~/.theuth/skills/cells.md`.
12. D9 `@uiw/codemirror-extensions-langs`, `anser`, `escape-carriage` are not installed (`crates/desktop/ui/package.json:39-41` has only react-codemirror and xterm) → three new UI dependencies (§16).
13. F6 (a) "the graceful quit screen names running cells": 163's overlay has no buttons by design (`docs/design/163-graceful-quit.md:61`) → the choice is a sheet shown before `requested` is dispatched (§5.4).
