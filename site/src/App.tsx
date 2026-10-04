import { useEffect, useState, type ReactNode } from "react";
import engraving from "./engraving.webp";
import { Pyramid } from "./Pyramid";

declare const __APP_VERSION__: string;
declare const __RELEASE_VERSION__: string;

const GITHUB = "https://github.com/BaseDatum/Theuth";
const APP = "https://app.theuth.io";
const RELEASES = "https://s3.rwx.dev/theuth/releases";
/** Release artifacts live at releases/<version>/theuth_<version>_<target>; there is no latest/ path, latest.json names the version. */
const artifact = (v: string, target: string) => `${RELEASES}/${v}/theuth_${v}_${target}`;
const DOCS = "/docs";

type Os = "macos" | "windows" | "linux";

/** navigator.userAgentData.platform where Chromium ships it, the UA string elsewhere; anything unrecognised is a Mac. */
function detectOs(): Os {
  const p = ((navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ?? navigator.userAgent).toLowerCase();
  if (p.includes("windows")) return "windows";
  if (p.includes("linux") || p.includes("x11") || p.includes("android")) return "linux";
  return "macos";
}

/** The brand mark (crates/desktop/icons): compact skull, one disk for an eye, a beak that tapers into a reed-pen nib. */
function Ibis({ size = 22 }: { size?: number }) {
  return (
    <svg viewBox="60 60 904 904" width={size} height={size} aria-hidden>
      <g fill="currentColor">
        <circle cx="640" cy="340" r="165" />
        <path d="M 660 178 C 400 196, 230 470, 186 846 L 244 890 C 300 570, 400 380, 600 500 Z" />
      </g>
      <circle cx="610" cy="288" r="36" className="fill-accent" />
    </svg>
  );
}

/** A link into the docs at /docs/<path>; paths follow the docs' own tree (getting-started, workspace, automation, reference). docs-cta makes it tap-sized when it stands alone. */
function Doc({ path = "", className = "", children }: { path?: string; className?: string; children: ReactNode }) {
  return (
    <a href={path ? `${DOCS}/${path}` : DOCS} className={`docs-link ${className}`.trim()}>
      {children}
    </a>
  );
}

/** Per OS: the install command (none for Windows: there is no script), the note, and the downloads; the first download is the primary. */
const OSES: Record<Os, { label: string; cmd: string | null; note: string; files: [string, string][] }> = {
  macos: {
    label: "macOS",
    cmd: "brew update\nbrew install --cask basedatum/tap/theuth",
    note: "The cask is the app; brew install basedatum/tap/theuth is the CLI alone: t, the daemon and the terminal UI in one binary, kept current by itself. Apple silicon; the app wants macOS 15.",
    files: [
      ["disk image", "macos-aarch64.dmg"],
      ["CLI tar.gz", "macos-aarch64.tar.gz"],
    ],
  },
  windows: {
    label: "Windows",
    cmd: null,
    note: "x86_64, Windows 11 or Windows 10 with WebView2. The installer is unsigned: SmartScreen says unknown publisher once; More info, Run anyway. It installs per user under %LOCALAPPDATA%\\theuth with t.exe beside the app. The zip is t.exe and its helpers; unzip it anywhere.",
    files: [
      ["installer", "windows-x86_64-setup.exe"],
      ["CLI zip", "windows-x86_64.zip"],
    ],
  },
  linux: {
    label: "Linux",
    cmd: "brew update\nbrew install basedatum/tap/theuth",
    note: "glibc 2.39 or newer: Ubuntu 24.04, Debian 13, Fedora 40 and later. The formula installs the CLI on x86_64 and arm64; the bundles are the app with t beside it, x86_64 only, published when a release builds them. AppImage: chmod +x, then run. Debian: sudo apt install ./theuth_<version>_linux-x86_64.deb.",
    files: [
      ["AppImage x86_64", "linux-x86_64.AppImage"],
      ["deb x86_64", "linux-x86_64.deb"],
      ["rpm x86_64", "linux-x86_64.rpm"],
      ["CLI tar.gz x86_64", "linux-x86_64.tar.gz"],
      ["CLI tar.gz arm64", "linux-aarch64.tar.gz"],
    ],
  },
};

function Install({ os, setOs, version }: { os: Os; setOs: (os: Os) => void; version: string }) {
  const [copied, setCopied] = useState(false);
  const active = OSES[os];

  const copy = () => {
    if (!active.cmd) return;
    navigator.clipboard
      .writeText(active.cmd)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      })
      .catch(() => {});
  };

  return (
    <section id="install" className="mx-auto max-w-3xl scroll-mt-24 px-6 pb-36">
      <div className="border border-well bg-raised">
        <div
          role="tablist"
          className="grid grid-cols-3 border-b border-well bg-well text-[13px] font-medium"
        >
          {(Object.keys(OSES) as Os[]).map((id) => (
            <button
              key={id}
              role="tab"
              aria-selected={id === os}
              onClick={() => setOs(id)}
              className={
                id === os
                  ? "bg-raised py-3 text-fg shadow-[inset_0_-2px_0_var(--accent)] transition-colors duration-[120ms]"
                  : "py-3 text-fg-3 transition-colors duration-[120ms] hover:text-fg-2"
              }
            >
              {OSES[id].label}
            </button>
          ))}
        </div>
        {active.cmd && (
          <div className="flex items-start justify-between gap-4 border-b border-well p-5">
            <pre className="min-w-0 overflow-x-auto font-mono text-[13px] leading-6 text-fg">
              {active.cmd.split("\n").map((line) => (
                <div key={line}>
                  <span className="select-none text-fg-3">$ </span>
                  {line}
                </div>
              ))}
            </pre>
            <button
              onClick={copy}
              className="btn h-8 shrink-0 px-3 text-[12px]"
              aria-live="polite"
            >
              {copied ? "copied" : "copy"}
            </button>
          </div>
        )}
        <ul className="flex flex-wrap items-center gap-x-5 gap-y-3 p-5 font-mono text-[13px]">
          {active.files.map(([label, target], i) => (
            <li key={target}>
              <a
                href={artifact(version, target)}
                className={
                  i === 0
                    ? "btn btn-primary h-9 px-4 text-[13px]"
                    : "text-fg-2 underline decoration-fg-3 underline-offset-4 hover:text-fg"
                }
              >
                {i === 0 ? `Download ${label}` : label}
              </a>
            </li>
          ))}
        </ul>
        <p className="border-t border-well px-5 py-3 text-[13px] leading-relaxed text-fg-3">
          {active.note}
        </p>
      </div>
      <dl className="mt-8 grid gap-x-8 gap-y-3 font-mono text-[13px] text-fg-3 sm:grid-cols-2">
        {[
          ["t", "opens the terminal UI"],
          ["t --update", "checks for an update"],
          ["~/.theuth", "holds config, sessions, knowledge"],
          ["wss://relay.theuth.io", "is the default relay"],
        ].map(([k, v]) => (
          <div key={k} className="flex gap-3">
            <dt className="text-fg">{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
      <p className="mt-6 text-[14px] leading-relaxed text-fg-3">
        New here? The <Doc path="getting-started">quickstart</Doc> walks the first session;{" "}
        <Doc path="getting-started/installation">installation</Doc> covers every package and{" "}
        <Doc path="getting-started/models">models</Doc> the providers and logins.
      </p>
    </section>
  );
}

const FLAGS = [
  "t",
  "t --headless",
  "t --acp",
  "t --link",
  "t --install-service",
  "t mesh add --vm",
  "t tasks list",
  "t --slack-setup",
  "t --link-signal",
];

const TOOL_FAMILIES = [
  ["shell", "background jobs"],
  ["pty", "start, send, screen, wait"],
  ["files", "read, write, list, grep, glob, edit"],
  ["code", "find, outline, refs, map, symbol, impact, project_profile"],
  ["documents", "doc_read, insert, replace, delete, move"],
  ["timers", "remind_me_in, cancel_reminder"],
  ["browser", "sixteen tools, screenshots"],
  ["computer", "eight tools, your desktop"],
  ["web", "search_web, read_url, search_x"],
  ["scrape", "scrape_url, crawl, map_site"],
  ["mcp", "any mcp.json, hot-reloaded"],
  ["routes", "site_learn, site_routes, site_call"],
  ["python", "state kept across turns"],
  ["tasks", "create, checkout, update, move, reassign, note, place"],
  ["agents", "agents_list, agent_draft, agent_put, agent_propose"],
  ["sandbox", "create, share, snapshot, restore, takeover"],
];

const SURFACES: { group: string; doc: string; items: { name: string; href?: string; body: string }[] }[] = [
  {
    group: "At the machine",
    doc: "workspace/desktop",
    items: [
      { name: "t", body: "The terminal UI. The default." },
      {
        name: "theuth.app",
        body: "Menubar app on macOS, tray app on Windows and Linux. Looks like a chat page and speaks JSON-RPC to the daemon. A globe in the header opens a live view of the browser the agent is driving.",
      },
      {
        name: "t --acp",
        body: "Agent Client Protocol on stdio, agent name theuth. Zed and any other ACP client.",
      },
    ],
  },
  {
    group: "Away from it",
    doc: "automation/channels",
    items: [
      {
        name: "app.theuth.io",
        href: APP,
        body: "Your phone or any browser, paired with a QR through the relay. Nothing listens on your machine; the relay forwards encrypted frames and never sees their contents or your keys.",
      },
      {
        name: "Slack",
        body: "t --slack-setup, then t --headless. Replies stream into the thread with a card per tool call; /theuth carries the commands. theuth agent, in your workspace.",
      },
      {
        name: "Signal",
        body: "t --link-signal, then headless. Texts become turns in a per-contact session; approvals are answered with a thumbs-up.",
      },
    ],
  },
];

const HANDS_BACK = [
  {
    name: "show",
    title: "Documents",
    doc: "workspace/documents",
    body: "Markdown, CSV tables, images, SVG, Mermaid diagrams, archive listings, and PDFs compiled in-process from markdown or Typst. Every card is a versioned snapshot; the model gets the first page back as an image and fixes what it sees.",
  },
  {
    name: "app",
    title: "Screens you work in",
    doc: "workspace/apps",
    body: "A dashboard, a control panel, a form. The agent publishes an interactive app whose buttons read your installed tools live, without another model turn. The press is the approval. A reply can carry a static card inline; Open as app keeps it.",
  },
  {
    name: "schedule",
    title: "Runs, loops and watchers",
    doc: "automation/schedules",
    body: "One markdown file per schedule with cron, every, or at in the front matter and the prompt as the body. Add until or max_runs and it repeats until the job is done, each report delivered to you. Add when: and a small Python sensor decides whether the model wakes at all.",
  },
  {
    name: "voice",
    title: "Read aloud, dictate, talk",
    doc: "workspace/voice",
    body: "Pocket TTS, Parakeet and Nemotron streaming speech models run inside the harness, on your machine: replies read back, dictation into any text field, hold-to-record voice notes, and talk, a live spoken conversation you can interrupt. Audio stays local; the text of the conversation goes to the model provider you chose, as in any chat.",
  },
];

/** The agents and machines side: everything here ships; the ledger below has the detail. */
const TEAM = [
  {
    name: "agents",
    title: "Hire a standing role",
    doc: "automation/agents",
    body: "Hire a named agent in chat or from the Agents view: a title, a charter, who it reports to, a model, a tool ceiling, an MCP allowlist, an optional monthly budget. Agents can propose hires; those wait, paused, in the Hiring inbox until you decide.",
  },
  {
    name: "tasks",
    title: "A board, not a transcript",
    doc: "automation/tasks",
    body: "Per project: backlog, todo, doing, review, blocked, done. You, theuth and its agents move the same cards, from chat or by dragging in the app; t tasks list shows the board from a shell. An attention inbox collects approvals, decisions, blocked cards and budgets.",
  },
  {
    name: "watchers",
    title: "Wake the model only when it matters",
    doc: "automation/watchers",
    body: "A schedule with when: runs a small Python sensor on the cadence, in the sandboxed runtime, and starts a turn only when the condition holds, with what it saw. Fire on change, while true or once; cooldown, hold-for, expiry.",
  },
  {
    name: "delegation",
    title: "Children with a leash",
    doc: "automation/delegation",
    body: "spawn_subagent runs a child here, on another engine, or in an external harness over the Agent Client Protocol. Its tool ceiling is the minimum over its ancestors and cannot be raised. task_place hands a card to another agent.",
  },
  {
    name: "mesh",
    title: "Several machines, one harness",
    doc: "automation/mesh",
    body: "Engines pair with a short string and find each other on the LAN, through NAT or through the relay. Agents, tasks, schedules and sessions replicate; the most stable engine holds Slack, Signal, schedule ticks and desks, and hands them over when it goes away.",
  },
  {
    name: "sandboxes",
    title: "A desktop that is not yours",
    doc: "automation/sandboxes",
    body: "t mesh add --vm boots an isolated Linux desktop microVM with the directories you share, joined to the mesh as an engine. Watch it in the app, take over, hand back, snapshot, throw away. Apple silicon, or Linux with KVM.",
  },
];

/** The whole ledger: what the tool is, what it lets you do, why that matters. Facts from the README and the as-built design notes only. */
const LEDGER: { group: string; doc: string; items: [string, string, string][] }[] = [
  {
    group: "It acts",
    doc: "reference/tools-permissions",
    items: [
      [
        "shell",
        "Runs commands as background jobs. Anything that finishes in three seconds answers inline; anything longer hands back a job id and wakes the model when it exits. cwd, env and timeout_secs apply to one call; PATH is seeded from your login shell, so what your dotfiles install is found. A job you kill from /jobs comes back as one line, not its output.",
        "Start the build, walk away, and the result finds you. Nothing blocks on a slow command.",
      ],
      [
        "pty",
        "Starts a program on a pseudo-terminal and drives it: type into it, read the rendered 120 by 40 screen, wait for a prompt or a port. wait matches output since the model's last send. A screen that stops changing for tools.pty_idle_secs wakes the model once with job_idle and the screen. On Windows the shell is pwsh over ConPTY.",
        "REPLs, dev servers, installers and TUIs, not just one-shot commands. A program waiting on a question no longer waits forever.",
      ],
      [
        "files",
        "Read, write, list, grep, glob and edit on the machine it runs on. read_file answers with a header and numbered lines, 2000 at a time: offset, tail, several ranges, or outline for the symbols of a source file; PDFs and Office files come back as markdown. edit forgives trailing whitespace, indentation and pasted line numbers, applies a list of edits all or nothing and returns a diff; write_file is atomic. Neither will write a truncated read back.",
        "The unglamorous majority of real work, without re-sending a whole file for a three-line change.",
      ],
      [
        "code",
        "Seven functions in the Python runtime over a tree-sitter symbol index built on first use under ~/.theuth/codeindex: code_find, code_outline, code_refs, code_map (a ranked map fitted to a token budget), code_symbol (definition, body, callers, callees, co-changed files), code_impact (what a diff touches, down to the symbol, and its tests) and project_profile (how the project is checked: package manager and the check, test, lint, format, build and typecheck commands read from manifests, lockfiles, justfile and CI, nothing run). Rust, TypeScript, JavaScript, Python and Go; read_file outline uses the same index, and the app's CodeMap panel draws it live. Nothing is written into your repo.",
        "Briefing a subagent or checking its diff costs a few hundred tokens of structure instead of thousands of lines of grep.",
      ],
      [
        "doc_*",
        "doc_read, doc_insert, doc_replace, doc_delete and doc_move on any note, knowledge concept, meeting note or markdown artifact, by block id; doc_create starts a note. Edits land as one transaction, typed out in every open editor and attributed to theuth; Undo last agent edit reverts the last one. Mutating calls pass the same approval gate.",
        "Ask for the change in chat and watch it happen in the paragraph you meant, not a rewrite of the file.",
      ],
      [
        "browser",
        "Sixteen tools on an embedded headless browser: open, read, click, fill, type, select, scroll, eval, screenshot, tabs. Stealth TLS fingerprint, tracker blocklist, optional proxy, timezone and geolocation. Every browser tool except browser_import_cookies is classed read_only, so under the default policy a click, a form submit or an eval runs without a prompt even though the site may act on it. import_cookies is off by default; turned on, the agent loads your real browser profile and acts as the signed-in you from then on without further prompts. Tighten it with tools.policy or watch it live.",
        "Sites with no API. Forms, dashboards, portals. Know what the gate covers before you hand it an account.",
      ],
      [
        "computer",
        "One screenshot of every display, then native mouse and keyboard on your own desktop. macOS asks for Screen Recording and Accessibility once.",
        "Apps with no CLI, no API and no web page still get done.",
      ],
      [
        "web",
        "search_web fans out to Google, Brave, Qwant, DuckDuckGo, Wikipedia, arXiv and GitHub without a key (Bing is there but off by default), merges by reciprocal rank, then reads the top hits and returns the passages that answer the query. Firecrawl, SearXNG, OpenSERP or the Brave API plug in as extra engines. read_url fetches with an SSRF guard that refuses private, loopback and link-local addresses. search_x reads X posts through xAI's x_search once an xai login or secret exists.",
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
        "mcp",
        "Drop any mcp.json from Claude Desktop, Cursor or OpenCode into ~/.theuth/mcp and it hot-reloads. Shell expansions and vault secrets in the config, OAuth for remote servers, per-server and per-tool risk overrides, git-cloned servers with /mcp add and /mcp sync.",
        "Every MCP server you already have, with tokens that never sit in a file.",
      ],
      [
        "message_user",
        "Sends text and files to you on Slack or Signal while you are not looking at the chat.",
        "It can finish the job after you close the laptop and tell you where the result is.",
      ],
      [
        "remind_me_in",
        "The model sets a timer instead of sleeping: the call returns at once, the session is free, and when it fires the message comes back as a reminder that starts or joins a turn. Five seconds to a day, eight pending per session, cancel_reminder retires one; reminders survive a restart.",
        '"Check the deploy in ten minutes" costs nothing while it waits.',
      ],
      [
        "ask_user",
        "Puts one or more multiple-choice questions to you: pick, type your own, skip one or dismiss. Works in the TUI, the app, a Slack thread and over Signal.",
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
        "Files the agent makes for you appear as cards: markdown, code, CSV tables, images, SVG, PDFs compiled in-process from markdown or Typst, Mermaid diagrams with pan, zoom, fullscreen and PNG or SVG export, and zip and tar archives listed from their headers without extracting. Every card is an immutable versioned snapshot; a markdown card is also a document the agent can edit with doc_*, each edit the next version.",
        "The transcript never lies about what you were shown. Copy, save, open, reveal.",
      ],
      [
        "app",
        "Interactive screens in OpenUI Lang: a dashboard, a control panel, a form. Buttons and inputs bind to your installed tools and run them live, without another model turn. The press is the approval. Fifty-three curated components, from Tabs and Steps to radar charts and a Graph that lights up the node an app is talking about. A reply can carry a static openui card inline; Open as app publishes it, no model turn. Apps run in the desktop app and the browser UI; the TUI, Slack and Signal get a receipt and the source.",
        "A tool the agent builds once and you use for months.",
      ],
      [
        "visualize",
        "A mermaid, json-render or openui fence in the model's reply renders as a picture in the TUI and the app and reads as text on Slack and Signal. json-render has a typed catalog: cards, tables, metrics, timelines, checklists, a Graph with typed nodes and edges laid out by ELK. A block that fails to render goes back to the model for up to three fix-up rounds; you never see a red card, at worst one dim diagram omitted.",
        "Architecture, flows and status boards arrive as one picture, and a typo in the picture is the model's problem, not yours.",
      ],
      [
        "schedule",
        "One markdown file per schedule, Routines in the app, with cron, every or at in the front matter and the prompt as the body. Add until or max_runs and it loops until done; approvals: false for unattended runs. Add when: sensor.py key=value and it is a watcher: a small Python check in the sandboxed runtime runs on the cadence, every 30 s at the fastest, and wakes the model only when it is true, with what it saw; fire on change, while true or once, with cooldown, hold-for and expiry; a watcher needs a stop and is switched off after three failing checks.",
        "Morning reports, nightly checks, a job that retries itself until the test passes, an inbox watched without a model turn per look.",
      ],
      [
        "voice",
        "Read aloud with Pocket TTS, voice notes with Parakeet, dictation with Nemotron streaming into any text field, and talk: a live spoken conversation you can interrupt. The speech models run inside the harness on your machine; the words, once text, go to the model provider you chose, like any message. Hold the mic to record a note, slide to cancel or lock; a note or dictation mid-turn steers the running turn. Echo cancellation uses the system voice processing on macOS.",
        "Talk to it and hear it back, with no audio sent to a speech API.",
      ],
      [
        "meetings",
        "Record a meeting from the menubar or /meetings: system audio or one app, your mic, transcribed live and batched into a session that keeps briefs on the people and tickets mentioned, shown in a notch island with a live transcript; a meeting bar under every pane, one click back. A note opens beside the transcript; the summary turn regroups what you typed under headings and appends Summary, Decisions, Action items and Open questions, never rewriting your words. Every device reads meetings. As built on macOS 15, with echo cancellation; the Windows 11 and Linux PipeWire capture paths are partial and lightly exercised, so plan on headphones there.",
        'You never have to ask "what\'s X?" in a meeting, and the notes exist before you leave the call.',
      ],
      [
        "knowledge",
        "Durable facts about your machines, projects, accounts and decisions as plain markdown concepts under ~/.theuth/knowledge, full-text indexed, every write a git commit made in-process. Harvested automatically every few tool-using turns, detached, so your next turn never waits behind one; surfaced as hints at the start of a turn. Every concept opens in the block editor with its frontmatter as the first block; the agent edits it with doc_* as knowledge:<id>.",
        "It remembers your setup across sessions, and you can read, edit, diff and restore every word of it in any editor.",
      ],
      [
        "notes",
        "Notes in the sidebar and /notes: block documents you type in, attach to a chat from the composer or open a chat under, so the model knows which document you mean. Deleted notes wait thirty days in the trash. The agent starts one with doc_create.",
        "A scratchpad the agent can read and edit on request, without owning a session.",
      ],
      [
        "editor",
        "One block editor for knowledge, notes and meeting notes: headings, lists, check lists, tables, highlighted code, a frontmatter block on concepts; a mermaid or json-render code block renders as the diagram. Every document is a CRDT: the markdown on disk is rewritten 400 ms after you stop typing, and edits to the file, from sed, knowledge_put or git pull, show up in the open editor. Snapshots list and restore. Export a note as Markdown or PDF, compiled in-process with Typst. On a phone a bar above the keyboard replaces the drag handles.",
        "Type where the agent types; the file on disk stays the file every other tool reads.",
      ],
      [
        "boards",
        "Ask how a piece of code works and the agent writes a board: a scrollable explanation with sections, excerpts anchored to the working tree, graph and sequence diagrams that light up with the sentence you are reading, a stop-by-stop tour, select-to-ask, and anchors re-verified against the code through the index. Export to PDF.",
        "A code walkthrough that stays honest about the file it points at.",
      ],
      [
        "search",
        "One fuzzy search over sessions, shown artifacts, current knowledge and knowledge history. Command K in the app, /search in the TUI, and the same tool for the model. Archived sessions are found and marked.",
        "Find the deployment plan you were shown last week. A paired phone gets results, never the corpus.",
      ],
      [
        "skills",
        "SKILL.md directories, loose markdown or git clones under ~/.theuth/skills; /skills add a URL and /skills sync pulls. A harness.md manual of this install is written on every start, with the visualize and apps catalogs beside it. In the app, /skill:<name> parks a skill on the composer and it rides with the next send.",
        "Your procedures, loaded on demand instead of pasted every time.",
      ],
      [
        "subagents",
        "spawn_subagent runs a child session on this machine, on another engine in the mesh, or in an external harness from acp/agents.yaml over the Agent Client Protocol, with the model, effort and tool cap you give it: tools=read_only|mutating|destructive is a ceiling the child cannot raise, the minimum over its ancestors, and the board shows a read-only badge. opencode is the external harness the test suite drives. discover_peers reports who is working, with jobs, terminals and tags; send_message takes one address or sixteen. Each Slack subagent gets its own thread with a live board in the parent.",
        "Fan out, delegate, and keep the approvals in one place. Off until agent.communication is on.",
      ],
      [
        "agents",
        "Hire a named agent in chat or from the Agents view: a file under ~/.theuth/agents with a title, a charter, who it reports to, a model and effort, a tool ceiling, an MCP allowlist and an optional monthly budget. Agents propose hires with agent_propose; the proposal lands paused in the Hiring inbox until you decide. An org chart shows the roster; theuth is the root and stays.",
        "Standing roles instead of one long chat: the on-call reviewer, the release shepherd, each on a leash you set.",
      ],
      [
        "tasks",
        "A per-project board, backlog, todo, doing, review, blocked, done, that you, theuth and its agents move cards across: task_create, task_checkout, task_update, task_move, task_reassign and task_note for the model, drag in the app; t tasks list reads the board from a shell, filtered by owner, state, engine or project. Finished work moves to review. An attention inbox collects approvals, decisions, blocked cards and budgets.",
        "Who is doing what, and what is waiting on you, in one place that is not a transcript.",
      ],
      [
        "a2a",
        "Remote agents over A2A 1.0 on the official Rust SDK: named peers, exported sessions, vault-held credentials, loopback listener with an HTTPS proxy in front. A second, separate switch.",
        "Talk to agents outside your box only when you turned it on, and only through the sessions you exported.",
      ],
    ],
  },
  {
    group: "You stay in control",
    doc: "reference",
    items: [
      [
        "approvals",
        "Each tool carries a risk class, read_only, mutating or destructive, and tools.policy maps each class to allow, ask or deny; the default allows read_only and asks for the rest. Any argument carrying a {{secret:NAME}} at least asks. The card lands in the thread you are in. /approvals off for a session, or for good. The class is the tool's, not the effect's: browser clicks and web reads are read_only.",
        "One dial over every tool, with a clear line on what it does and does not catch.",
      ],
      [
        "broadcast",
        "One message to every running session, all roots, or the ones you tick, as a steer or as the next turn: /broadcast in the TUI, Broadcast in the app, owner-only /broadcast on Slack and Signal. Each recipient's transcript carries a broadcast marker; a dry run shows who is busy first.",
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
        "An encrypted, versioned vault; the key lives in the macOS Keychain, the Linux Secret Service or the Windows Credential Manager, with an owner-only file where none answers. The model uses a secret as {{secret:NAME}} in any tool argument and the value is swapped in at run time and scrubbed from output. secret_request opens a masked box for you to type into.",
        "API keys, tokens and passwords the model can use but never read or leak.",
      ],
      [
        "context",
        "Compact by summary, cut or snap at a threshold you set. A context meter, used over limit, moves after every model round in the app header and the TUI. /undo and /redo a turn. /shake images, documents or tool output out of context without losing what came after. /edit a message and replay. Type while it works to queue; interject to redirect now.",
        "Long sessions stay cheap and steerable instead of restarting from zero.",
      ],
      [
        "models",
        "Sign in with the OpenAI, Anthropic, Cursor or xAI subscription you have. Keys in the vault: the genai providers by adapter name, OpenCode Zen as the opencode secret or OPENCODE_API_KEY, OpenRouter as the openrouter secret or OPENROUTER_API_KEY; both are built-in providers, not gateways you add. Bedrock through an AWS profile with SSO, no credentials in the UI. Any other OpenAI-compatible gateway or a local model through models.json. Per-model effort levels; agent.escalate_model names the cheap model for side calls. models.accelerator picks where local speech and decision models run: auto, cpu, webgpu (Metal on macOS, Vulkan on Linux, D3D12 on Windows), directml (Windows, explicit opt-in) or cuda (needs the CUDA build); t --bench-models tells you what it got. When a provider returns no price, the cost is estimated from catalog rates and marked as such.",
        "Use what you already pay for. Switch with /model mid-session.",
      ],
      [
        "vision fallback",
        "Set agent.vision_model and a text-only main model still handles images: it writes a question, an image-capable model answers, the answer joins the turn.",
        "Cheap or local models keep working when a screenshot comes in.",
      ],
      [
        "labs",
        "Settings → Labs switches on a local classifier, laya, about 0.8 GB, loaded only while it is needed and never leaving the machine. Meeting oracle Off, Shadow or Active: small talk is skipped and a cached brief published without a model turn, a ⚡ row under each batch shows the verdict, and Shadow tells you how often it agreed with the model before you let it act. Browser System-1 adds browser_run, up to 25 actions under one approval. Training capture writes secret-scrubbed rows to this disk only.",
        "Fewer model turns on the decisions a small local model gets right, and you watch it be right first.",
      ],
      [
        "failures",
        "Denied model access, expired AWS sessions and stalled streams stay in the transcript with guidance. They are never fed to the model as conversation. Interrupted replies are salvaged and resumed.",
        "You see what went wrong; the model does not hallucinate around it.",
      ],
      [
        "update",
        "One signed release manifest: the macOS app and CLI, CLI tarballs for Linux x86_64 and aarch64, and the Windows installer and CLI zip when a release builds them. The t binary on macOS and Linux verifies the signature, replaces itself on disk and is live at the next start; the macOS app bundle updates itself the same way. Windows and the Linux desktop packages are updated by reinstalling; the Windows installer is unsigned. t --update forces a check.",
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
        "Pair a phone or any browser with a QR through the relay. The uplink dials out; nothing listens on your machine. Noise end-to-end encryption; the relay keeps no state and no accounts, forwards encrypted frames, and learns only frame sizes, timing, IP addresses and the harness id, never your messages or keys. Devices listed, renamed and revoked from /link. Self-host the relay with one compose file. A note or concept open on a paired device edits the same document live, cursors and names shown; offline keystrokes queue and replay on reconnect.",
        "Drive the harness from a phone without opening a port.",
      ],
      [
        "share",
        "Share… on a note or concept makes a link, editor or viewer, expiring in a day, a week, a month or never, shown once as URL and QR. Whoever opens it in a plain browser gives a name and joins that one document over the same encrypted relay path as a paired phone, with only that document's doc_* methods. See who is here, demote, remove, revoke; a viewer's edits are refused. documents.sharing turns it off.",
        "Co-edit with someone who has no theuth, on your machine's copy, with the relay still unable to read a word of it.",
      ],
      [
        "nodes",
        "On another box: install, set a key, t --install-service. It writes a launchd or systemd user unit through your login shell, or a Task Scheduler logon task on Windows, turns link on and prints a pairing key. /nodes add on your main harness.",
        "Give the model, and your phone, another machine. Subagents run there; approvals land in front of you.",
      ],
      [
        "mesh",
        "Several engines, no leader: t mesh pair prints a string, t mesh add pairs with it over iroh QUIC, on the LAN by mDNS, through NAT by hole punching, through the relay otherwise. Agents, tasks, schedules, skills, secrets and sessions replicate; the most stable engine holds the hub lease, the Slack and Signal sockets, schedule ticks and desks, and the lease moves when it goes away. t mesh add --ssh installs an engine on a Linux or Mac host; --docker runs one in a container on this machine.",
        "Your laptop, a server and a VM are one harness; close the laptop and another engine takes the duties.",
      ],
      [
        "sandbox",
        "t mesh add --vm, or sandbox_create from the model, boots an isolated Linux desktop microVM on this machine with the host directories you share, joined to the mesh as an engine. sandbox_share, _snapshot, _restore, _resize, _pause, _takeover and more; t sandbox ls|start|stop|rm|desktop and /sandbox manage them, and the app shows the VM's desktop with Take over and Hand back. Apple silicon, or Linux with KVM; Intel Macs and Windows hosts are refused with a reason.",
        "Let it install, click and break things on a machine that is not yours, then snapshot it or throw it away.",
      ],
      [
        "slack",
        "t --slack-setup creates the app from a manifest and verifies the tokens. DMs and mentions open sessions that stream into threads with a card per tool call, Allow and Deny buttons, and /theuth for the commands, broadcast among them.",
        "theuth agent, in your workspace, with the same approvals.",
      ],
      [
        "signal",
        "t --link-signal pairs the daemon as a linked device. Texts become turns in a per-contact session; eyes while working, a check when done; approvals answered with a thumbs-up; owner-only /broadcast; files land in an inbox.",
        "The agent in your pocket over the messenger you already trust.",
      ],
      [
        "acp",
        "t --acp speaks the Agent Client Protocol on stdio, agent name theuth.",
        "Zed and any other ACP editor, no plugin.",
      ],
      [
        "engine",
        "One engine per data directory. If the app, a headless t or another t is already running on your ~/.theuth, then t, t -p and t --acp attach to it over loopback JSON-RPC: same jobs, terminals, Python runtime, MCP servers and approvals; the status line says attached. When it quits, the TUI takes over.",
        "Never two engines fighting over one store; the terminal and the app are two views of one agent.",
      ],
      [
        "theuth.app",
        "A menubar app on macOS, a tray app on Windows and Linux: sessions with the subagent tree, streamed markdown with tool cards, inline approvals, attachments, a settings dialog for every connection, a knowledge explorer with history and a raw editor, notes, artifact and app panels, a real terminal for the agent's PTY jobs, a live view of the agent's browser. No Dock icon on macOS; the tray or a rebindable chord, Option Space by default, toggles it. Attaches to a running engine or starts one.",
        "Everything the terminal does, as a chat window that is always one keystroke away.",
      ],
      [
        "workspace",
        "Chats grouped under projects, each with an icon and one of ten mineral colours. Drag a row onto the chat to tile it; drag New chat to open a draft pane; with the view split, New chat opens in the focused pane. Right-click any row for its menu. Every shortcut is rebindable in Settings → Shortcuts. Sidebar glyphs show which chat is thinking, in a tool, running a job or holding a live terminal.",
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

const SOON = ["iOS app: a paired client over the link, TestFlight pipeline in the repo", "Android app"];
const NOT = [
  "Telegram, Discord or WhatsApp",
  "Intel Mac builds",
  "A published Docker image: t mesh add --docker builds the engine container locally",
];
const LIMITS = [
  "Meetings are built for macOS 15; capture on Windows 11 and Linux with PipeWire exists but is partial and lightly exercised, without echo cancellation. Every device reads meetings.",
  "Echo-cancelled voice input uses macOS voice processing; speech on Windows and Linux is less exercised.",
  "Sandbox VMs need Apple silicon or Linux with KVM, and a published VM image for your release; the image is pulled on first create. Read the sandboxes doc for what the current release provides before relying on them. t mesh add --ssh does not target Windows hosts.",
  "computer_* needs Screen Recording and Accessibility on macOS and an X11 display on Linux.",
];

/** The docs tree, one column per section; the overview of each section lives at /docs/<section>. */
const DOCS_SECTIONS: { title: string; path: string; body: string; links: [string, string][] }[] = [
  {
    title: "Getting started",
    path: "getting-started",
    body: "Install, the first session, picking a model.",
    links: [
      ["Quickstart", "getting-started"],
      ["Installation", "getting-started/installation"],
      ["First session", "getting-started/first-session"],
      ["Models", "getting-started/models"],
    ],
  },
  {
    title: "Workspace",
    path: "workspace",
    body: "The app and the terminal, and what you work in.",
    links: [
      ["Desktop", "workspace/desktop"],
      ["Terminal", "workspace/terminal"],
      ["Sessions", "workspace/sessions"],
      ["Voice", "workspace/voice"],
      ["Meetings", "workspace/meetings"],
      ["Documents", "workspace/documents"],
      ["Knowledge", "workspace/knowledge"],
      ["Apps", "workspace/apps"],
      ["Search", "workspace/search"],
    ],
  },
  {
    title: "Automation",
    path: "automation",
    body: "Agents, tasks, schedules, other machines, channels.",
    links: [
      ["Agents", "automation/agents"],
      ["Delegation", "automation/delegation"],
      ["Tasks", "automation/tasks"],
      ["Schedules", "automation/schedules"],
      ["Watchers", "automation/watchers"],
      ["Mesh", "automation/mesh"],
      ["Sandboxes", "automation/sandboxes"],
      ["Channels", "automation/channels"],
      ["Browser and desktop", "automation/browser-desktop"],
    ],
  },
  {
    title: "Reference",
    path: "reference",
    body: "Every switch, tool and command.",
    links: [
      ["Configuration", "reference/configuration"],
      ["Tools and permissions", "reference/tools-permissions"],
      ["MCP", "reference/mcp"],
      ["Skills", "reference/skills"],
      ["Secrets", "reference/secrets"],
      ["CLI", "reference/cli"],
      ["Troubleshooting", "reference/troubleshooting"],
      ["Development", "reference/development"],
    ],
  },
];

export function App() {
  const [version, setVersion] = useState(__RELEASE_VERSION__);
  const [os, setOs] = useState<Os>(detectOs);
  useEffect(() => {
    // latest.json is the only mutable object in the bucket; when it is reachable, follow it instead of the build-time version.
    fetch(`${RELEASES}/latest.json`)
      .then((r) => r.json() as Promise<{ version: string }>)
      .then(({ version }) => setVersion(version))
      .catch(() => {});
  }, []);

  return (
    <>
      <a href="#main" className="skip-link btn btn-primary">
        Skip to content
      </a>
      <Pyramid />
      <header className="sticky top-0 z-10 flex h-16 items-center justify-between border-b border-well/60 bg-page/80 px-6 backdrop-blur-sm md:px-10">
        <a
          href="#"
          className="nav-link gap-2.5 text-[15px] font-medium"
        >
          <Ibis />
          theuth
        </a>
        <nav aria-label="Primary" className="flex items-center gap-3 text-[14px] text-fg-2 sm:gap-6">
          <a href={DOCS} className="nav-link">
            docs
          </a>
          <a href={GITHUB} className="nav-link max-sm:hidden">
            github
          </a>
          <a href={APP} className="nav-link">
            app
          </a>
          <a href="#install" className="btn h-11 px-4 text-[13px]">
            Install
          </a>
        </nav>
      </header>

      <main id="main">
        <section className="mx-auto grid min-h-[calc(100dvh-4rem)] max-w-[1400px] grid-cols-1 items-center gap-12 px-6 pt-16 pb-24 md:grid-cols-12 md:px-10">
          <div className="md:col-span-7">
            <h1 className="display text-[clamp(64px,10.5vw,128px)]">
              The harness
              <br />
              that does
              <br />
              things
            </h1>
            <p className="mt-8 max-w-[28em] text-[18px] leading-relaxed text-fg-2">
              A single-process AI harness that does things for the user.
            </p>
            <div className="mt-10 flex flex-wrap gap-3">
              <a href="#install" className="btn btn-primary">
                Install
              </a>
              <a href={artifact(version, OSES[os].files[0]![1])} className="btn">
                Download for {OSES[os].label}
              </a>
            </div>
            <p className="mt-6 text-[14px] text-fg-3">
              New here?{" "}
              <Doc path="getting-started" className="docs-cta">
                Read the quickstart →
              </Doc>
            </p>
          </div>
          <div className="relative hidden min-h-[60vh] md:col-span-5 md:block">
            <div className="absolute top-0 right-0 text-fg-2">
              <Ibis size={40} />
            </div>
          </div>
        </section>

        <Install os={os} setOs={setOs} version={version} />

        <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
          <div className="md:w-[54%]">
            <img
              src={engraving}
              alt="An ibis holding a reed pen, engraved over construction lines"
              className="aspect-[16/9] w-full object-cover object-[50%_35%]"
              loading="lazy"
            />
            <h2 className="display mt-12 text-[clamp(44px,5.4vw,76px)]">
              Not a coding agent.
            </h2>
            <p className="mt-6 max-w-[34em] text-[17px] leading-relaxed text-fg-2">
              theuth runs shell jobs, drives a real browser and your own
              desktop, reads and writes files, and searches the web without a
              key. What it makes, it hands back as documents and apps you can
              open. It staffs a task board with agents you hire, each on a
              leash you set, across the machines you pair. Mutating and
              destructive tools ask first.
            </p>
            <p className="mt-4 max-w-[34em] text-[15px] leading-relaxed text-fg-3">
              A single-process Rust harness. Plugins over a typed bus. Frontends
              attach over JSON-RPC.
            </p>
          </div>
        </section>

        <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
          <ol className="grid gap-px border border-well bg-well md:grid-cols-12">
            <li className="bg-page/85 p-7 backdrop-blur-[2px] md:col-span-7 md:p-9">
              <p className="index">01</p>
              <h2 className="mt-3 text-[22px] font-medium">One process.</h2>
              <p className="mt-4 max-w-[38em] text-[15px] leading-relaxed text-fg-2">
                t is the daemon, the terminal UI, the headless server and the
                editor agent. Plugins are Rust crates on a typed message bus.
                Every frontend attaches over JSON-RPC: the TUI, the macOS app, a
                phone through the relay, Slack, Signal, your editor. One binary,
                one data directory, one set of approvals.
              </p>
              <ul className="mt-8 flex flex-wrap gap-x-6 gap-y-2 font-mono text-[13px] text-fg-3">
                {FLAGS.map((f) => (
                  <li key={f}>{f}</li>
                ))}
              </ul>
              <Doc path="workspace/sessions" className="docs-cta mt-6">
                Docs: sessions and the engine →
              </Doc>
            </li>
            <li className="bg-raised/90 p-7 backdrop-blur-[2px] md:col-span-5 md:p-9">
              <p className="index">02</p>
              <h2 className="mt-3 text-[22px] font-medium">Tools that act.</h2>
              <p className="mt-4 text-[15px] leading-relaxed text-fg-2">
                Quick shell commands answer inline; long ones run as jobs and
                wake the model when they exit. Long-running programs get a
                pseudo-terminal the agent types into and reads back. The browser
                and the desktop are real, with screenshots. Every tool is also a
                function inside a Python runtime that keeps its state.
              </p>
              <dl className="mt-7 grid grid-cols-[auto_1fr] gap-x-5 gap-y-1.5 font-mono text-[12.5px]">
                {TOOL_FAMILIES.map(([k, v]) => (
                  <div key={k} className="contents">
                    <dt className="text-fg">{k}</dt>
                    <dd className="text-fg-3">{v}</dd>
                  </div>
                ))}
              </dl>
              <Doc path="automation/browser-desktop" className="docs-cta mt-6">
                Docs: browser and desktop →
              </Doc>
            </li>
            <li className="bg-page/85 p-7 backdrop-blur-[2px] md:col-span-4 md:p-9">
              <p className="index">03</p>
              <h2 className="mt-3 text-[22px] font-medium">
                Ask before it bites.
              </h2>
              <p className="mt-4 text-[15px] leading-relaxed text-fg-2">
                Every tool carries a risk class; tools.policy maps each class to
                allow, ask or deny. By default read-only tools run and
                mutating and destructive ones ask, in the thread you are in: a
                card in the TUI or the app, Allow and Deny in Slack, a thumbs-up
                on Signal. The class is the tool's, not the effect's: a browser
                click is read-only. Secrets live in an encrypted vault; the
                model uses them by name and never sees a value.
              </p>
              <pre className="mt-7 border border-well bg-well/60 p-4 font-mono text-[12.5px] leading-6 text-fg-2">
                {
                  "tools:\n  policy:\n    read_only:   allow\n    mutating:    ask\n    destructive: ask"
                }
              </pre>
              <Doc path="reference/tools-permissions" className="docs-cta mt-6">
                Docs: tools and permissions →
              </Doc>
            </li>
            <li className="bg-page/85 p-7 backdrop-blur-[2px] md:col-span-4 md:p-9">
              <p className="index">04</p>
              <h2 className="mt-3 text-[22px] font-medium">Memory as marks.</h2>
              <p className="mt-4 text-[15px] leading-relaxed text-fg-2">
                Knowledge is plain markdown under ~/.theuth/knowledge, indexed
                for full-text search, every change a git commit made in-process,
                so history and restore work everywhere. The harness harvests
                what is worth keeping while you work. Sessions live in sqlite.
                Compact by summary, cut, or snap; undo, redo, or shake images
                and tool output out of the context; search everything you ever
                showed it.
              </p>
              <Doc path="workspace/knowledge" className="docs-cta mt-6">
                Docs: knowledge →
              </Doc>
            </li>
            <li className="bg-page/85 p-7 backdrop-blur-[2px] md:col-span-4 md:p-9">
              <p className="index">05</p>
              <h2 className="mt-3 text-[22px] font-medium">Other machines.</h2>
              <p className="mt-4 text-[15px] leading-relaxed text-fg-2">
                Pair a phone or any browser with a QR through
                wss://relay.theuth.io. Nothing listens on your machine; the link
                is end-to-end encrypted with Noise, and the relay keeps no state
                and no accounts. Other machines join as engines of one leaderless
                mesh: t mesh pair, then t mesh add, with --ssh for a host you
                can reach, --docker for a container here, --vm for an isolated
                Linux desktop. Agents, tasks and schedules replicate; subagents
                run where you say; approvals land in front of you.
                Model-to-model talk stays off until agent.communication is on.
              </p>
              <Doc path="automation/mesh" className="docs-cta mt-6">
                Docs: mesh and sandboxes →
              </Doc>
            </li>
            <li className="bg-page/85 p-7 backdrop-blur-[2px] md:col-span-12 md:grid md:grid-cols-12 md:gap-10 md:p-9">
              <div className="md:col-span-5">
                <p className="index">06</p>
                <h2 className="mt-3 text-[22px] font-medium">
                  Any model you already pay for.
                </h2>
              </div>
              <div className="md:col-span-7">
                <p className="mt-4 text-[15px] leading-relaxed text-fg-2 md:mt-0">
                  Sign in with the OpenAI, Anthropic, Cursor or xAI subscription
                  you have. Put API keys in the vault, OpenCode Zen and
                  OpenRouter among them. Use Bedrock through an AWS profile, SSO
                  included, with no credentials in the UI. Add any
                  OpenAI-compatible gateway, or a local model. A text-only model
                  can borrow a vision model's eyes when an image comes in.
                </p>
                <Doc path="getting-started/models" className="docs-cta mt-6">
                  Docs: models and logins →
                </Doc>
              </div>
            </li>
          </ol>
        </section>

        <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
          <h2 className="display text-[clamp(44px,5.4vw,76px)]">
            What it hands back.
          </h2>
          <ul className="mt-14 grid gap-y-12 md:w-[54%]">
            {HANDS_BACK.map((h) => (
              <li
                key={h.name}
                className="grid grid-cols-[7rem_1fr] gap-6 max-sm:grid-cols-1 max-sm:gap-3"
              >
                <p className="font-mono text-[15px] text-fg">{h.name}</p>
                <div>
                  <h3 className="text-[18px] font-medium">{h.title}</h3>
                  <p className="mt-2 text-[15px] leading-relaxed text-fg-2">
                    {h.body}
                  </p>
                  <Doc path={h.doc} className="docs-cta mt-3">
                    Docs: {h.title.toLowerCase()} →
                  </Doc>
                </div>
              </li>
            ))}
          </ul>
        </section>

        <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
          <h2 className="display text-[clamp(44px,5.4vw,76px)]">
            It runs a team.
          </h2>
          <p className="mt-6 max-w-[34em] text-[17px] leading-relaxed text-fg-2">
            Agents you hire, a board they work from, watchers that wake them,
            and the machines they run on. One set of approvals over all of it.
          </p>
          <ul className="mt-14 grid gap-px border border-well bg-well sm:grid-cols-2 md:grid-cols-3">
            {TEAM.map((t) => (
              <li key={t.name} className="flex flex-col bg-page/85 p-7 backdrop-blur-[2px]">
                <p className="font-mono text-[14px] text-fg">{t.name}</p>
                <h3 className="mt-3 text-[18px] font-medium">{t.title}</h3>
                <p className="mt-3 flex-1 text-[14.5px] leading-relaxed text-fg-2">
                  {t.body}
                </p>
                <Doc path={t.doc} className="docs-cta mt-5">
                  Docs: {t.name} →
                </Doc>
              </li>
            ))}
          </ul>
        </section>

        <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
          <h2 className="display text-[clamp(44px,5.4vw,76px)]">
            Six doors, one daemon.
          </h2>
          <div className="mt-14 grid gap-12 md:w-[54%] md:grid-cols-2">
            {SURFACES.map((g) => (
              <div key={g.group}>
                <h3 className="text-[15px] font-medium text-fg">{g.group}</h3>
                <ul className="mt-6 space-y-8">
                  {g.items.map((s) => (
                    <li key={s.name}>
                      <p className="font-mono text-[14px] text-fg">
                        {"href" in s ? (
                          <a
                            href={s.href}
                            className="underline decoration-fg-3 underline-offset-4 hover:text-fg-2"
                          >
                            {s.name}
                          </a>
                        ) : (
                          s.name
                        )}
                      </p>
                      <p className="mt-2 text-[14.5px] leading-relaxed text-fg-3">
                        {s.body}
                      </p>
                    </li>
                  ))}
                </ul>
                <Doc path={g.doc} className="docs-cta mt-6">
                  Docs: {g.group.toLowerCase()} →
                </Doc>
              </div>
            ))}
          </div>
        </section>

        <section
          id="everything"
          className="mx-auto max-w-[1400px] scroll-mt-24 px-6 pb-36 md:px-10"
        >
          <h2 className="display text-[clamp(44px,5.4vw,76px)]">
            Everything it does.
          </h2>
          <p className="mt-6 max-w-[34em] text-[17px] leading-relaxed text-fg-2">
            The whole ledger. Each line is a tool or a switch that ships in{" "}
            {__APP_VERSION__}, what it lets you do, and why you would care. Each
            group links to the part of the <Doc>docs</Doc> that goes deeper.
          </p>
          <div className="mt-14 border border-well bg-page/85 backdrop-blur-[2px]">
            {LEDGER.map((g) => (
              <div
                key={g.group}
                className="border-t border-well first:border-t-0 md:grid md:grid-cols-12"
              >
                <div className="p-7 md:col-span-2 md:p-9">
                  <h3 className="index">{g.group}</h3>
                  <Doc path={g.doc} className="docs-cta mt-2">
                    docs →
                  </Doc>
                </div>
                <dl className="divide-y divide-well/60 md:col-span-10">
                  <div className="hidden grid-cols-[9rem_1fr_1fr] gap-8 px-7 pb-3 text-[12px] tracking-[0.12em] text-fg-3 uppercase md:grid md:px-9 md:pt-9">
                    <span />
                    <span>lets you</span>
                    <span>so that</span>
                  </div>
                  {g.items.map(([name, does, why]) => (
                    <div
                      key={name}
                      className="grid gap-2 px-7 py-6 md:grid-cols-[9rem_1fr_1fr] md:gap-8 md:px-9"
                    >
                      <dt className="font-mono text-[14px] text-fg">{name}</dt>
                      <dd className="text-[14.5px] leading-relaxed text-fg-2">
                        {does}
                      </dd>
                      <dd className="text-[14.5px] leading-relaxed text-fg-3">
                        {why}
                      </dd>
                    </div>
                  ))}
                </dl>
              </div>
            ))}
          </div>
        </section>

        <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
          <div className="grid gap-12 md:w-[54%] md:grid-cols-2">
            <div>
              <h2 className="text-[18px] font-medium">Coming soon</h2>
              <ul className="mt-5 space-y-2 text-[15px] text-fg-2">
                {SOON.map((s) => (
                  <li key={s}>{s}</li>
                ))}
              </ul>
              <button disabled className="btn mt-8" title="Coming soon">
                Join the testing group
              </button>
              <p className="mt-2 text-[13px] text-fg-3">
                The group opens with the mobile betas.
              </p>
            </div>
            <div>
              <h2 className="text-[18px] font-medium">Not here</h2>
              <ul className="mt-5 space-y-2 text-[15px] text-fg-3">
                {NOT.map((n) => (
                  <li key={n}>{n}</li>
                ))}
              </ul>
              <h2 className="mt-10 text-[18px] font-medium">Platform limits today</h2>
              <ul className="mt-5 space-y-2 text-[14px] leading-relaxed text-fg-3">
                {LIMITS.map((l) => (
                  <li key={l}>{l}</li>
                ))}
              </ul>
              <Doc path="reference/troubleshooting" className="docs-cta mt-5">
                Docs: troubleshooting →
              </Doc>
            </div>
          </div>
        </section>

        <section
          id="docs"
          aria-labelledby="docs-heading"
          className="mx-auto max-w-[1400px] scroll-mt-24 px-6 pb-36 md:px-10"
        >
          <h2 id="docs-heading" className="display text-[clamp(44px,5.4vw,76px)]">
            Read the docs.
          </h2>
          <p className="mt-6 max-w-[34em] text-[17px] leading-relaxed text-fg-2">
            Four sections, from the first session to every switch in
            config.yaml. Start with the quickstart.
          </p>
          <div className="mt-8 flex flex-wrap gap-3">
            <a href={`${DOCS}/getting-started`} className="btn btn-primary">
              Quickstart
            </a>
            <a href={DOCS} className="btn">
              All docs
            </a>
          </div>
          <div className="mt-14 grid gap-px border border-well bg-well sm:grid-cols-2 md:grid-cols-4">
            {DOCS_SECTIONS.map((s) => (
              <div key={s.path} className="bg-page/85 p-7 backdrop-blur-[2px]">
                <h3 className="text-[18px] font-medium">
                  <a href={`${DOCS}/${s.path}`} className="hover:text-fg-2">
                    {s.title}
                  </a>
                </h3>
                <p className="mt-2 text-[14px] leading-relaxed text-fg-3">{s.body}</p>
                <ul className="mt-5">
                  {s.links.map(([label, path]) => (
                    <li key={path}>
                      <Doc path={path} className="block py-2">
                        {label}
                      </Doc>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        </section>
      </main>

      <footer className="border-t border-well/60 bg-page/80 px-6 py-10 backdrop-blur-sm md:px-10">
        <div className="mx-auto flex max-w-[1400px] flex-wrap items-center justify-between gap-6">
          <a
            href="#"
            className="nav-link gap-2.5 text-[15px] font-medium"
          >
            <Ibis />
            theuth
          </a>
          <nav aria-label="Footer" className="flex flex-wrap items-center gap-x-8 gap-y-2 font-mono text-[13px] text-fg-3">
            <a href={DOCS} className="nav-link">
              docs
            </a>
            <a href={`${DOCS}/getting-started`} className="nav-link">
              quickstart
            </a>
            <a href={GITHUB} className="nav-link">
              github.com/BaseDatum/Theuth
            </a>
            <a href={APP} className="nav-link">
              app.theuth.io
            </a>
            <span>relay.theuth.io</span>
          </nav>
        </div>
      </footer>
    </>
  );
}
