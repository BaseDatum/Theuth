# R3 FIELD: prior art and user evidence for "the agent prepares, the human runs" (survey to 2026-11-03, design 171 CELLS)

**Method and limits.** I used `read_url`, `scrape`-style fetches and `search_web`. Search providers were rate-limited for part of the run (brave, ddg, google and qwant all failed). I fell back to direct page reads, the GitHub search API, and Hacker News pages. Vote and reaction counts are shown only where a page showed them. Where I looked and found nothing, I say "not found". I read about 40 URLs. Cites are inline, numbered by the AXES.md axes (1–14). Axes with no R3 evidence are marked "no R3 evidence", not guessed. The report is shorter than the 350–600 line target because I ran out of budget.

## Section map
- Axes 1–4 (cell model, kernels, languages, output): §A, executable runbook tools
- Axes 5–6 (editor, notes): §A and §E
- Axes 7–8 and 13 (chat integration, agent interface, prior art): §B, harnesses
- Axes 9 and 12 (safety, cognitive load): §C, §D, §F
- Axis 14 (licences): §G
- Final: §H borrow/avoid, §I ten facts

---

## §A. Axes 1–6, 13: executable runbooks and notebooks

### A1. Runme (runme.dev, runmedev/runme)

| Item | Finding | Source |
|---|---|---|
| Cell model | A Markdown fence plus a JSON attribute block after the info string. Example: ` ```sh {"cwd":"/tmp","id":"01HPQ8F2…"} `. A ULID `id` is auto-written. | https://docs.runme.dev/configuration/cell-level/ |
| Cell option keys | `background`, `closeTerminalOnSuccess`, `cwd`, `excludeFromRunAll`, `interactive`, `interpreter`, `mimeType`, `name`, `ignore`, `transform`, `terminalRows`, `promptEnv`, `skipPrompts`, and the deprecated `tag`/`category`. | same |
| `interactive` | The docs prose says `interactive=false` "defaults to *true*". The keys table says the default is `false`. The page contradicts itself. | same |
| Doc-level options | YAML front-matter: `cwd`, `shell` (bash, zsh or an absolute path), `skipPrompts`, `terminalRows`. | https://docs.runme.dev/configuration/document-level/ |
| Cwd rules | Default cwd is the Markdown file's directory. Doc cwd and cell cwd combine. The docs example says `cwd: /tmp/dummy` (doc) plus `cwd: ..` (cell) gives `/tmp`. | cell-level and document-level pages |
| Shell | The `shell` option exists because notebooks are shared among users with different shells. | document-level page |
| Run affordances | A Run icon left of each block. "Configure" opens a general/advanced options view. Run All honours `excludeFromRunAll`. | cell-level page |
| State between cells | The kernel is "like Jupyter but with a Shell/Bash Kernel, shared environment sessions" (https://runme.dev/). Env vars persist across cells, "much like a terminal session". Output can be piped into later cells (the search snippet of https://github.com/runmedev/runme; the README fetch showed only the licence). | runme.dev, GitHub README |
| Output | Interactive cells use a terminal. Non-interactive cells put output inline and make it available downstream. | cell-level page |
| Licence and traction | Apache-2.0 for the repo (the runme.dev page text says "APL-2.0", read as a typo). About 2.2k stars. The runme.dev page counts 163.5k, which is unexplained and not trusted. | https://github.com/runmedev/runme |

Takeaway for 171: Runme is the closest match to "cell metadata lives in the fence info string". Its per-cell `cwd`, `interpreter`, `interactive` and `excludeFromRunAll` fit what Sky wants. Its main hazard is that `{"id":"01HP…"}` gets written back into the Markdown, which is noisy for human-authored notes.

### A2. Atuin Desktop (atuinsh/desktop)

- It launched as "a local-first, executable runbook editor for real terminal workflows". Features: script blocks, embedded terminals, DB clients, Prometheus charts, Jinja-style templating, CRDT sync, Atuin Hub for sharing (https://atuin.sh/blog/atuin-desktop-runbooks-that-run, 22 Apr 2025).
- **Status: the repo `atuinsh/desktop` says "archived by the owner on Sep 30, 2026. It is now read-only."** It had 2.4k stars, 1,290 commits and an Apache-2.0 licence (https://github.com/atuinsh/desktop).
- Why it was archived is not on that page. I did not find a statement of the reason. Atuin's blog index now promotes "Atuin AI" and "Atuin Threads" (agent sessions across machines) instead.
- Takeaway: a standalone runbook-editor product did not survive as a product. Treat it as a UX reference, not a dependency. Do not claim "reception" beyond the archive fact.

### A3. Warp Drive Notebooks

- Notebooks are runnable documents in Warp Drive: Markdown plus executable shell command blocks, searchable by the Command Palette and exportable as `.md`.
- Running a command block: the insert button at the block's bottom, or ⌘+Enter on the selected block. The command text is inserted into the user's active terminal session, or a new session if none is open. Code blocks get a copy button only.
- So Warp's "run" is a paste into a real terminal. The user's shell, cwd, environment and scrollback apply. Output is the terminal itself. Source: https://docs.warp.dev/knowledge-and-collaboration/warp-drive/notebooks
- Warp agent permissions: profiles with Agent Decides / Always ask / Always allow / Never, plus a command allowlist (regexes, empty by default, users typically add read-only commands) and a denylist. Source: https://docs.warp.dev/agents/capabilities/agent-profiles-permissions/

### A4. Quarto cell options

- Options are `#|` comment lines at the top of a code cell. Example keys: `label`, `echo`, `eval`, `output`, `fig-cap`, `tags`, `id`. `id` falls back to `label`.
- `eval: false` "just echos the code into the output". That is the "present, don't run" switch at doc level.
- Options can be set in document YAML and overridden per cell. Sources: https://quarto.org/docs/reference/cells/cells-jupyter.html and https://quarto.org/docs/computations/execution-options.html
- Takeaway: `#|` is language-native and survives copy/paste into a real file. It is the opposite of Runme's JSON-in-fence, and only works for languages with `#` comments (bash and python both do; JavaScript would need `//|`).

### A5. marimo

- Reactive execution model: run a cell and every cell referencing its variables re-runs. It is static analysis without running. This can be configured to **lazy** (mark stale instead of running), which matters for side-effecting cells. Source: https://docs.marimo.io/guides/reactivity/
- Sandbox mode: `marimo edit --sandbox notebook.py` builds an isolated uv (or Pixi) environment from the notebook's inline requirements. The marimo docs say plainly: "Sandboxes isolate installed packages, not access to your files or network. Only run notebooks from sources you trust." Source: https://docs.marimo.io/guides/package_management/sandboxes/
- Shell commands via `!` were an open feature request, issue #3956 "Invoking shell commands using ! syntax", with 3 reactions. Source: GitHub search API, `repo:marimo-team/marimo bash cell` (item https://github.com/marimo-team/marimo/issues/3956).
- Issue #8177 "MCP server: agent experience report — friction points from real usage" has 4 reactions. I did not read its body, so I make no claims about its content.
- marimo's AI assistant has Ask mode plus an Agent mode that can edit cells and run **stale** cells, and a Code mode with kernel access. Source: https://docs.marimo.io/guides/editor_features/ai_completion/
- Takeaway: reactivity is wrong for side-effecting shell/IaC cells, but "stale" as a cell state is a good idea.

### A6. Zed REPL

- Built on Jupyter kernels. `repl: run` (ctrl-shift-enter on macOS) runs a block, selection or line. Results appear below the selection. Supported kernels: Python (ipykernel), TypeScript (Deno), R, Julia, Scala.
- Cell mode uses `# %%` separators (jupytext-style) in Python and `// %%` in TypeScript.
- On macOS system Python won't work; users need pyenv or a venv with `ipykernel installed`. Zed keeps **separate** selectors for the Python toolchain and for the REPL kernel, and warns that diagnostics can differ from the REPL until you align them with `toolchain: select`.
- Source: https://zed.dev/docs/repl. There is no bash kernel in that list.
- User evidence: Zed issue #59907 "Remote REPL + Remote Devcontainers" has 35 reactions (open). Source: GitHub search `repo:zed-industries/zed repl`.
- Takeaway: "run line / run selection / run block with a gutter-less toolbar button" is proven. The kernel-picker mismatch with the language toolchain is a known trap.

### A7. nteract desktop and elements (2025–2026 reboot)

- `nteract/nteract` is now the desktop app: "a local-first notebook environment where humans, kernels, and AI agents can work with the same live document." It is Tauri + React, built on `jupyter-zmq-client` and `jupyter-protocol` crates. Source: https://raw.githubusercontent.com/nteract/nteract/main/README.md
- Runtime model (same README): notebook content lives in an **Automerge** document. Live kernel state lives in a separate "runtime state document". The `runtimed` daemon owns kernel processes, execution queues, output capture and "the write path for execution results".
- "Execution requests name a synced `cell_id`; the daemon reads the cell source from the shared document." Desktop, CLI, MCP server and agent clients all use that path.
- MCP: `nteract mcp` lets "agents run code, read and write cells, manage dependencies". Codex plugin install is `codex plugin marketplace add nteract/agent-plugins`. The README documents a version-skew failure: an old plugin rejects a newer app with `unsupported_notebook_target` (plugin 0.6.6 + desktop 2.8.6 is the verified pair).
- Important contrast with 171: nteract lets the agent **run** cells through MCP. 171's agent presents and the human presses Run. nteract has no human-only gate in the text I read.
- **nteract/elements** (the user's named library): React components in the shadcn registry. It has 15 cell components, 9 output renderers (ANSI, HTML, image, JSON, SVG, Markdown, media-router), 50+ widgets, and a CodeMirror editor with Jupyter keybindings. Install is `shadcn add @nteract/media-router`. The site states "**Work in Progress** — bringing the nteract components out of hibernation". It says it "powers runt", a Tauri desktop notebook editor. Sources: https://nteract-elements.vercel.app/docs and https://github.com/nteract/elements
- Elements licence: I did not find it on either page. Not found (the GitHub page listed only folders in the part I read). Needs checking by the licence recon child.

### A8. Livebook

- Each notebook gets its own Erlang VM node by default (the standalone runtime). It can declare its own dependencies with `Mix.install/2`. Other runtimes: attached node, Fly.io, Kubernetes pod. Source: https://livebook.hexdocs.pm/runtime.html
- Community "smart cell" for shell commands: `kentaro/smart_cell_command`, MIT, 11 stars. Evidence that shell is not first-class in Livebook. Source: https://github.com/kentaro/smart_cell_command
- Livebook issue #2547 "Race condition on smart cell evaluation" (title seen only in search results). I did not read the body.
- Not found: Livebook's own treatment of human-presses-run vs agent-run.

### A9. Jupyter AI (v3.x)

- A permission system "gives you guardrails over agent actions — agents request approval before writing files or executing commands." Source: https://github.com/jupyterlab/jupyter-ai
- v3.1.0 (23 July 2026) added in-chat controls: persona picker, a permission-mode switch, effort, live usage. It also added **server-side notebook execution**: "Cells can now run on the server via a REST API with outputs streaming to every connected client." Source: https://jupyter-ai.readthedocs.io/en/stable/releases/v3.1.0.html
- That release also fixed "data loss resulting from rare race conditions after opening a file while another user was connecting to it" (same page). That is a concrete warning for any multi-client live document.
- Not found: a cell-level "agent presents, human runs" button in Jupyter AI.

### A10. Not covered or only partly covered
- Observable, Fiberplane notebooks, Rundeck and the Netflix Jupyter-as-runbook practice: I did not fetch pages for these. Not found / not read.
- Open Interpreter: the page I fetched (https://docs.openinterpreter.com/safety/safe-mode, 2.3k chars) now describes a different product, a terminal coding agent that "runs commands with explicit guardrails. A sandbox decides what is even possible, and an approval mode decides when to ask first." The old `auto_run`/confirm-each-block behaviour was not confirmed in that page.
- Goose: https://block.github.io/goose/docs/guides/goose-permissions returned 404, and the goose-docs.ai mirror returned only 536 chars. Not found.

---

## §B. Axes 7, 8, 13: agent harnesses and what "Run" means

| Harness | Human approval surface | Where it runs, and what the model sees | Source |
|---|---|---|---|
| Claude Code | Modes: `default` (reads only without asking), `acceptEdits`, `plan`, `auto` (a second-model classifier reviews actions), `dontAsk`, `bypassPermissions` ("isolated containers and VMs only"). From v2.1.283 `auto` is the starting mode for interactive terminal and VS Code. Docs describe auto as for "reducing prompt fatigue". | The agent runs the command. Not a human Run. | https://code.claude.com/docs/en/permission-modes |
| Codex CLI | An OS-enforced sandbox plus an approval policy. Network is off by default. `approval_policy = "untrusted"` is **retired**. Recommended interactive form is `sandbox_mode = "read-only"` plus `approval_policy = "on-request"`. | Agent runs. | https://developers.openai.com/codex/agent-approvals-security |
| Cursor (IDE) | Run Modes. Auto-review is a classifier; a Run Everything mode exists. Docs: "Auto-review is not a security boundary." The classifier is a small managed model and can read files read-only. | Agent runs. | https://cursor.com/docs/agent/security/run-modes |
| Cursor CLI "Shell Mode" | **User-typed** shell commands inside the agent conversation. They run in `$SHELL` with the CLI's cwd and env, with a 30 s timeout, no interactive prompts, no long-running processes. `cd` does not persist between commands. Large output is truncated, `Ctrl+O` expands. It still checks permissions and team policy. | Output is shown in the conversation. | https://cursor.com/docs/cli/shell-mode |
| VS Code agents | Permission levels: default (tool/URL/terminal approval settings), Assisted (an LLM judge), Allow all. Two layers: approvals and sandboxing of agent terminal commands. Terminal confirmation is read-only: "If a call needs different parameters, reject it and tell the agent what to change." | Agent runs. | https://code.visualstudio.com/docs/agents/run/approvals and https://code.visualstudio.com/docs/copilot/agents/agent-tools |
| Windsurf / Devin Local | Allow / Ask / Deny rules, with Deny highest priority. Modes: Code (asks), Smart (a fast model auto-approves what it judges safe), Bypass. The command card offers once, session, or permanent allow. | Agent runs. | https://docs.windsurf.com/windsurf/terminal |
| Warp Agent | Profiles per action type (see §A3). | Agent runs, or Notebook pastes into the user's terminal. | Warp docs, §A3 |
| nteract MCP | Agent calls run on a shared document. | Daemon runs; agent sees outputs through the shared doc. | nteract README |
| Jupyter AI | Permission requests before writing files or executing. | Server or kernel. | §A9 |

Findings:
1. Every mainstream harness frames the choice as the **agent runs, the human approves** (a gate on the tool call). None of the pages I read offers an agent-prepared, human-pressed Run button as the core pattern. The closest are Warp Notebooks (user runs a saved block, inserting it into their own terminal) and Cursor CLI Shell Mode (the user runs, and the output joins the conversation).
2. Claude Code's own docs concede that approvals cause fatigue, and the default has moved to a classifier. That is the trend 171 is going against, on purpose, for a narrow case (IaC/prod).
3. Shell Mode's limits (30 s timeout, no interactivity, `cd` does not persist, login shell) are the same traps Sky already hit with PTYs and zsh/bash. Compare this with the evidence in AXES.md.
4. **Copilot "run in terminal" on chat code blocks**: I did not get a fetchable page that confirms the current behaviour. The VS Code docs I read describe agent tool approval instead. Not found.
5. **ChatGPT and Claude code-execution UIs**: not read.

---

## §C. Axis 9, 12: user evidence

### C1. Direct evidence for "show me, don't run"
- **Fewshell** (Show HN, 12 points, 30 Apr 2026): "a terminal agent specifically designed to avoid this. There is no setting to enable command auto-approval. This is by-design, so that the user never has to second-guess or worry about accidentally having it enabled." Posted "in light of recent news about an agent deleting a production database". Source: https://news.ycombinator.com/item?id=47957127
- Reactions in that thread:
  - "Numerous prompting will cause prompt fatigue, similar to pressing yes on a dialog boxes."
  - "if I had to approve each command of the agent, that'd remove 90% of the benefits of using an agent in the first place."
  - "it's a fair point sandboxing is necessary. But not sufficient for agents that legitimately need write access. A payment agent needs Stripe. A maintenance agent needs DB writes. You can't sandbox those away. The missing layer is between has access and executes."
- Reading: this is a split audience. A "never auto-approve" product gets 12 points. People who want autonomy reject per-command approval as pointless. 171 fits the second claim only if it is opt-in per request ("prep, don't apply") and costs one press, not one prompt per command.

### C2. Incident evidence: terraform destroy
- **DataTalks.Club, 26 Feb 2026.** Claude Code ran `terraform destroy` against a stale/old state file. It wiped the production RDS (1,943,200 rows), VPC, ECS cluster, load balancers and the automated snapshots. AWS restored from a hidden snapshot about 24 hours later. Source: https://botmonster.com/ai/terraform-incident-claude-code-wiped-production-database/ (secondary, dated 2026-07-31).
- Key detail for 171: "Claude Code then proposed running `terraform destroy` to clean up the duplicates. Grigorev approved, thinking only the new duplicates would go." The same page names "Unchecked execution" as failure 4. So the human approval existed and did not catch it. The agent had also swapped in the old state file without his noticing.
- Hacker News thread, 145 points: https://news.ycombinator.com/item?id=47278720. Quotes:
  - "As the tool gets better, people trust it more… The 'almost' part of automation is the issue."
  - Counter-view: the user "rejected multiple Claude's warnings… and suggestions to act in safer ways."
  - "Guardrails for both AI and humans are important."
- Lesson for cells: a human-press gate does not fix a human who misreads the consequence. The cell must show the **consequence**, such as the `terraform plan` summary counts, before the Run press, not only the command text.

### C3. Complaints about config and environment
- Cursor forum bug report "Command Allowlist is silently ignored when 'Auto-Run in Sandbox' is enabled", dated 2026-02-17. The setting text says tools "will respect the allowlist or ask", but in sandbox mode they don't, and the report's workaround was the Legacy Terminal Tool. I could read only the page header text; the body was an empty shell. Source: https://forum.cursor.com/t/command-allowlist-is-silently-ignored-when-auto-run-in-sandbox-is-enabled/152136
- Takeaway: layered permission systems become confusing when two of them interact. 171 should keep one rule, human-press-only, not an allowlist.
- GitHub search for "zsh/bash/cwd shell" in anthropics/claude-code and "terraform destroy without permission" returned nothing usable or only unrelated high-reaction issues (e.g. #6235 AGENTS.md support, 6,690 reactions, unrelated to shells). **Not found:** high-reaction issues specifically about wrong shell/cwd/env, or run buttons on code blocks. The queries hit rate limits (HTTP 403) on two repositories (microsoft/vscode, openai/codex). I could not search those two.
- Not found: any popular GitHub issue literally asking "don't run, just show me the commands". Sky's own request is the main evidence, from AXES.md (run sheets, "Agents tick nothing").

### C4. Sky's evidence, restated from AXES.md (not re-verified by me)
Zsh login shell, bash 3.2 on macOS, PTY hand-stripping of escapes, and credential scanning before showing output. These match the Cursor Shell Mode limits above (login shell, 30 s, no interactivity), which is independent confirmation of the failure class.

---

## §D. Axis 4: infrastructure plan → review → apply

| System | Gate | How the human approves | Source |
|---|---|---|---|
| Atlantis | `atlantis plan` runs on the PR branch and comments the output. `atlantis apply` is a PR comment, with optional `-d dir`, `-w workspace` and project flags. Apply requirements (approvals) are configurable. Site claims: "Developers can safely submit Terraform pull requests without credentials. Require approvals for applies." | Reviewer reads the plan comment in the PR, then a comment triggers apply. | https://www.runatlantis.io/ and https://www.runatlantis.io/docs/using-atlantis.html |
| HCP Terraform | Run states with buttons: Confirm & Apply, Override & Continue (soft-mandatory policy failed), Discard Run, Cancel Run, Force Cancel Run. The run page shows a timeline plus plan and apply output. | The buttons are verb-labelled by consequence ("Confirm & Apply", "Discard Run"). | https://developer.hashicorp.com/terraform/cloud-docs/run/manage |
| Spacelift | Without an approval policy, "anyone with write access to a stack can create a run". Approval policies review runs when queued, unconfirmed or pending review. With autodeploy on, plan policy can warn and force an unconfirmed state. | Human review of a run with an Undecided policy decision. | https://docs.spacelift.io/concepts/policy/approval-policy |
| Pulumi | `pulumi up` shows a preview and an arrow-key prompt (yes / no / details). The Pulumi docs URL I tried returned "not found". The prompt behaviour is Sky's observation in AXES.md. | Interactive prompt in a terminal. | AXES.md |

What this shows:
- Plan and apply are **separate artifacts with a state between them**. The plan is reviewed where reviewers look: a PR comment or a run page. The approval control is a labelled button or a typed command, not "OK".
- None of the pages I read quantifies what reviewers actually check. I found no study. Not found.
- Practical translation for 171: a cell that is `plan` and a later cell that is `apply` are two cells. The second cell's state can say "unrun, 0 of N plans seen".

---

## §E. Axes 5, 6: editor and notes

- Warp makes a code block a "Command" element by choosing it from the "/" menu or typing triple backticks. Language is picked at the bottom of the block. Source: Warp notebooks docs.
- Zed runs on selection or the line under the cursor (§A6). nteract elements ships CodeMirror with Jupyter keybindings (§A7). Both are consistent with the user's CodeMirror suggestion.
- Notebook storage at nteract: Automerge document for cells and a separate runtime-state document for execution state. Jupyter AI v3.1 hit a data-loss race when a second user joined an open file. Both are cites for keeping run state outside the document that replicates.
- Not found: any source describing Monaco vs CodeMirror performance for this use. Not researched.
- Markdown round trip: Runme keeps the notebook as plain `.md` with attributes in the info string (§A1). Warp exports `.md`. Quarto keeps `#|` in the source. Of these, Runme and Quarto are the two proven round-trip formats.

---

## §F. Axis 9, 12: safety and cognitive-load research

### F1. Approval fatigue numbers
Source: https://scalex.dev/blog/ai-agent-permissions-stats/ (a browser-game dataset, not a controlled study; 40,000+ plays and 409,000 approve/deny decisions; about 34% of commands were threats; players knew they were in a test and under time pressure).
- Average player missed **1 in 3 threats** (mean accuracy 66.3%). 7% approved every prompt. Only 35.2% caught every threat, and only 20.8% did so while blocking at most 1 in 5 of the safe commands.
- Miss rates by category:

| Category | Example | Miss rate |
|---|---|---|
| Obviously destructive | `rm -rf /` | 11.7% |
| Persistent mutation | crontab injection | 23.8% |
| Exfiltration / code execution | curl to unknown API | 33.4% |
| Scope violation | `cat ~/.aws/credentials` | 35.0% |

- `npm run analyze` (a script in `package.json` that pipes data to a remote host) was approved **64.7%** of the time even though the history log showed the payload. The three "familiar script name" threats were pooled at 52.5% missed, against 28.4% for other exfiltration-style attacks.
- Miss rates rose later in a run.
- Same dataset summarised elsewhere: reviewers miss 33% of threats, with 64% approval of the disguised script command (https://aitodaybrief.com/en/news/agents-and-mcp/human-approval-prompts-fail-in-ai-coding-agents-study-shows, page body empty, metadata only).
- Anthropic's own sandboxing post says constant approving can lead to approval fatigue "where users might not pay close attention to what they're approving" (https://www.anthropic.com/engineering/claude-code-sandboxing, quoted via https://specstory.com/learning/environments/approval-fatigue).
- Specstory's description of the mechanism: "A prompt shows the command, not what the command will do. Approving `npm test` approves whatever the project's test script runs, and the agent can edit that script. A long command joined with `&&` can hide one risky step among routine ones." (https://specstory.com/learning/environments/approval-fatigue)
- Translation for cells: a cell that shows a one-line `bash -c` wrapper hides risk, so show the real script, and flag when a cell calls a project script (`make`, `npm run`, `./deploy.sh`) that the agent also edited in the same turn.

### F2. Consequence-labelled buttons
- NN/g (Nielsen, reviewed Aug 2026): "Instead of Yes/No answers, provide response options that summarize what will happen for each possible response… use buttons labeled Delete file and Keep file." Also: a confirmation is "not a second chance at all if users automate their response… and simply click *Yes*". The title includes "If Not Overused". Source: https://www.nngroup.com/articles/confirmation-dialog/
- UX by Example: name the actual consequence ("This permanently deletes your 42 projects and can't be undone") with real verbs on the buttons. Source: https://uxbyexample.co.uk/entries/confirmation-dialog/
- HCP Terraform's buttons already do this ("Confirm & Apply", "Discard Run") (§D).
- Not found: a controlled study of verb-labelled vs generic buttons for command approval specifically.

### F3. Prompt injection when a human runs agent-written code
- "Agent data injection" attack (The Hacker News, 16 Jul 2026): it forges trusted fields across six AI models, redirecting web and coding agents while bypassing prompt injection defences. Source: https://thehackernews.com/2026/07/new-agent-data-injection-attack-can.html. I read only the header and summary text.
- A systematisation paper reviews 78 studies on prompt injection against agentic coding assistants: https://arxiv.org/html/2601.17548v1. I read only its abstract-level text.
- The agentic-design.ai page on copy-paste injection returned 89 chars. Not read.
- The scalex data (F1) shows the human reviewer is a weak defence against an injected command, and that the dangerous commands look routine.
- Takeaway: a Run button moves the decision to the human but does not remove injection. What it adds for the user is a chance to **read** the whole script. What it does not add is any check on what the script does.
- marimo's warning (§A5) that sandboxes do not isolate files or network applies equally to any local cell runner.

### F4. Interruption cost
Not found. I did not locate a source on interruption cost for approval prompts. The only adjacent evidence is the HN quote that per-command approval "would remove 90% of the benefits" (§C1), which is opinion.

### F5. Human-in-the-loop design stance
- Fewshell's stance (no auto-approve switch exists) is the only product I found that treats "human always presses" as the design, and it is 12 points, so it is weak evidence of demand (§C1).
- Cursor's docs say outright that the classifier "is not a security boundary" and "can make mistakes" (§B), so even vendors do not claim a gate is a guarantee.

---

## §G. Axis 14: licences and weight (only what I read)

| Item | Licence / status | Source |
|---|---|---|
| Runme | Apache-2.0, about 2.2k stars | https://github.com/runmedev/runme |
| Atuin Desktop | Apache-2.0, **archived 30 Sep 2026** | https://github.com/atuinsh/desktop |
| nteract (desktop and runtimed) | Not read | README raw page lists no licence |
| nteract elements | **Not found** on the pages I read; shadcn-registry components are copied into the repo, which makes licence terms matter | https://nteract-elements.vercel.app/docs |
| Livebook smart_cell_command | MIT, 11 stars | https://github.com/kentaro/smart_cell_command |
| Jupyter AI | Not read | |
| marimo | Not read | |

Adoption model shown by elements: components are copied into your repo via `shadcn add`, so there is no runtime package, but the project calls itself "Work in Progress" and "out of hibernation". Both facts bear on the adopt-vs-build call.

---

## §H. Ranked borrow / avoid list

### Borrow (ranked)
1. **Per-cell metadata in the fence info string, with a few keys:** `cwd`, `interpreter`, `interactive`, `excludeFromRunAll`, `name`. Source: Runme, https://docs.runme.dev/configuration/cell-level/. Add document-level defaults for `cwd` and `shell`, with doc and cell cwd combining. Source: https://docs.runme.dev/configuration/document-level/
2. **Verb-labelled, consequence-bearing run controls** ("Apply 3 changes", "Run bash in ~/iac"), not "Run" plus "OK". Sources: https://www.nngroup.com/articles/confirmation-dialog/ and https://developer.hashicorp.com/terraform/cloud-docs/run/manage
3. **Plan and apply as two separate cells,** with the plan visible where the reviewer looks before the apply press. Source: https://www.runatlantis.io/docs/using-atlantis.html
4. **One execution path for all callers, naming a cell id, with run state in a separate document from the content** (nteract: daemon reads source from the doc, writes outputs and queue state elsewhere). Source: https://raw.githubusercontent.com/nteract/nteract/main/README.md
5. **Run line / selection / block with a toolbar button and results shown below.** Source: https://zed.dev/docs/repl
6. **Run-All exclusion flag** so dangerous cells are skipped by default in run-all. Source: https://docs.runme.dev/configuration/cell-level/
7. **A "stale" cell state** (marimo lazy mode) rather than auto-rerun. Source: https://docs.marimo.io/guides/reactivity/
8. **Streaming outputs to every connected client** from a server-side run. Source: https://jupyter-ai.readthedocs.io/en/stable/releases/v3.1.0.html
9. **ANSI/HTML/image/JSON/media-router output renderers** from nteract elements, if the licence passes. Source: https://nteract-elements.vercel.app/docs
10. **Env vars persisting across cells like a terminal session,** as Runme does. Source: https://github.com/runmedev/runme
11. **Document-level `eval: false` equivalent** ("this note's cells are present-only") for the "prepare but don't apply" request. Source: https://quarto.org/docs/computations/execution-options.html

### Avoid (ranked)
1. **Approval prompts as the only safety layer.** Humans miss about a third of threats, and disguised script names are missed more than half the time. Sources: https://scalex.dev/blog/ai-agent-permissions-stats/ and https://botmonster.com/ai/terraform-incident-claude-code-wiped-production-database/
2. **Showing only the command and not what it will do.** Source: https://specstory.com/learning/environments/approval-fatigue
3. **Stacking permission systems** (allowlist, sandbox, classifier) that interact silently. Source: https://forum.cursor.com/t/command-allowlist-is-silently-ignored-when-auto-run-in-sandbox-is-enabled/152136
4. **Giving the agent a run path through MCP** (nteract's model) for 171's cells. It breaks the human-only guarantee. Source: nteract README, §A7.
5. **Depending on a runbook-editor product.** Atuin Desktop was archived. Source: https://github.com/atuinsh/desktop
6. **Cursor Shell Mode's limits as a model:** 30 s timeout, no interactivity, no cwd persistence. Source: https://cursor.com/docs/cli/shell-mode
7. **A Python kernel picker separate from the project toolchain.** Source: https://zed.dev/docs/repl
8. **Replicating live run state inside a shared document without a lock.** Jupyter AI had a data-loss race when a second user joined. Source: https://jupyter-ai.readthedocs.io/en/stable/releases/v3.1.0.html
9. **Assuming a sandbox isolates files and network.** Source: https://docs.marimo.io/guides/package_management/sandboxes/
10. **JSON `id` written into human-authored Markdown** if the notes must stay clean. Source: https://docs.runme.dev/configuration/cell-level/

---

## §I. The ten facts the design most depends on

1. Every mainstream harness frames the choice as "agent runs, human approves". I found no mainstream product whose core pattern is "agent prepares, human presses Run". The closest are Warp Notebooks (paste into the user's terminal) and Cursor CLI Shell Mode (user-run, output into the conversation). Sources: https://code.claude.com/docs/en/permission-modes, https://docs.warp.dev/knowledge-and-collaboration/warp-drive/notebooks, https://cursor.com/docs/cli/shell-mode
2. A human approval did not prevent the DataTalks.Club `terraform destroy`. The user approved without noticing the swapped state file. The cell must show the consequence (plan summary), not only the command. Source: https://botmonster.com/ai/terraform-incident-claude-code-wiped-production-database/
3. In the 40,000-play dataset humans missed 1 in 3 threats, and a disguised `npm run analyze` was approved 64.7% of the time even with the payload shown. This is a game dataset and a short session, not a controlled study. Source: https://scalex.dev/blog/ai-agent-permissions-stats/
4. Runme is the nearest cell grammar: a fence plus attributes including `cwd`, `interpreter`, `interactive` and `excludeFromRunAll`, with doc-level `cwd` and `shell` and shared env across cells. Source: https://docs.runme.dev/configuration/cell-level/
5. nteract's runtime model is the one to copy structurally: cells in an Automerge doc, execution state in a separate doc, and the daemon reads source by `cell_id`. But its MCP lets agents run code, which 171 must not do. Source: https://raw.githubusercontent.com/nteract/nteract/main/README.md
6. nteract elements is a shadcn-registry (copy-in) component set marked "Work in Progress", with 9 output renderers and CodeMirror. I did not find its licence. Source: https://nteract-elements.vercel.app/docs
7. Atuin Desktop, the closest standalone "runbooks that run" product, was archived on 30 Sep 2026. Source: https://github.com/atuinsh/desktop
8. Terraform-ecosystem approval UIs use consequence-named buttons ("Confirm & Apply", "Discard Run") and put the plan in the place the reviewer reads. Atlantis applies by PR comment. Sources: https://developer.hashicorp.com/terraform/cloud-docs/run/manage, https://www.runatlantis.io/docs/using-atlantis.html
9. Shell Mode in Cursor documents the same traps Sky hit: login shell (`$SHELL`), 30 s timeout, no interactive prompts, no `cd` persistence, truncated output. A Python REPL in Zed needs a kernel that differs from the project toolchain. Sources: https://cursor.com/docs/cli/shell-mode, https://zed.dev/docs/repl
10. NN/g: a confirmation is "not a second chance at all" when users answer it automatically. Per-command approval invites that habit, which argues for one press per cell, with the danger shown at a glance, not a second confirmation. Source: https://www.nngroup.com/articles/confirmation-dialog/

## Gaps to hand to other recon children
- nteract elements and runtimed licences: not found.
- Copilot "run in terminal", ChatGPT and Claude code execution UIs, Goose, Observable, Fiberplane, Rundeck, Netflix Jupyter practice: not read or 404.
- GitHub issues with high reactions on wrong shell/cwd/env, or on run buttons: not found. Queries on two repos were rate-limited (HTTP 403).
- Atuin Desktop archive reason: not found.
- Interruption-cost research and a controlled study of consequence-labelled buttons for command approval: not found.