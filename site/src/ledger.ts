/** The whole ledger: what the tool is, what it lets you do, why that matters. Facts from the README, the code and the as-built design notes only. */
export const LEDGER: { group: string; doc: string; items: [string, string, string][] }[] = [
  {
    group: "It acts",
    doc: "reference/tools-permissions",
    items: [
      [
        "shell",
        "Runs commands as background jobs. Anything that finishes in three seconds answers inline; anything longer hands back a job id and wakes the model when it exits. cwd, env and timeout_secs apply to one call; PATH is seeded from your login shell, so what your dotfiles install is found. A job you kill from /jobs comes back as one line, not its output.",
        "Start the build, walk away and the result finds you. Nothing blocks on a slow command.",
      ],
      [
        "pty",
        "Starts a program on a pseudo-terminal and drives it: type into it, read the rendered 120 by 40 screen, wait for a prompt or a port. wait matches output since the model's last send. A screen that stops changing for tools.pty_idle_secs wakes the model once with job_idle and the screen. On Windows the shell is pwsh over ConPTY.",
        "REPLs, dev servers, installers and TUIs, not just one-shot commands. A program waiting on a question no longer waits forever.",
      ],
      [
        "files",
        "Read, write, list, grep, glob and edit on the machine it runs on. read_file answers with a header and numbered lines, 2000 at a time: offset, tail, several ranges or outline for the symbols of a source file; PDFs and Office files come back as markdown, images as pictures. edit forgives trailing whitespace, indentation and pasted line numbers, applies a list of edits all or nothing and returns a diff; write_file is atomic. move_text, copy_text, indent and clip move, copy or re-indent a block by its line reference without the model retyping it. Neither will write a truncated read back.",
        "The unglamorous majority of real work, without re-sending a whole file for a three-line change.",
      ],
      [
        "code",
        "Eight functions in the Python runtime over a tree-sitter symbol index built on first use under ~/.theuth/codeindex: code_find, code_outline, code_refs, code_map (a ranked map fitted to a token budget), code_symbol (definition, body, callers, callees, co-changed files), code_impact (what a diff touches, down to the symbol, and its tests), code_overview (the architecture perspectives the CodeMap draws, as data) and project_profile (how the project is checked: package manager and the check, test, lint, format, build and typecheck commands read from manifests, lockfiles, justfile and CI, nothing run). Rust, TypeScript, JavaScript, Python and Go; read_file outline uses the same index, and the app's CodeMap panel draws it live. Nothing is written into your repo.",
        "Briefing a subagent or checking its diff costs a few hundred tokens of structure instead of thousands of lines of grep.",
      ],
      [
        "doc_*",
        "doc_read, doc_insert, doc_replace, doc_delete and doc_move on any note, knowledge concept, meeting note or markdown artifact, by block id; doc_create starts a note. Edits land as one transaction, typed out in every open editor and attributed to theuth; Undo last agent edit reverts the last one. Mutating calls pass the same approval gate.",
        "Ask for the change in chat and watch it happen in the paragraph you meant, not a rewrite of the file.",
      ],
      [
        "browser",
        "Eighteen tools on an embedded headless browser: open, read, click, fill, type, select, scroll, wait, eval, screenshot, tabs, attach and more. Stealth TLS fingerprint, tracker blocklist, optional proxy, timezone and geolocation. One browser profile per mesh, sealed at rest: sign in once in a web view tile and the agent's browser on any of your machines carries on signed in; Settings → Browser lists the sites, with Forget this site and Sign out everywhere. browser_show_element lifts a sign-in form into a chat card, so you type the password there and the agent gets a receipt without values. Browser tools are classed read_only, but an acting call on a site signed in through the profile, or on your own tile, is gated as mutating. browser_import_cookies, which loads your desktop browser's profile, is off by default.",
        "Sites with no API. Forms, dashboards, portals. Log in once and the agent inherits the session, never the password.",
      ],
      [
        "computer",
        "Sixteen computer_* tools. By pixels: one screenshot of every display, through ScreenCaptureKit on macOS, then native mouse and keyboard. By the accessibility tree: computer_snapshot reads an app's elements as refs and computer_act presses, types into or sets them headlessly, without moving the pointer or stealing focus; find, get, windows, window and clipboard read and write sit beside them. Adapters for macOS, Windows and Linux; applescript, shortcuts and powershell in the same category. macOS asks for Screen Recording and Accessibility once.",
        "Apps with no CLI, no API and no web page still get done, and the agent can work in one while you type in another.",
      ],
      [
        "web",
        "search_web fans out to Google, Brave, Qwant, DuckDuckGo, Wikipedia, arXiv and GitHub without a key (Bing is there but off by default), merges by reciprocal rank, then reads the top hits and returns the passages that answer the query. Firecrawl, SearXNG, OpenSERP or the Brave API plug in as extra engines. read_url fetches with an SSRF guard that refuses private, loopback and link-local addresses, and names a login, consent or bot wall as one instead of returning it as text. search_x reads X posts through xAI's x_search once an xai login or secret exists.",
        "No search API subscription. One call replaces search, click, skim.",
      ],
      [
        "scrape",
        "scrape_url, crawl and map_site on a vendored crawler: stays under the seed, honours robots.txt, seeds from sitemaps, two engine tiers when a site needs a real browser.",
        "A whole documentation site or catalogue in context, politely.",
      ],
      [
        "routes",
        "site_learn opens a page in the session's browser and records the JSON routes its own JavaScript calls, as URL templates with holes; click a filter or next page and learn again if the first load was plain HTML. site_call replays a learned route over plain HTTP and returns records, no page, no rendering; site_routes lists what a site has taught it. Header names are stored, never values; http(s) only; all three are read-only.",
        "The second thousand rows of a dashboard come back as data, not as a thousand screenshots.",
      ],
      [
        "python",
        "A persistent runtime where every tool is an async function and state survives across turns. The worker is the t binary itself; nothing to install. Cells are type-checked before they run, with imports from earlier cells carried over; sleep really waits, up to five minutes a call; tool calls made inside a cell show as child cards and survive reload.",
        "Loops, data wrangling and glue code without a model round trip per step.",
      ],
      [
        "autoresearch",
        "Give it a goal, a benchmark that prints a number, a correctness check and the files it may change; /autoresearch or New study drafts the study in a chat, and a field is marked verified only after the engine has actually run your benchmark. Each experiment runs in its own git worktree, your checkout untouched; the harness, not the model, runs the benchmark, and a change is kept only if it beats the incumbent by more than the measured noise and passes the check. Evaluator files are held in custody by sha256, a hidden holdout runs at the end and kept diffs can be reviewed by a model from another vendor. Studies can also weigh several objectives on a Pareto front, score prompts case by case, let a blind judge pick between two outputs, search a population or measure a live deploy with rollback. Needs a POSIX shell; on Windows run it from WSL.",
        "Wake up to a chart and a stack of measured changes, not a model's word that it got faster.",
      ],
      [
        "mcp",
        "Drop any mcp.json from Claude Desktop, Cursor or OpenCode into ~/.theuth/mcp and it hot-reloads. Shell expansions and vault secrets in the config, OAuth for remote servers, per-server and per-tool risk overrides, git-cloned servers with /mcp add and /mcp sync.",
        "Every MCP server you already have, with tokens that never sit in a file.",
      ],
      [
        "message_user",
        "Sends text and files to you on Slack, Signal or Telegram while you are not looking at the chat; diagrams in the message arrive as pictures.",
        "It can finish the job after you close the laptop and tell you where the result is.",
      ],
      [
        "remind_me_in",
        "The model sets a timer instead of sleeping: the call returns at once and the session is free; when it fires the message comes back as a reminder that starts or joins a turn. Five seconds to a day, eight pending per session, cancel_reminder retires one; reminders survive a restart.",
        '"Check the deploy in ten minutes" costs nothing while it waits.',
      ],
      [
        "ask_user",
        "Puts one or more multiple-choice questions to you: pick, type your own, skip one or dismiss. Works in the TUI, the app, a Slack thread, over Signal and in a Telegram chat.",
        "Fewer confident wrong guesses on the decisions that matter.",
      ],
    ],
  },
  {
    group: "It hands back",
    doc: "workspace",
    items: [
      [
        "show",
        "Files the agent makes for you appear as cards: markdown, code, CSV tables, images, SVG, PDFs compiled in-process from markdown or Typst, Mermaid diagrams with pan, zoom, fullscreen and PNG or SVG export; zip and tar archives are listed from their headers without extracting. An .html file is a live page inline in the transcript, sandboxed and themed to the app; the connected app renders it before it is published and a console error sends it back to the model, which fixes it with show{name, edits}. Every card is an immutable versioned snapshot; a markdown card is also a document the agent can edit with doc_*, each edit the next version.",
        "The transcript never lies about what you were shown. Copy, save, open, reveal.",
      ],
      [
        "tiles",
        "show_web opens a dev server, a dashboard or a docs site in a native web view beside the chat, not an iframe, so sites that refuse framing still render; you click, scroll or pop it out into its own window, and the agent can drive the same tile with browser_attach. show_pty puts a live terminal there that you can type into, show_note a note or board, show_file a highlighted source file that follows edits, show_diff a git diff of the working tree, a commit or a range, split if you like. The web view is a stranger to the app: no app IPC, its own cookie store, http and https only.",
        "The agent starts the app and the page is already open beside the conversation.",
      ],
      [
        "app",
        "Interactive screens in OpenUI Lang: a dashboard, a control panel, a form. Buttons and inputs bind to your installed tools and run them live, without another model turn. The press is the approval. Ninety-seven curated components, from Tabs and Steps to radar charts, Gantt and Sankey diagrams and a Graph that lights up the node an app is talking about. A reply can carry a static openui card inline; Open as app publishes it, no model turn. Apps run in the desktop app and the browser UI; the TUI, Slack, Signal and Telegram get a receipt and the source.",
        "A tool the agent builds once and you use for months.",
      ],
      [
        "visualize",
        "A mermaid, json-render or openui fence in the model's reply renders as a picture in the TUI and the app; on Slack, Signal and Telegram a connected desktop or web client draws it as a PNG, and text stands in when none is open. json-render has a typed catalog of fifty-seven components: cards, tables, metrics, timelines, checklists, charts, state machines, swimlanes, a Graph with typed nodes and edges laid out by ELK. A block that fails to render goes back to the model for up to three fix-up rounds; you never see a red card, at worst one dim diagram omitted.",
        "Architecture, flows and status boards arrive as one picture, on your phone's messenger too, and a typo in the picture is the model's problem, not yours.",
      ],
      [
        "cells",
        "A fenced block with #| option lines, in a reply, a note or a notebook, becomes a runnable cell: shell on a pseudo-terminal or Python, with live output, run history, after: gates and a danger label. The agent can write a cell but cannot run one: only your press starts it, and the engine refuses the run if the text changed since you looked. Secrets are swapped in at run time and scrubbed from the output. A cell in a note runs on the machine you are viewing it from.",
        "Runbooks the agent drafts and you execute, one press per step.",
      ],
      [
        "schedule",
        "One markdown file per schedule, Routines in the app, with cron, every or at in the front matter and the prompt as the body. Add until or max_runs and it loops until done; approvals: false for unattended runs. Add when: sensor.py key=value and it is a watcher: a small Python check in the sandboxed runtime runs on the cadence, every 30 s at the fastest, and wakes the model only when it is true, with what it saw; fire on change, while true or once, with cooldown, hold-for and expiry; a watcher needs a stop and is switched off after three failing checks.",
        "Morning reports, nightly checks, a job that retries itself until the test passes, an inbox watched without a model turn per look.",
      ],
      [
        "voice",
        "Read aloud with Pocket TTS, voice notes with Parakeet, dictation with Nemotron streaming into any text field and talk: a live spoken conversation you can interrupt, where a tool still running after five seconds becomes a background job so the talk never waits. On Apple silicon Parakeet can run on the GPU through MLX, in-process, after an explicit download; the built-in engine takes over if it fails. The speech models run inside the harness on your machine; the words, once text, go to the model provider you chose, like any message. Hold the mic to record a note, slide to cancel or lock; a note or dictation mid-turn steers the running turn. Echo cancellation uses the system voice processing on macOS.",
        "Talk to it and hear it back, with no audio sent to a speech API.",
      ],
      [
        "dictation",
        "Hold a hotkey and talk, and the words land in whatever app has focus, through Accessibility on macOS and UI Automation on Windows, with spoken punctuation, a personal dictionary and end commands such as send it and scratch that. A second chord is command mode: say what you want done on the computer and the agent acts, checks and reports one line. The clipboard is restored after a paste; one switch in Settings → Voice turns the optional polish rewrite off.",
        "Local dictation in every app, and a voice that can drive the machine, not just a chat box.",
      ],
      [
        "meetings",
        "Record from the menubar or /meetings, or answer when theuth notices an app holding the microphone and asks; it never starts or stops a recording on its own. The call is matched to an event from macOS Calendar or any ICS subscription, and a meeting that belongs to a project gets a pre-brief written from the previous ones. System audio or one app plus your mic, transcribed live into a session that keeps briefs on the people and tickets mentioned, in a notch island and a full-height column; a watchdog reopens stalled capture and marks any audio gap. A note opens beside the transcript; the summary turn appends Summary, Decisions, Action items and Open questions, never rewriting your words; speaker names are suggested from the attendees, and any line can enrol a voice. As built on macOS 15; the Windows 11 and Linux PipeWire capture paths are partial and lightly exercised, so plan on headphones there.",
        'No bot joins your call; you never have to ask "what\'s X?" in a meeting, and the notes exist before you leave it.',
      ],
      [
        "notes",
        "Notes in the sidebar and /notes: block documents you type in, attach to a chat from the composer or open a chat under, so the model knows which document you mean; every note has its own chat bar. Deleted notes wait thirty days in the trash. The agent starts one with doc_create.",
        "A scratchpad the agent can read and edit on request, without owning a session.",
      ],
      [
        "notebooks",
        "A notebook holds notes, chats and sources: files, URLs, meetings, pasted text, past sessions, Confluence pages and Jira issues through MCP, YouTube, audio and video. Each item has a dial, off, mention, summary or full, for what the model sees, with an estimate before the turn; after the first turn only the changes are sent. Answers cite down to the block as numbered chips. Search mixes full text with embeddings from a local model; no provider embeds anything. Ask across several notebooks at once; overviews as a briefing, FAQ, timeline or mind map, and an audio overview read by two local voices.",
        "A research workspace on your own harness that cites its sources and never ships the corpus off to be embedded.",
      ],
      [
        "editor",
        "One block editor for knowledge, notes and meeting notes: headings, lists, check lists, tables, highlighted code, a frontmatter block on concepts; a mermaid or json-render code block renders as the diagram. Every document is a CRDT: the markdown on disk is rewritten 400 ms after you stop typing, and edits to the file, from sed, knowledge_put or git pull, show up in the open editor. Snapshots list and restore. Export a note as Markdown or PDF, compiled in-process with Typst. On a phone a bar above the keyboard replaces the drag handles.",
        "Type where the agent types; the file on disk stays the file every other tool reads.",
      ],
      [
        "boards",
        "Ask how a piece of code works and the agent writes a board: a scrollable explanation with sections, excerpts anchored to the working tree, graph and sequence diagrams that light up with the sentence you are reading, a stop-by-stop tour, select-to-ask and anchors re-verified against the code through the index. Export to PDF.",
        "A code walkthrough that stays honest about the file it points at.",
      ],
      [
        "skills",
        "SKILL.md directories, loose markdown or git clones under ~/.theuth/skills; /skills add a URL and /skills sync pulls. A harness.md manual of this install is written on every start, with the visualize and apps catalogs beside it. In the app, /skill:<name> or the composer's + menu parks one or several skills on the composer and they ride with the next send.",
        "Your procedures, loaded on demand instead of pasted every time.",
      ],
      [
        "subagents",
        "spawn_subagent runs a child session on this machine, on another engine in the mesh or in an external harness over the Agent Client Protocol: Claude Code, Codex, OpenCode or omp, with the model, effort and tool cap you give it. tools=read_only|mutating|destructive is a ceiling the child cannot raise, the minimum over its ancestors. Subagents those harnesses spawn themselves show as watch-only children with live state and transcript. A child that stalls wakes its parent once; wait_peer waits on a child's state and survives a restart. discover_peers reports who is working, with jobs, terminals and tags; send_message takes one address or sixteen. agent.communication, unset, is on while this engine has a paired machine or a contact.",
        "Fan out to the coding agents you already use and see inside their own fan-out, with the approvals in one place.",
      ],
      [
        "agents",
        "Hire a named agent in chat or from the Agents view: a file under ~/.theuth/agents with a title, a charter, who it reports to, a model and effort, a tool ceiling, an MCP allowlist and an optional monthly budget. Agents propose hires with agent_propose; the proposal lands paused in the Hiring inbox until you decide. An org chart shows the roster; theuth is the root and stays.",
        "Standing roles instead of one long chat: the on-call reviewer, the release shepherd, each on a leash you set.",
      ],
      [
        "tasks",
        "A per-project board, backlog, todo, doing, review, blocked, done, that you, theuth and its agents move cards across: task_create, task_checkout, task_update, task_move, task_reassign and task_note for the model, drag in the app; t tasks list reads the board from a shell, filtered by owner, state, engine or project. A task belongs to one machine and agent, so two machines never work it at once. Finished work moves to review; blocked cards and budgets reach the Inbox.",
        "Who is doing what, and what is waiting on you, in one place that is not a transcript.",
      ],
    ],
  },
  {
    group: "It remembers",
    doc: "workspace/knowledge",
    items: [
      [
        "knowledge",
        "Durable facts about your machines, projects, accounts and decisions as plain markdown concepts under ~/.theuth/knowledge, full-text indexed, every write a git commit made in-process. Harvested automatically every few tool-using turns, detached, so your next turn never waits behind one; surfaced as hints at the start of a turn. Every concept opens in the block editor with its frontmatter as the first block; the agent edits it with doc_* as knowledge:<id>.",
        "It remembers your setup across sessions, and you can read, edit, diff and restore every word of it in any editor.",
      ],
      [
        "peek_memory",
        "When a hint points at stored knowledge, the model can ask peek_memory first: the local decision model reads each memory against the question, yes or no, in a fraction of a second and with no API tokens, and only the relevant ones come back in full.",
        "Long-term memory without filling the context with memories that do not apply.",
      ],
      [
        "dreaming",
        "One built-in routine, on by default, in a night window, 01:00 to 06:00, once no session has turned for twenty minutes. It merges, supersedes, archives and re-verifies knowledge, mines your corrections into test cases and proposes small edits to skills and a standing preferences block of at most twenty lines, keeping only those that pass a held-out replay, judged where it can by a model from another vendor. Nothing is deleted; every change is a git commit authored dreaming, with an undo per group. A budget per night, $1 by default, is checked before every model call. Settings → Dreaming has the window, the budget, four model pickers and Dream now. It runs on one machine of the mesh.",
        "An agent that gets better with use instead of rotting, with receipts and an undo for every change.",
      ],
      [
        "fractal context",
        "A memory mode per session, on by default for agent desks: the message log stays the source of truth and the model sees a view over it, up to 128 KB, built from a tree of short summary lines that each keep their address, a small ledger of goals, decisions, constraints and open tasks, plus a verbatim tail of recent turns. The model zooms back into any summarised line for the original. The view is rewritten only at planned points, so the provider's prompt cache keeps hitting, and a small local model can do the summarising. The ∞ switch on a new chat, /fractal or Settings turns it on; a Memory panel shows the backlog, with Fold now and Pause.",
        "Agents that run for weeks without a context wall or a lossy compaction, at cached-token prices.",
      ],
      [
        "isolated memory",
        "Hire an agent with isolated_memory and it gets its own knowledge store under its folder, with the same search, history and tools; it sees none of the main memory, and nothing it learns is indexed there. Knowledge tools refuse rather than fall back. Dreaming leaves it alone. It separates memory, not permissions: the agent keeps every tool it was given.",
        "A specialist agent that does not pollute your everyday memory, or get polluted by it.",
      ],
      [
        "search",
        "One fuzzy search over sessions, shown artifacts, current knowledge and knowledge history. Command K in the app, /search in the TUI and the same tool for the model. Archived sessions are found and marked.",
        "Find the deployment plan you were shown last week. A paired phone gets results, never the corpus.",
      ],
    ],
  },
  {
    group: "You stay in control",
    doc: "reference",
    items: [
      [
        "approvals",
        "Each tool carries a risk class, read_only, mutating or destructive, and tools.policy maps each class to allow, ask or deny; the default allows read_only and asks for the rest. Any argument carrying a {{secret:NAME}} at least asks. The card lands in the thread you are in. /approvals off for a session, or for good. The class is the tool's, not the effect's: web reads and most browser calls are read_only, unless the browser is acting on a signed-in site. A human-only tool, share_publish today, is approved only by you in person, on a surface you own; never another chat user, the double, an app button or a policy switch.",
        "One dial over every tool, with a clear line on what it does and does not catch.",
      ],
      [
        "builtins",
        "Settings → Tools → Builtins sorts every harness tool into 21 categories, from files, shell, browser and computer to secrets, dreaming and channels. A category or a single tool is On, Off here (this chat and its subagents) or Off always (kept in tools.builtins.disabled). A new tool always lands in a category, so none is out of reach of the switch.",
        "Turn off the shell, the browser or the desktop for one chat with one click.",
      ],
      [
        "inbox",
        "Everything that needs you, questions, approvals, mesh notices, tasks, finished and stopped sessions, in one list ordered by a score the engine computes and shows the reasons for. Answer on the card, filter by kind or project, act in bulk from the keyboard. Dismiss leaves it to the agent; it never refuses. A stopped session leaves a card with the reason, a summary, next steps and a reply box. Above the list a live graph draws your sessions, subagents, external harness children, desks, peer machines and contacts, and the messages between them.",
        "Supervise a fleet of agents from one triaged queue, not ten chat windows.",
      ],
      [
        "double",
        "Off by default. A small model on this machine predicts your answer to each approval, question or decision card before you see it, writes the guess down and scores itself when you answer. Trust is earned per kind of card on a ladder: observe, suggest, then answer for you after a 30 second grace you can take over; one override drops it a rung. Destructive calls never go past suggest; secrets, handoffs and human-only approvals are never answered for you.",
        "Autonomy that is measured on your own decisions, not a switch you flip and hope.",
      ],
      [
        "advisor",
        "Pick a second model for one session: it reads each step and posts short notes, with read-only tools and no shell, browser or write access, and a failing advisor never stalls the session. The agent can also consult it for a one-shot opinion, six times a turn by default. Nothing has an advisor until you choose one.",
        "A second pair of eyes from another vendor on a long run, paid for only where you want it.",
      ],
      [
        "broadcast",
        "One message to every running session, all roots or the ones you tick, as a steer or as the next turn: /broadcast in the TUI, Broadcast in the app, owner-only /broadcast on Slack, Signal and Telegram. Each recipient's transcript carries a broadcast marker; a dry run shows who is busy first.",
        'Tell four agents "stop touching Cargo.lock" once, and each knows it was not the only one told.',
      ],
      [
        "terminals",
        "Type into the model's terminals yourself. /terminals then i in the TUI, Ctrl ] to hand the keys back; in the app and on a phone a real terminal with scrollback and resize, a key strip on mobile. The model's next screen read says user attached.",
        "Answer the installer's prompt yourself instead of relaying keystrokes through the model.",
      ],
      [
        "archive",
        "Archive a chat from its row menu or the multi-select bar; archived trees collapse into one Archived group and stay searchable, marked. The title model also tags every session with up to four words, shown on the row and to a model picking peers.",
        "The sidebar shows what you are working on without deleting what you might need.",
      ],
      [
        "secrets",
        "An encrypted, versioned vault; the key lives in the macOS Keychain, the Linux Secret Service or the Windows Credential Manager, with an owner-only file where none answers. The model uses a secret as {{secret:NAME}} in any tool argument and the value is swapped in at run time and scrubbed from output. secret_request opens a masked box for you to type into, or several on one card (username, password, code), with an optional expiry after which the values are deleted.",
        "API keys, tokens and passwords the model can use but never read.",
      ],
      [
        "sharing",
        "Settings → Sharing lists every way out in one place: notes and notebooks shared by link, chats and agents exported to contacts (pause, resume, stop), public shares on the edge and the names your mesh holds. Each has its own Stop.",
        "Every outbound door in one panel, and you can close any of them.",
      ],
      [
        "context",
        "Compact by summary, cut or snap at a threshold you set; or switch to Fractal context. A context meter, used over limit, moves after every model round in the app header and the TUI. /undo and /redo a turn. /shake images, documents, diagrams, tool output or peer messages out of context by category without losing what came after. /edit a message and replay. Type while it works to queue; interject to redirect now.",
        "Long sessions stay cheap and steerable instead of restarting from zero.",
      ],
      [
        "models",
        "Sign in with the OpenAI, Anthropic, Cursor or xAI subscription you have. Keys in the vault: the genai providers by adapter name, OpenCode Zen as the opencode secret or OPENCODE_API_KEY, OpenCode Go as the opencode-go secret or OPENCODE_GO_API_KEY, OpenRouter as the openrouter secret or OPENROUTER_API_KEY; all three are built-in providers, not gateways you add. Bedrock through an AWS profile with SSO, no credentials in the UI. Settings → Local servers probes Ollama, LM Studio, omlx, llama.cpp or vLLM and sets per-model limits; any other OpenAI-compatible gateway through models.json. Every model picker carries effort; agent.escalate_model names the cheap model for side calls. models.accelerator picks where local speech and decision models run: auto, cpu, webgpu (Metal on macOS, Vulkan on Linux, D3D12 on Windows), directml (Windows, explicit opt-in) or cuda (needs the CUDA build); t --bench-models tells you what it got.",
        "Use what you already pay for, or what runs on your own GPU. Switch with /model mid-session.",
      ],
      [
        "costs",
        "/costs shows spend by hour, day, week or month, split by model, provider, session, agent or routine, each dollar marked known, reported, estimated from the price table or unpriced. A second panel shows the ChatGPT and Codex windows and Claude Pro or Max windows as gauges with their history, and rate-limit headroom for API keys.",
        "Know what every agent, routine and model cost you, to the hour, and how close you are to a limit.",
      ],
      [
        "vision fallback",
        "Set agent.vision_model and a text-only main model still handles images: it writes a question, an image-capable model answers, the answer joins the turn.",
        "Cheap or local models keep working when a screenshot comes in.",
      ],
      [
        "labs",
        "Settings → Labs switches on a local classifier, laya, about 0.8 GB, loaded only while it is needed and never leaving the machine. Meeting oracle Off, Shadow or Active: small talk is skipped and a cached brief published without a model turn, a ⚡ row under each batch shows the verdict and Shadow tells you how often it agreed with the model before you let it act. Browser System-1 adds browser_run, up to 25 actions under one approval. Training capture writes secret-scrubbed rows to this disk only.",
        "Fewer model turns on the decisions a small local model gets right, and you watch it be right first.",
      ],
      [
        "failures",
        "Denied model access, expired AWS sessions and stalled streams stay in the transcript with guidance. They are never fed to the model as conversation. Interrupted replies are salvaged and resumed.",
        "You see what went wrong; the model does not hallucinate around it.",
      ],
      [
        "update",
        "One signed release manifest: the macOS app and CLI, CLI tarballs for Linux x86_64 and aarch64 and the Windows installer and CLI zip when a release builds them. The t binary on macOS and Linux verifies the signature, replaces itself on disk and is live at the next start; the macOS app bundle updates itself the same way. Windows and the Linux desktop packages are updated by reinstalling; the Windows installer is unsigned. t --update forces a check.",
        "Current without a reinstall where the updater runs; the manifest tells you where that is.",
      ],
      [
        "print mode",
        't -p "prompt" streams one reply and exits; pipe the prompt on stdin.',
        "Scripts and cron get the same harness the chat does.",
      ],
    ],
  },
  {
    group: "It reaches",
    doc: "automation",
    items: [
      [
        "link",
        "Pair a phone or any browser with a QR through the relay. The uplink dials out; nothing listens on your machine. Noise end-to-end encryption; the relay keeps no state and no accounts, forwards encrypted frames and learns only frame sizes, timing, IP addresses and the harness id, never your messages or keys. Devices listed, renamed and revoked from /link. Self-host the relay with one compose file. A note or concept open on a paired device edits the same document live, cursors and names shown; offline keystrokes queue and replay on reconnect.",
        "Drive the harness from a phone without opening a port.",
      ],
      [
        "co-edit",
        "Share… on a note or concept makes a link, editor or viewer, expiring in a day, a week, a month or never, shown once as URL and QR. Whoever opens it in a plain browser gives a name and joins that one document over the same encrypted relay path as a paired phone, with only that document's doc_* methods. See who is here, demote, remove, revoke; a viewer's edits are refused. documents.sharing turns it off.",
        "Co-edit with someone who has no theuth, on your machine's copy, with the relay still unable to read a word of it.",
      ],
      [
        "publish",
        "share_publish puts a local dev server or a static folder at a real https URL under your mesh's name on the sharing edge, with a Let's Encrypt wildcard certificate; once the name has its certificate, publish to first visit took 385 ms in the first live run. TLS ends on your machine: the edge reads the hostname from the ClientHello and copies encrypted bytes, seeing visitor IPs, hostnames, byte counts and timing, not pages. Off until you allow public sharing on that machine, and each publish is approved by you in person on a card naming the URL, source, expiry and access. Expiry 1 h, 8 h by default, 24 h or 7 d; an optional password the engine generates; one press stops a share and Stop all stops every one. Works behind NAT. The edge's operator owns the domain and could mint a certificate for it, which would show in Certificate Transparency logs; run your own edge to remove that.",
        "A link to what you built, served from your laptop through an edge that cannot read it, and only when you press the button.",
      ],
      [
        "mesh",
        "Several engines, no leader: t mesh pair prints a string and t mesh add pairs with it, or pair a nearby machine found by mDNS once both screens show the same six-digit code and both people press. The pairing string carries the vault key, so treat it as a secret. iroh QUIC, direct on the LAN or by hole punching, otherwise through a relay that sees endpoint ids and IP addresses, not content. Agents, tasks, schedules, skills, secrets and sessions replicate; the most stable engine holds the hub lease, the channel connections, schedule ticks and desks, and the lease moves when it goes away. Unpair mints a new vault key for the machines that stay. t mesh add --ssh installs an engine on a Linux or Mac host, --docker runs one in a container; t --install-service keeps an engine running as a launchd or systemd user unit or a Windows logon task.",
        "Your laptop, a server and a VM are one harness; close the laptop and another engine takes the duties.",
      ],
      [
        "home",
        "A session runs on one machine, its owner. From any other paired machine or your phone you read it live, send to it and change its model or approvals, and the message goes to the owner; if the owner is offline you are asked before the session moves. Move to sends a session to another machine on purpose; New chat on starts one there with that machine's folders and models. Notes, routines and MCP servers have a home machine too.",
        "Work on any machine without the session hopping, and never run the same turn twice.",
      ],
      [
        "sandbox",
        "t mesh add --vm, or sandbox_create from the model, boots an isolated Linux desktop microVM on this machine with the host directories you share, joined to the mesh as an engine. sandbox_share, _snapshot, _restore, _resize, _pause, _takeover and more; t sandbox ls|start|stop|rm|desktop and /sandbox manage them, and the app shows the VM's desktop with Take over and Hand back. Apple silicon, or Linux with KVM; Intel Macs and Windows hosts are refused with a reason.",
        "Let it install, click and break things on a machine that is not yours, then snapshot it or throw it away.",
      ],
      [
        "contacts",
        "Pair with someone else's theuth over A2A on the same encrypted iroh transport: both of you see a code derived from both keys and both press Accept. You export the chats and agents you choose; their agent messages those like any other sender, labelled with its name, under a per-contact risk cap that starts at read_only. Ask first holds a contact's first message until you press Allow once, Always or Block. Full access lets their sessions message yours and list your session titles, never read a transcript; private chats are never listed. A contact added on one machine is a contact of the whole mesh. Contacts never get your vault or your replicas.",
        "Your agents work with a colleague's agents, across companies, on what each of you grants and nothing else.",
      ],
      [
        "contact files",
        "Your agent sends a contact any file up to 5 GB on its own stream of the contact connection, resumable and checked with BLAKE3, and receives files back. A received file lands in a per-contact folder, quarantined on macOS, and the model sees an engine-written placeholder it cannot forge, not the contents. theuth's own secret state is always refused on the way out.",
        "Agents across organisations can hand each other real artifacts without a shared drive.",
      ],
      [
        "mcp server",
        "Switch it on in Settings → MCP and any MCP client, Claude Desktop, an editor or another agent, can search your meetings and transcripts, read and write notes and knowledge or create and move tasks: 44 tools, 25 of them read-only, over streamable HTTP on 127.0.0.1 only or through t mcp on stdio. A bearer token from the vault, results redacted against the vault, private sessions left out, every call in an audit log. Off by default; a connected client's writes are not approval-gated.",
        "Your meetings, notes and tasks become a data source for every other AI tool, on a loopback port you can audit.",
      ],
      [
        "companion",
        "A native helper on macOS and Windows: a rail at the screen edge with your Inbox count and one mark per agent, an approval or a question answered in one keystroke, an Ask bar you type or speak into and the meeting island with its briefs. Point and say, Control S on macOS or Ctrl Alt Space on Windows, lets you circle something on screen and ask about it. Its windows ask the system to stay out of screen shares by default.",
        "Run a team of agents from the edge of the screen instead of hunting for the right tab.",
      ],
      [
        "slack",
        "t --slack-setup creates the app from a manifest and verifies the tokens. DMs and mentions open sessions that stream into threads with one collapsed card per batch of tool calls, a plan message updated in place, Allow and Deny buttons and /theuth for the commands, broadcast and share among them. Tables, metrics and checklists arrive as native Block Kit, diagrams as pictures.",
        "theuth agent, in your workspace, with the same approvals and without the noise.",
      ],
      [
        "signal",
        "t --link-signal pairs the daemon as a linked device. Texts become turns in a per-contact session; eyes while working, a check when done; approvals answered with a thumbs-up; owner-only /broadcast and /share stop; files land in an inbox.",
        "The agent in your pocket over the messenger you already trust.",
      ],
      [
        "telegram",
        "t --telegram-setup verifies a @BotFather token and pairs the owner with a one-time t.me link. With Topics on, every session and subagent gets its own topic; replies stream as a live draft with a native Stop button and land as Telegram-rendered Markdown; one collapsible card per tool batch; eyes while working, an OK when done; Allow and Deny buttons; owner-only /broadcast and /share stop; files land in an inbox.",
        "The agent in your pocket, with buttons.",
      ],
      [
        "acp",
        "t --acp speaks the Agent Client Protocol on stdio, agent name theuth.",
        "Zed and any other ACP editor, no plugin.",
      ],
      [
        "a2a",
        "Remote agents over A2A 1.0 on the official Rust SDK: named peers, exported sessions, vault-held credentials, loopback listener with an HTTPS proxy in front. A second, separate switch, a2a.enabled, off by default; contacts do not need it.",
        "Talk to agents outside your box only when you turned it on, and only through the sessions you exported.",
      ],
      [
        "engine",
        "One engine per data directory. If the app, a headless t or another t is already running on your ~/.theuth, then t, t -p and t --acp attach to it over loopback JSON-RPC: same jobs, terminals, Python runtime, MCP servers and approvals; the status line says attached. When it quits, the TUI takes over.",
        "Never two engines fighting over one store; the terminal and the app are two views of one agent.",
      ],
      [
        "theuth.app",
        "A desktop app for macOS, Windows and Linux, in the Dock and ⌘Tab on macOS and in the tray everywhere: sessions with the subagent tree, streamed markdown with tool cards, inline approvals, attachments, a settings page for every connection, a knowledge explorer with history and a raw editor, notes, artifact and app panels, a real terminal for the agent's PTY jobs, a live view of the agent's browser. A rebindable chord, Option Space by default, shows or hides it; quitting lets running work finish first. Attaches to a running engine or starts one.",
        "Everything the terminal does, as a chat window that is always one keystroke away.",
      ],
      [
        "workspace",
        "Chats grouped under projects, each with an icon and one of ten mineral colours, open as panes in one workspace: drop a chat from the sidebar to tile it or join a panel as a tab, a dot on a tab while its turn runs, Open in new window on every tile. Running chats float to an Active section at the top; the sidebar collapses to an icon rail. Right-click any row for its menu. Every shortcut is rebindable in Settings → Shortcuts. Sidebar glyphs and a one-line status the agent sets show which chat is thinking, in a tool, running a job or holding a live terminal.",
        "Two or four agents side by side in one window, and you see which one needs you.",
      ],
      [
        "t",
        "The terminal UI: slash commands for everything above, /broadcast, /terminals, /agents with live activity, a context gauge in the header, Ctrl V pastes an image, drop a file to attach it, mouse selection is terminal-native, the history stays put while a reply streams. t.exe on Windows.",
        "One binary. Works over ssh.",
      ],
    ],
  },
];
