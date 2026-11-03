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
13. F6 (a) "the graceful quit screen names running cells": 163's overlay has no buttons by design (`docs/design/163-graceful-quit.md:56`) → the choice is a sheet shown before `requested` is dispatched (§5.4).

## 1. Why

Sky's ask, verbatim (2026-11-03 16:58, `docs/design/171-axes.md`):

> https://github.com/nteract/elements
> https://nteract-elements.vercel.app/docs
> https://github.com/nteract/nteract
> https://uiwjs.github.io/react-codemirror/
> Practical recommendation for your setup
> Start with nteract/elements for the cell chrome, toolbars, and output area (Shadcn-native).
> Use @uiw/react-codemirror (or Monaco) for the editor itself if you need finer control — style it with your existing Shadcn theme.
> Implement the interaction layer yourself:
> Run cell → send whole cell source over RPC to Rust
> Run selection / current line → extract the relevant code and send that
> Run in order → sequential RPC calls with shared kernel/session state on the backend
>
> Feed the results (stdout, stderr, rich MIME data, images, etc.) into the output renderers from nteract/elements or @runtimed/components.
> This gives you a Jupyter-like experience (cells, sequential execution, rich outputs) while keeping full control of the Rust backend and RPC protocol. You avoid having to speak the full Jupyter kernel protocol unless you want to.
>
> So I want you to write a design document for a new feature and enhancement to theuth.  right now we have the notebooks, but I don't think they have any code execution capability or anything like that.  So I want to add that as part of the notes and notebooks using these UI elements properly styled to match theuths theme.   In addition to that, I want an agent to be able to present cells to me with a specific cwd and stuff running either bash or python, and the ability for the agent to select a local python interpreter if one exists or use the built in monty if it's sufficient for the use case.  The idea being that the agent can present a shell command to me in a chat that I explicitly told it to prepare but not run, and then I can click a button to run a single line or the entire script and both I and the agent can see the output.  I can watch it live in the chat, and the agent can see it in context once it completes.
>
> So it's give the agent a way to allow me to press a button to execute bash or python (separate from how we execute tools/mcps now, this is a user facing thing, not replacing or changing the tools/mcp setup in any way)
>
> and allow notes and notebooks to have the same kind of jupyter notebooks setup but suppport both bash and python, maybe even javascript too.
>
> Let's think through this and figure out how we can align the feature with the design principles of theuth and achieve the goal of the feature while reducing the users cognitive load and making work more efficient for the user.
>
> we also need to think through the prompting to explain to the agent when to use it.  For example I often when working with infrastructure as code will tell it, you know prep all the code but don't apply it because production danger, just present me with the commands to run.  so when I request something like this, the agent should understand that means, show me these cells in chat or in the notebook, so I can actually execute the commands without leaving the agent window.

Then, after the licence finding: *"why do we care if there's no license. the focus is the least code and highest quality product. we don't care about the license issues."* (R1, `docs/design/171-calls.md`).

**The cost of today** (Sky's October DR runs, `docs/design/171-axes.md` "User evidence"):
- Desks write **run sheets** as notes: a checkbox heading per step, a ```` ```bash ```` block, `You should see: <exact output>`, stop lines ("If it doesn't say exit 0, stop and tell theuth"), `; echo "exit $?"` appended to surface exit codes, `--profile/--context` on every line. Sky copies each block into Terminal.app and ticks by hand. **Agents tick nothing.**
- Sky's login shell is zsh; bash blocks pasted into interactive zsh break (`${PIPESTATUS[0]}`, a trailing `# comment` without `interactive_comments`), so sheets say "type `bash` first", and macOS `/bin/bash` is 3.2.
- The step-by-step mode ("go one step at a time … allow me to review the output, and then tell you what to do next") ran on an agent PTY: `start('exec "$SHELL" -i')`, `send()` per block, `wait(pattern=<prompt>)`, OSC/CSI escapes and pulumi spinner frames hand-stripped with regexes, output scanned by hand for `AKIA…`, `eyJ…`, `aws_session_token` before showing, a scratch file plus `show`, a comparison with "You should see", and the agent typing `yes` into pulumi's arrow-key prompt. When Sky typed himself, theuth only saw the screen through the ~1 min idle wake.
- The field agrees on the failure class: Cursor's Shell Mode lists the same traps — `$SHELL`, 30 s timeout, no prompts, no `cd` persistence (`https://cursor.com/docs/cli/shell-mode`); a human approval did not stop the 2026 `terraform destroy` because the consequence was not on screen (`https://botmonster.com/ai/terraform-incident-claude-code-wiped-production-database/`).

**What changes for Sky.** He says "prep the preview and up, don't apply" and the agent's reply holds three cells: export the state, preview, apply. Each shows its folder, its shell and what it will do; the apply button reads "Apply to ir-prod1", stays disarmed until the preview succeeded with the expected "3 to update", and then shows that preview's counts on itself. He presses; the output streams into the cell in a real terminal, he answers pulumi's prompt in place, and when the run ends the agent has a clean, capped, credential-masked copy and checks it against the expectation. Long or repeatable procedures land in a note as the same cells, so a run sheet *runs* and records who ran what, when, with which exit — instead of being ticked by hand.

**What this is NOT.** Not a tool: no registry entry, no `tools.policy` row, no approval card, no double involvement, no MCP exposure. Not agent execution: the agent presents, edits and reads; it never runs, types into a kernel, or ticks a run sheet. Not a coding feature: no editor integration, no test runner. Nothing in tools, MCP, policy or approvals changes (§17.4 must-not-regress).

## 2. Facts relied on

| # | fact | where | used in |
|---|---|---|---|
| 1 | Custom chat fences are streamdown `plugins.renderers`, matched by language before the built-in code block | `crates/desktop/ui/src/components/chat/MessageAssistant.tsx:31-39` | §4.1 |
| 2 | `FenceContext{running, hasLaterAssistant, session?, entry?}` | `crates/desktop/ui/src/components/chat/surface/JsonRenderFence.tsx:15` | §3.4, §4.1 |
| 3 | `session` is undefined in read-only renderings | `crates/desktop/ui/src/components/chat/MessageAssistant.tsx:298-301` | §4.1, §11.2 |
| 4 | streamdown 2.6.0 renderers get `{code, isIncomplete, language, meta?}` | `~/Develop/theuth/crates/desktop/ui/node_modules/streamdown/dist/index.d.ts:304-316` | §0, §3.1 |
| 5 | The engine keeps the whole trimmed info string as `lang` and compares by equality | `crates/surface/src/fences.rs:13`, `:72`, `:94` | §3.1 |
| 6 | A note's `codeBlock` has one prop, `language` | `packages/documents/src/schema.ts:73` | §0, §4.2 |
| 7 | Only `render` of the code block is ours; node and Yjs fragment are the default's | `crates/desktop/ui/src/components/editor/CodeBlock.tsx:16-18` | §4.2 |
| 8 | Markdown import keeps the fence info string verbatim in `language` | `crates/desktop/ui/src/components/editor/codeLanguages.ts:10` | §3.1 |
| 9 | Only knowledge documents carry a frontmatter block | `docs/design/59-documents.md:485`; `packages/documents/src/schema.ts:4` | §3.3 |
| 10 | One Yjs update ≤ 48 KiB; a document ≤ 8 MiB | `crates/document/src/actor.rs:947`, `:357` | §4.2, §13 |
| 11 | `doc_read` outlines a code block as `[code:<language>] …` | `packages/documents/src/convert.ts:425` | §9.5 |
| 12 | Member blocks are labelled `doc-member:<pub8>` | `crates/document/src/tools.rs:34`; `crates/document/src/share.rs:262` | §13.1 |
| 13 | Guests (mode 2) may call 11 methods only | `crates/link/src/conn.rs:538` | §12, §13 |
| 14 | Replicated tables beside `docs`: `["docs","notebooks","notebook_items","sources"]`, attached by `replica::attach` | `crates/document/src/store.rs:65`; `crates/document/src/lib.rs:238` | §13.2 |
| 15 | `@Run` mints `UserActivation` and goes through the registry | `crates/tools/src/registry.rs:183` | §12 |
| 16 | Admission check `authorize(ext, session)` | `crates/rpc/src/api.rs:2847` | §7, §15.2 |
| 17 | `send_input` asks `forward::route` before acting | `crates/rpc/src/api.rs:3589`; `crates/rpc/src/forward.rs:1` | §5.1, §15.2 |
| 18 | `rpc::client::Local` makes every call as the owner console (`RpcPeer::internal()`); the MCP server reaches the engine through it | `crates/rpc/src/client.rs:24-31`; `crates/link/src/types.rs:420-423`; `crates/mcp-server/src/lib.rs:6-8` | §12.1 |
| 19 | Guests are distinguishable: `RpcPeer::guest()` | `crates/link/src/types.rs:438-441` | §12.1 |
| 20 | `StartPty{session, call_id, args, cwd, …}` | `crates/tools/src/pty.rs:987` | §5.2 |
| 21 | PTY jobs run `$SHELL -lc <command>` on Unix, pwsh/powershell on Windows | `crates/platform/src/shell.rs:45-49` | §5.2, §6.4 |
| 22 | PTY budget 512 MiB ≈ 80 live PTYs engine-wide | `crates/tools/src/jobs.rs:59-62` | §5.10 |
| 23 | Every pipe/PTY chunk passes `Log::push`, which runs `secrets::redact` before log, tail and event | `crates/tools/src/jobs.rs:592-598` | §8.3, §12 |
| 24 | `JobOutput` is coalesced to one per 500 ms and lossy | `crates/tools/src/jobs.rs:50` | §8.1 |
| 25 | PTY screen is `vt100` 0.18 with 512 history rows; `portable-pty` 0.9 | `Cargo.toml:269-270`; `crates/tools/src/pty.rs:71`, `:1436` | §5.5, §8.3 |
| 26 | `vt100` hands unknown OSC sequences to `Callbacks::unhandled_osc` | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/vt100-0.18.1/src/callbacks.rs:69` | §5.5 |
| 27 | The engine answers terminal queries; the viewer swallows them (167) | `crates/tools/src/pty.rs:2460`; `crates/desktop/ui/src/components/chat/TerminalViewer.tsx:170` | §5.10, §11 |
| 28 | Two viewers fight over PTY size; exactly one sizes it | `crates/desktop/ui/src/components/chat/TerminalViewer.tsx:32` | §5.10 |
| 29 | The agent's `shell` is an embedded brush shell per session; state last-finished-wins | `crates/tools/src/shell.rs:1`, `:45` | §5.2, §6.5 |
| 30 | Login shell = `$SHELL`; only `PATH` is probed from it today | `crates/tools/src/shell.rs:205`; `crates/platform/src/env.rs:2`, `:12` | §5.4 |
| 31 | `<job_result>` wrapper; the only model-delivery path for jobs | `crates/agent/src/supervisor.rs:3093`, `:2821` | §9.1 |
| 32 | `inject` admits an `Automatic` input; 145 parks inputs for talk sessions | `crates/agent/src/supervisor.rs:1504`, `:1540-1556` | §9.3 |
| 33 | `HarnessDeliver{session, kind, text, wrapped}` is a public message to `Agents` | `crates/agent/src/supervisor.rs:2726-2742` | §9.3, §15 |
| 34 | `<surface_error>` is built in surface and rendered hidden on the desktop | `crates/surface/src/prompt.rs:330`; `crates/desktop/ui/src/lib/injected.ts:54-58` | §9.2 |
| 35 | `check_fences` runs after each reply; `CHECKED_LANGS = ["mermaid","openui"]` | `crates/agent/src/loop_.rs:5904`, `:3037` | §9.2 |
| 36 | Prompt blocks: unattended sessions drop `app`; attended = depth 0 and not scheduler-opened | `crates/agent/src/loop_.rs:3207-3221`, `:4240-4241` | §10.6 |
| 37 | The sentence to amend lives in the surface block and in the visualize skill | `crates/surface/src/prompt.rs:289`; `crates/surface/visualize.md:42` | §10.4 |
| 38 | Binary-owned skills are rewritten by `first_run` | `crates/theuth/src/main.rs:1324-1336` | §10.2 |
| 39 | Tool-less Monty profile (sensors), own pool | `crates/agent/src/sensor/bridge.rs:1-6`; `crates/agent/src/sensor/mod.rs:184` | §5.9 |
| 40 | Monty: 512 MiB heap, 60 s per request, type check before run, `/cwd` read-only, print buffered per call | `crates/agent/src/repl.rs:42`, `:53`, `:1027`, `:1104`, `:1274` | §5.9, §6.2 |
| 41 | `find_cpython` (engine PATH only) is the one interpreter lookup | `crates/agent/src/sensor/mod.rs:701` | §6.1 |
| 42 | Vault hooks `mentions`/`expand`/`redact` are process-global; placeholder text `{{secret:` | `crates/tools/src/secrets.rs:54-72` | §12 |
| 43 | Jobs die with the engine; no adoption; keeper not chosen | `docs/design/109-durability.md:28`, `:42`, `:46` | §5.4, §18 F6 |
| 44 | nteract daemon: guarded execute; record-then-read; server-side stream terminal; env redaction before storage | `nteract:crates/notebook-protocol/src/protocol.rs:738-745`; `nteract:docs/adr/execution-pipeline.md:30-44`; `nteract:crates/runtimed/src/stream_terminal.rs:1-7`; `nteract:crates/runtimed/src/output_redaction.rs:1-5` | §5, §8 |
| 45 | Approvals miss 1 in 3 threats; consequence-named buttons | `https://scalex.dev/blog/ai-agent-permissions-stats/`; `https://developer.hashicorp.com/terraform/cloud-docs/run/manage` | §12 |
