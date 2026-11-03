# 171 CELLS — shared recon axes (2026-11-03)

Design 171 (claimed 2026-11-03 17:03, docs only): **user-run code cells** — bash / python (local interpreter or theuth's built-in Monty) / maybe JavaScript — in three places: (1) chat replies, where the agent *presents* cells it was told to prepare but not run and the user presses Run (whole cell, one line, a selection, run-all); (2) notes; (3) notebooks (design 144). Output streams live to the user; the agent sees it in context when the run completes. This is NOT a tool and must not change the tool/MCP/approval machinery.

## The user's ask, verbatim (Sky, 2026-11-03 16:58)

```
https://github.com/nteract/elements
https://nteract-elements.vercel.app/docs
https://github.com/nteract/nteract
https://uiwjs.github.io/react-codemirror/
Practical recommendation for your setup
Start with nteract/elements for the cell chrome, toolbars, and output area (Shadcn-native).
Use @uiw/react-codemirror (or Monaco) for the editor itself if you need finer control — style it with your existing Shadcn theme.
Implement the interaction layer yourself:
Run cell → send whole cell source over RPC to Rust
Run selection / current line → extract the relevant code and send that
Run in order → sequential RPC calls with shared kernel/session state on the backend

Feed the results (stdout, stderr, rich MIME data, images, etc.) into the output renderers from nteract/elements or @runtimed/components.
This gives you a Jupyter-like experience (cells, sequential execution, rich outputs) while keeping full control of the Rust backend and RPC protocol. You avoid having to speak the full Jupyter kernel protocol unless you want to.

So I want you to write a design document for a new feature and enhancement to theuth.  right now we have the notebooks, but I don't think they have any code execution capability or anything like that.  So I want to add that as part of the notes and notebooks using these UI elements properly styled to match theuths theme.   In addition to that, I want an agent to be able to present cells to me with a specific cwd and stuff running either bash or python, and the ability for the agent to select a local python interpreter if one exists or use the built in monty if it's sufficient for the use case.  The idea being that the agent can present a shell command to me in a chat that I explicitly told it to prepare but not run, and then I can click a button to run a single line or the entire script and both I and the agent can see the output.  I can watch it live in the chat, and the agent can see it in context once it completes.

So it's give the agent a way to allow me to press a button to execute bash or python (separate from how we execute tools/mcps now, this is a user facing thing, not replacing or changing the tools/mcp setup in any way)

and allow notes and notebooks to have the same kind of jupyter notebooks setup but suppport both bash and python, maybe even javascript too.

Let's think through this and figure out how we can align the feature with the design principles of theuth and achieve the goal of the feature while reducing the users cognitive load and making work more efficient for the user.

we also need to think through the prompting to explain to the agent when to use it.  For example I often when working with infrastructure as code will tell it, you know prep all the code but don't apply it because production danger, just present me with the commands to run.  so when I request something like this, the agent should understand that means, show me these cells in chat or in the notebook, so I can actually execute the commands without leaving the agent window.
```

## User evidence already on file (from theuth knowledge, 2026-10-15..17 DR runs)

- Sky's IaC/DR practice today: desks write **run sheets** as theuth notes: one checkbox heading per step, each action = optional one-line label + one ```bash block + `You should see: <exact expected output>`, stop lines ("If it doesn't say exit 0, stop and tell theuth"), commands end with `; echo "exit $?"` to surface exit codes, explicit `--profile/--context` on every line. **Agents tick nothing; Sky ticks as he runs.** Desks never run `pulumi up` themselves ("no Pulumi without a human").
- Sky's login shell is **zsh**; bash-written blocks break when pasted into interactive zsh (`${PIPESTATUS[0]}`, trailing `# comment` without `interactive_comments`), so sheets say "run in bash: type `bash` first". macOS /bin/bash is 3.2.
- Live runs (2026-10-16/17): Sky: "you're going to create a PTY, and you will go one step at a time, execute one of the commands in order, allow me to review the output, and then tell you what to do next." / "stop all of your existing PTYs and create one new one, we will drive it collaboratively." The agent then: `start('exec "$SHELL" -i')`, `send()` each block, `wait(pattern=<prompt host>)`, hand-stripped OSC/CSI escapes and pulumi spinner frames with regexes, scanned output for credentials (AKIA…, eyJ…, aws_session_token) before showing, wrote cleaned text to scratch + `show`, compared with "You should see", and answered pulumi's arrow-key prompt ("Do you want to perform this update? [Use arrows…] yes / > no / details") by typing `yes`. When Sky typed himself, theuth only saw the screen via the ~1 min idle wake.

## Axes (answer every axis that applies to your tree; cite path:line or URL)

1. **Cell model** — what a cell is (source, language, options/metadata, outputs, execution count, ids); how metadata is written (nbformat JSON, Quarto `#|` option comments, Runme fence attributes, BlockNote code-block props).
2. **Kernel / runtime model** — who owns the process, persistent state across cells/lines, lifecycle (start, interrupt, restart, idle timeout, shutdown), execution queue, one kernel per what (document? session? language?); Jupyter protocol vs a native protocol.
3. **Languages & interpreter discovery** — bash/zsh/sh/pwsh; python (`.venv`, uv, pyenv, conda, system), Monty (theuth's embedded sandboxed Python subset: what it can and cannot do, isolation, type-check-without-running), JavaScript (node/deno/bun, `deno jupyter`); how others find and present interpreters.
4. **Output model & rendering** — stream / display_data / execute_result / error, MIME bundles, ANSI and `\r` progress handling, images, HTML sandboxing, tables/DataFrames, very long output, terminal (PTY) fidelity vs line streams, interactive prompts / stdin.
5. **Editor** — CodeMirror 6 via @uiw/react-codemirror vs Monaco; run-line / run-selection keymaps and gutter buttons; theming with shadcn CSS variables; read-only vs editable cells.
6. **Notes & notebooks integration** — block-document storage (Yjs/yrs), markdown round trip, where options and outputs persist, replication size limits, sharing roles and who may run, edits by untrusted members, export/import (.ipynb), search.
7. **Chat integration** — how an assistant message renders fenced blocks, whether the fence info string reaches a renderer, message/cell identity, how a button press in a message reaches the engine, how a result flows back into the agent's context (and whether it starts a turn), versioning/superseding.
8. **Agent interface & prompting** — fence grammar vs a tool call, how the agent learns when to use it, how it picks an interpreter, what exactly it sees after a run (size caps, redaction), how it reads a running cell.
9. **Safety & trust** — execution only on a human press (never the agent, never the owner's double), separation from tools.policy, secrets placeholders + scrubbing, credential redaction in what the agent sees, danger labels, run-all boundaries, audit, which machine runs it (mesh engines, sandbox VMs), Windows.
10. **Live streaming, multi-client, durability** — event delivery to desktop/web/phone/TUI, reconnect, engine restart mid-run, two clients pressing Run.
11. **Surfaces & channels** — desktop (Tauri macOS/Windows/Linux), web, iPad/iPhone, Windows companion, TUI, Slack/Signal/Telegram.
12. **Cognitive load** — what must be visible at a glance (state, cwd, interpreter, host, danger, expected output), defaults, one press per action, how output is summarised.
13. **Prior art for "agent prepares, human runs"** — Cursor / Copilot / Windsurf run buttons, Warp, Atuin Desktop, Runme, Zed REPL, marimo, Jupyter AI, nteract desktop + MCP, Livebook, Observable, ChatGPT/Claude; complaints and praise.
14. **Licences, dependency weight, adopt vs build.**

## Map to the questions the design must answer
- Q1 What is the one grammar for a cell across chat, notes, notebooks and files?
- Q2 What is the kernel model and where does it live in the engine?
- Q3 How does a press become a run, and how does the output reach the user live and the agent afterwards?
- Q4 How does the agent know when to present cells instead of running tools, and which interpreter to pick?
- Q5 What keeps it safe (human-only execution, secrets, redaction, danger) without adding confirmations?
- Q6 What do we adopt (nteract elements, CodeMirror, runtimed crates) and what do we build?
