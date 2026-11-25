import { useEffect, useState } from "react";
import engraving from "./engraving.webp";
import { Pyramid } from "./Pyramid";
import { LEDGER } from "./ledger";
import { Better } from "./sections/Better";
import { DOCS, Doc } from "./sections/doc";
import { Keys } from "./sections/Keys";
import { Publish } from "./sections/Publish";

declare const __APP_VERSION__: string;
declare const __RELEASE_VERSION__: string;

const GITHUB = "https://github.com/BaseDatum/Theuth";
const APP = "https://app.theuth.io";
const RELEASES_API = "https://api.github.com/repos/BaseDatum/Theuth/releases/latest";
/** Release artifacts are attached to each GitHub Release tag as v<version>/theuth_<version>_<target>. */
const artifact = (v: string, target: string) => `${GITHUB}/releases/download/v${v}/theuth_${v}_${target}`;

type Os = "macos" | "windows" | "linux";

/** navigator.userAgentData.platform where Chromium ships it, the UA string elsewhere; anything unrecognised is a Mac. */
function detectOs(): Os {
  const p = ((navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ?? navigator.userAgent).toLowerCase();
  if (p.includes("windows")) return "windows";
  if (p.includes("linux") || p.includes("x11") || p.includes("android")) return "linux";
  return "macos";
}

/** The brand mark (assets/brand, T0 "Theta moon"): a Θ whose ring is a crescent moon and whose bar is a reed pen in carnelian. */
function Theta({ size = 22 }: { size?: number }) {
  return (
    <svg viewBox="14 14 92 92" width={size} height={size} aria-hidden>
      <path fill="currentColor" fillRule="evenodd" d="M18 60 a42 42 0 1 0 84 0 a42 42 0 1 0 -84 0 Z M27 60 a37 37 0 1 0 74 0 a37 37 0 1 0 -74 0 Z" />
      <path fill="#8f2f1f" d="M24 56.5 H86 L101 60 L86 63.5 H24 Z" />
    </svg>
  );
}

/** The wordmark (assets/brand/wordmark.svg): "theuth" set in Copperplate, outlined, in `currentColor`. */
function Wordmark({ height = 13 }: { height?: number }) {
  return (
    <svg viewBox="0 0 579.76 76" height={height} width={(height * 579.76) / 76} role="img" aria-label="theuth">
      <path fill="currentColor" d="m 24.8,71.36 q 0.8,0 1.6,-0.48 0.8,-0.64 0.8,-1.92 V 15.68 H 6.4 Q 5.12,15.68 4,16.32 2.88,16.96 2.88,18.4 H 0 V 0 h 2.88 q 0,2.24 1.12,3.04 1.12,0.8 2.4,0.8 h 56.96 q 1.28,0 2.4,-0.8 1.12,-0.8 1.12,-3.04 h 2.88 v 18.4 h -2.88 q 0,-1.44 -1.12,-2.08 -1.12,-0.64 -2.4,-0.64 h -20.8 v 53.28 q 0,2.4 2.88,2.4 v 2.88 H 24.8 Z m 87.92,-2.4 q 0,1.28 0.64,1.92 0.8,0.48 1.76,0.48 v 2.88 H 94.96 v -2.88 q 0.96,0 1.6,-0.48 0.8,-0.64 0.8,-1.92 V 9.12 q 0,-1.28 -0.8,-1.76 -0.64,-0.64 -1.6,-0.64 V 3.84 h 20.16 v 2.88 q -0.96,0 -1.76,0.64 -0.64,0.48 -0.64,1.76 V 31.68 H 155.6 V 9.12 q 0,-1.28 -0.8,-1.76 -0.64,-0.64 -1.6,-0.64 V 3.84 h 20.16 v 2.88 q -0.96,0 -1.76,0.64 -0.64,0.48 -0.64,1.76 v 59.84 q 0,1.28 0.64,1.92 0.8,0.48 1.76,0.48 v 2.88 H 153.2 v -2.88 q 0.96,0 1.6,-0.48 0.8,-0.64 0.8,-1.92 V 44.8 H 112.72 Z M 268.47997,18.72 q 0,-3.04 -4.96,-3.04 h -39.04 v 16 h 24.64 q 2.72,0 4,-0.32 1.28,-0.48 1.28,-2.24 h 2.88 v 16.96 h -3.2 q 0.48,-1.76 -1.28,-2.08 -1.76,-0.48 -4.96,-0.48 h -23.36 V 62.4 h 40.96 q 1.92,0 3.2,-0.8 1.28,-0.96 1.28,-2.4 h 2.88 V 76 h -2.88 q 0,-1.44 -1.28,-1.6 -1.28,-0.16 -3.2,-0.16 h -58.72 v -2.88 q 0.8,0 1.6,-0.48 0.8,-0.64 0.8,-1.92 V 9.12 q 0,-1.28 -0.8,-1.76 -0.8,-0.64 -1.6,-0.64 V 3.84 h 56.8 q 4.96,0 4.96,-2.88 h 2.88 v 17.76 z m 54.79996,-12 q -3.52,0 -3.52,2.4 v 36.64 q 0,7.68 5.6,12.32 5.6,4.64 17.12,4.64 11.52,0 17.12,-4.64 5.6,-4.64 5.6,-12.32 V 9.12 q 0,-2.4 -4,-2.4 V 3.84 h 22.24 v 2.88 q -3.52,0 -3.52,2.4 V 49.6 q 0,4.96 -2.24,9.6 -2.24,4.48 -6.88,8 -4.48,3.52 -11.68,5.6 -7.04,1.92 -16.64,1.92 -9.6,0 -16.8,-1.92 -7.04,-2.08 -11.68,-5.6 -4.48,-3.52 -6.72,-8 -2.24,-4.64 -2.24,-9.6 V 9.12 q 0,-1.28 -0.96,-1.76 -0.96,-0.64 -3.68,-0.64 V 3.84 h 22.88 z M 431.1999,71.36 q 0.8,0 1.6,-0.48 0.8,-0.64 0.8,-1.92 V 15.68 h -20.8 q -1.28,0 -2.4,0.64 -1.12,0.64 -1.12,2.08 h -2.88 V 0 h 2.88 q 0,2.24 1.12,3.04 1.12,0.8 2.4,0.8 h 56.96 q 1.28,0 2.4,-0.8 1.12,-0.8 1.12,-3.04 h 2.88 v 18.4 h -2.88 q 0,-1.44 -1.12,-2.08 -1.12,-0.64 -2.4,-0.64 h -20.8 v 53.28 q 0,2.4 2.88,2.4 v 2.88 h -20.64 z m 87.92,-2.4 q 0,1.28 0.64,1.92 0.8,0.48 1.76,0.48 v 2.88 h -20.16 v -2.88 q 0.96,0 1.6,-0.48 0.8,-0.64 0.8,-1.92 V 9.12 q 0,-1.28 -0.8,-1.76 -0.64,-0.64 -1.6,-0.64 V 3.84 h 20.16 v 2.88 q -0.96,0 -1.76,0.64 -0.64,0.48 -0.64,1.76 v 22.56 h 42.88 V 9.12 q 0,-1.28 -0.8,-1.76 -0.64,-0.64 -1.6,-0.64 V 3.84 h 20.16 v 2.88 q -0.96,0 -1.76,0.64 -0.64,0.48 -0.64,1.76 v 59.84 q 0,1.28 0.64,1.92 0.8,0.48 1.76,0.48 v 2.88 h -20.16 v -2.88 q 0.96,0 1.6,-0.48 0.8,-0.64 0.8,-1.92 V 44.8 h -42.88 z" />
    </svg>
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
    note: "x64 and ARM64, Windows 11 or Windows 10 with WebView2. The installers are unsigned: SmartScreen says unknown publisher once; More info, Run anyway. They install per user under %LOCALAPPDATA%\\theuth with t.exe beside the app. The zips hold t.exe and its helpers; unzip anywhere.",
    files: [
      ["installer x64", "windows-x86_64-setup.exe"],
      ["installer ARM64", "windows-aarch64-setup.exe"],
      ["CLI zip x64", "windows-x86_64.zip"],
      ["CLI zip ARM64", "windows-aarch64.zip"],
    ],
  },
  linux: {
    label: "Linux",
    cmd: "brew update\nbrew install basedatum/tap/theuth",
    note: "x64 and ARM64, glibc 2.39 or newer: Ubuntu 24.04, Debian 13, Fedora 40 and later. The formula installs the CLI on both architectures; the desktop bundles include t beside the app. AppImage: chmod +x, then run. Debian: sudo apt install ./theuth_<version>_linux-<arch>.deb.",
    files: [
      ["AppImage x64", "linux-x86_64.AppImage"],
      ["AppImage ARM64", "linux-aarch64.AppImage"],
      [".deb x64", "linux-x86_64.deb"],
      [".deb ARM64", "linux-aarch64.deb"],
      [".rpm x64", "linux-x86_64.rpm"],
      [".rpm ARM64", "linux-aarch64.rpm"],
      ["CLI x64", "linux-x86_64.tar.gz"],
      ["CLI ARM64", "linux-aarch64.tar.gz"],
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
        <div className="p-5 font-mono text-[13px]">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <a
              href={artifact(version, active.files[0]![1])}
              className="btn btn-primary h-9 px-4 text-[13px]"
            >
              Download {active.files[0]![0]}
            </a>
            <a
              href={`${GITHUB}/releases`}
              className="text-[12px] text-fg-3 underline decoration-fg-3/60 underline-offset-4 hover:text-fg"
            >
              v{version} on GitHub Releases →
            </a>
          </div>
          {active.files.length > 1 && (
            <ul className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2.5 border-t border-well/60 pt-4">
              {active.files.slice(1).map(([label, target]) => (
                <li key={target}>
                  <a
                    href={artifact(version, target)}
                    className="text-fg-2 underline decoration-fg-3 underline-offset-4 hover:text-fg"
                  >
                    {label}
                  </a>
                </li>
              ))}
            </ul>
          )}
        </div>
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
  "t mcp",
  "t --link",
  "t --install-service",
  "t mesh pair",
  "t mesh add --vm",
  "t tasks list",
  "t --slack-setup",
  "t --link-signal",
  "t --telegram-setup",
];

/** Counts verified in code for this release (crates/browser, crates/computer); recount when they change. */
const TOOL_FAMILIES = [
  ["shell", "background jobs"],
  ["pty", "start, send, screen, wait"],
  ["files", "read, write, list, grep, glob, edit"],
  ["code", "find, outline, refs, symbol, impact, code_overview"],
  ["documents", "doc_read, insert, replace, delete, move"],
  ["notebooks", "notebook_*, source_*, cited answers"],
  ["timers", "remind_me_in, cancel_reminder"],
  ["browser", "eighteen tools, screenshots, your tile"],
  ["computer", "sixteen tools, the accessibility tree"],
  ["web", "search_web, read_url, search_x"],
  ["scrape", "scrape_url, crawl, map_site"],
  ["mcp", "any mcp.json, hot-reloaded"],
  ["routes", "site_learn, site_routes, site_call"],
  ["python", "state kept across turns"],
  ["tasks", "create, checkout, update, move, reassign, note, place"],
  ["agents", "agents_list, agent_draft, agent_put, agent_propose"],
  ["sandbox", "create, share, snapshot, restore, takeover"],
  ["share", "share_publish, yours to approve"],
];

const SURFACES: { group: string; doc: string; items: { name: string; href?: string; body: string }[] }[] = [
  {
    group: "At the machine",
    doc: "workspace/desktop",
    items: [
      { name: "t", body: "The terminal UI. The default." },
      {
        name: "theuth.app",
        body: "The desktop app on macOS, Windows and Linux. Chats are panes in one workspace with tiles beside them: a live web page, a terminal, a diff. The Inbox and the agent graph live here.",
      },
      {
        name: "t --acp",
        body: "Agent Client Protocol on stdio, agent name theuth. Zed and any other ACP client.",
      },
      {
        name: "t mcp",
        body: "theuth as an MCP server, for Claude Desktop, an IDE or another agent: 44 tools over your meetings, notes, knowledge, tasks and sessions. Off by default, loopback only, behind a token, every call in an audit log.",
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
        body: "t --slack-setup, then t --headless. One card per batch of tools, a plan message that updates in place, tables and charts as native blocks; /theuth carries the commands.",
      },
      {
        name: "Signal",
        body: "t --link-signal, then headless. Texts become turns in a per-contact session; approvals are answered with a thumbs-up.",
      },
      {
        name: "Telegram",
        body: "t --telegram-setup, then headless. The bot's chat gets a topic per session; replies stream as a live draft with a Stop button; approvals are Allow and Deny buttons.",
      },
    ],
  },
];

/** The doors heading counts SURFACES, so adding a door cannot leave it wrong again. */
const COUNT = ["None", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten", "Eleven", "Twelve"];
const DOORS = SURFACES.reduce((n, g) => n + g.items.length, 0);

const HANDS_BACK = [
  {
    name: "show",
    title: "Documents and pages",
    doc: "workspace/documents",
    body: "Markdown, CSV tables, images, SVG, Mermaid, diagrams, charts and PDFs compiled in-process. An HTML page renders live in the transcript, sandboxed and themed; the agent checks it for console errors before you see it. Every card is a versioned snapshot.",
  },
  {
    name: "app",
    title: "Screens you work in",
    doc: "workspace/apps",
    body: "A dashboard, a control panel, a form. The agent publishes an interactive app whose buttons read your installed tools live, without another model turn. The press is the approval. A reply can carry a static card inline; Open as app keeps it.",
  },
  {
    name: "show_web",
    title: "Tiles beside the chat",
    doc: "workspace/web-views",
    body: "The agent opens your dev server, a dashboard or a docs site in a real web view next to the conversation, or a live terminal, a note, a source file that follows its edits, a split diff. Sign in once in a tile and the agent's browser on any of your machines carries on signed in.",
  },
  {
    name: "notebooks",
    title: "Research with sources",
    doc: "workspace/notebooks",
    body: "Files, URLs, meetings, past sessions, Confluence, Jira, YouTube, audio and video as sources. A dial per item sets what the model sees; answers cite down to the block; embeddings run on a local model. Ask across notebooks, or ask for a briefing, a timeline or an audio overview.",
  },
  {
    name: "meetings",
    title: "Notes without a bot in the call",
    doc: "workspace/meetings",
    body: "theuth notices the app holding the microphone, asks before it records, matches the call to your calendar and writes a brief from the series' earlier meetings. Afterwards it suggests who spoke and writes the notes to your template.",
  },
  {
    name: "voice",
    title: "Read aloud, dictate, talk",
    doc: "workspace/voice",
    body: "Speech models run inside the harness, on your machine: replies read back, dictation into any app with your own dictionary, spoken commands and talk, a live conversation that keeps going while the work runs. Audio stays local; the text goes to the model provider you chose, as in any chat.",
  },
];

/** The agents and machines side: everything here ships; the ledger below has the detail. */
const TEAM = [
  {
    name: "agents",
    title: "Hire a standing role",
    doc: "automation/agents",
    body: "A title, a charter, who it reports to, a model, a tool ceiling, an MCP allowlist, an optional monthly budget and, if you want, a memory of its own that never mixes with yours. Agents can propose hires; those wait, paused, until you decide.",
  },
  {
    name: "tasks",
    title: "A board, not a transcript",
    doc: "automation/tasks",
    body: "Per project: backlog, todo, doing, review, blocked, done. You, theuth and its agents move the same cards, from chat or by dragging in the app; t tasks list shows the board from a shell. A task belongs to one agent on one machine, so two machines never work the same card.",
  },
  {
    name: "schedules",
    title: "Runs and loops",
    doc: "automation/schedules",
    body: "One markdown file per schedule with cron, every or at in the front matter and the prompt as the body. Add until or max_runs and it repeats until the job is done, each report delivered to you.",
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
    body: "spawn_subagent runs a child here, on another machine, or as Claude Code, Codex, OpenCode or omp. theuth shows their own subagents, steers a busy child, nudges a silent one and wakes when one stalls. A child's tool ceiling is the minimum over its ancestors.",
  },
  {
    name: "contacts",
    title: "Work with someone else's theuth",
    doc: "automation/contacts",
    body: "Pair with a colleague's theuth by a code you both confirm and export the agents and chats they may reach; your agents then message each other directly. A contact you add on one machine is a contact on all of them.",
  },
  {
    name: "inbox",
    title: "One queue, one map",
    doc: "workspace/desktop",
    body: "Every question, approval, notice and finished run lands in one Inbox, scored so what stops an agent comes first, answered on the card. Above it, a live graph of every session, subagent, harness child and contact, with the messages between them.",
  },
  {
    name: "mesh",
    title: "Several machines, one harness",
    doc: "automation/mesh",
    body: "Pair a laptop, a desktop and a server; sessions, agents, tasks, notes and the vault replicate. A session runs on one machine and you drive it from any other, or move it. The most stable engine holds the channels and schedule ticks.",
  },
  {
    name: "sandboxes",
    title: "A desktop that is not yours",
    doc: "automation/sandboxes",
    body: "t mesh add --vm boots an isolated Linux desktop microVM with the directories you share, joined to the mesh as an engine. Watch it in the app, take over, hand back, snapshot, throw away. Apple silicon, or Linux with KVM.",
  },
];

const SOON = ["iPhone app: paired over the encrypted link, Liquid Glass chrome, web-view tiles", "Android app"];
const NOT = [
  "Discord or WhatsApp",
  "Intel Mac builds",
  "A published Docker image: t mesh add --docker builds the engine container locally",
  "Custom domains for public shares; run your own edge on your own domain instead",
];
const LIMITS = [
  "Meetings are built for macOS 15; capture on Windows 11 and Linux with PipeWire exists but is partial and lightly exercised, without echo cancellation. Every device reads meetings.",
  "Echo-cancelled voice input uses macOS voice processing; speech on Windows and Linux is less exercised. Speech recognition on the Apple GPU is Apple silicon only.",
  "Sandbox VMs need Apple silicon or Linux with KVM, and a published VM image for your release; the image is pulled on first create. Read the sandboxes doc for what the current release provides before relying on them. t mesh add --ssh does not target Windows hosts.",
  "computer_* needs Screen Recording and Accessibility on macOS and an X11 display on Linux.",
  "A public share lives only while the machine serving it is awake and online: it is a tunnel, not hosting.",
  "Autoresearch study kinds beyond a single metric do not run on Windows.",
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
      ["Notebooks", "workspace/notebooks"],
      ["Web views", "workspace/web-views"],
      ["Knowledge", "workspace/knowledge"],
      ["Apps", "workspace/apps"],
      ["Search", "workspace/search"],
    ],
  },
  {
    title: "Automation",
    path: "automation",
    body: "Agents, tasks, schedules, other machines, other people, channels.",
    links: [
      ["Agents", "automation/agents"],
      ["Delegation", "automation/delegation"],
      ["Tasks", "automation/tasks"],
      ["Schedules", "automation/schedules"],
      ["Watchers", "automation/watchers"],
      ["Autoresearch", "automation/autoresearch"],
      ["Mesh", "automation/mesh"],
      ["Contacts", "automation/contacts"],
      ["Sharing", "automation/sharing"],
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
      ["Security", "reference/security"],
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
    // Follow newer GitHub Releases at runtime when reachable; while the repo is private (404) or rate-limited, keep __RELEASE_VERSION__.
    fetch(RELEASES_API, { headers: { Accept: "application/vnd.github+json" } })
      .then((r) => (r.ok ? (r.json() as Promise<{ tag_name?: string }>) : null))
      .then((j) => {
        const tag = j?.tag_name?.replace(/^v/, "");
        if (tag) setVersion(tag);
      })
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
          <Theta />
          <Wordmark />
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
            <h1 className="display text-[clamp(52px,10.5vw,128px)]">
              Your agents.
              <br />
              Your machines.
              <br />
              Your keys.
            </h1>
            <p className="mt-8 max-w-[28em] text-[18px] leading-relaxed text-fg-2">
              theuth directs any model and any coding agent on computers you
              own. It shares the work and keeps the keys.
            </p>
            <div className="mt-10 flex flex-wrap gap-3">
              <a href="#install" className="btn btn-primary">
                Install
              </a>
              <a href={artifact(version, OSES[os].files[0]![1])} className="btn">
                Download for {OSES[os].label}
              </a>
            </div>
            <p className="mt-6 flex flex-wrap items-center gap-x-6 text-[14px] text-fg-3">
              <span>
                New here?{" "}
                <Doc path="getting-started" className="docs-cta">
                  Read the quickstart →
                </Doc>
              </span>
              <a href="#keys" className="docs-link docs-cta">
                How the keys stay home →
              </a>
            </p>
          </div>
          <div className="relative hidden min-h-[60vh] md:col-span-5 md:block">
            <div className="absolute top-0 right-0 text-fg-2">
              <Theta size={40} />
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
              It runs the coding agents.
            </h2>
            <p className="mt-6 max-w-[34em] text-[17px] leading-relaxed text-fg-2">
              theuth is not one more coding agent. It briefs Claude Code, Codex,
              OpenCode and omp as children over the Agent Client Protocol,
              watches their own subagents, steers a busy one, nudges a silent one
              and wakes when one stalls, then reads the diff before it reports
              back. Around them it does the rest of the work: shell jobs, a real
              browser, your desktop, files and the web. Mutating and destructive
              tools ask first.
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
                t is the daemon, the terminal UI, the headless server, the
                editor agent and an MCP server. Plugins are Rust crates on a
                typed message bus. Every frontend attaches over JSON-RPC: the
                TUI, the desktop app, a phone through the relay, Slack, Signal,
                Telegram, your editor. One binary, one data directory, one set
                of approvals.
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
                is real, with screenshots; the desktop is read through the
                accessibility tree. Every tool is also a function inside a
                Python runtime that keeps its state.
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
                card in the TUI or the app, Allow and Deny in Slack or Telegram, a
                thumbs-up on Signal. Some tools are human-only: publishing to
                the web waits for you, in person. Any builtin category can be
                off for one chat or for good.
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
                what is worth keeping while you work, and a local model checks
                each memory against the question before it costs a token.
                Sessions live in sqlite; shake images and tool output out of the
                context; search everything you ever showed it.
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
                wss://relay.theuth.io; the link is end-to-end encrypted with
                Noise and nothing listens on your machine. Other machines join
                one leaderless mesh: t mesh pair, or a nearby code both screens
                confirm, then t mesh add with --ssh for a host you can reach,
                --docker for a container here, --vm for an isolated Linux
                desktop. Sessions, agents, tasks, notes and the vault replicate;
                drive a chat from any machine, or move it.
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
                  you have. Put API keys in the vault, OpenCode Zen, OpenCode Go
                  and OpenRouter among them. Use Bedrock through an AWS profile,
                  SSO included, with no credentials in the UI. Add any
                  OpenAI-compatible gateway, or a local server (Ollama, LM
                  Studio, omlx, llama.cpp, vLLM), probed for its models and limits.
                  A text-only model can borrow a vision model's eyes when an
                  image comes in.
                </p>
                <Doc path="getting-started/models" className="docs-cta mt-6">
                  Docs: models and logins →
                </Doc>
              </div>
            </li>
          </ol>
        </section>

        <Keys />

        <Publish />

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
                    Docs: {h.doc.split("/").pop()!.replace("-", " ")} →
                  </Doc>
                </div>
              </li>
            ))}
          </ul>
        </section>

        <Better />

        <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
          <h2 className="display text-[clamp(44px,5.4vw,76px)]">
            It runs a team.
          </h2>
          <p className="mt-6 max-w-[34em] text-[17px] leading-relaxed text-fg-2">
            Agents you hire, a board they work from, routines that wake them,
            the coding agents they direct, the people they work with and the
            machines they run on. One Inbox and one set of approvals over all
            of it.
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
            {COUNT[DOORS]} doors, one daemon.
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
            <Theta />
            <Wordmark />
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
            <span>tun.zip</span>
          </nav>
        </div>
      </footer>
    </>
  );
}