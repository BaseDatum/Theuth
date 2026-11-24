import { useEffect, useState, type ReactNode } from "react";
import engraving from "./engraving.webp";
import { Pyramid } from "./Pyramid";
import { LEDGER } from "./ledger";

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
  "t --telegram-setup",
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
      {
        name: "Telegram",
        body: "t --telegram-setup, then headless. The bot's chat gets a topic per session; replies stream as a live draft with a Stop button; approvals are Allow and Deny buttons.",
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
    body: "Engines pair with a short string and find each other on the LAN, through NAT or through the relay. Agents, tasks, schedules and sessions replicate; the most stable engine holds Slack, Signal, Telegram, schedule ticks and desks, and hands them over when it goes away.",
  },
  {
    name: "sandboxes",
    title: "A desktop that is not yours",
    doc: "automation/sandboxes",
    body: "t mesh add --vm boots an isolated Linux desktop microVM with the directories you share, joined to the mesh as an engine. Watch it in the app, take over, hand back, snapshot, throw away. Apple silicon, or Linux with KVM.",
  },
];

const SOON = ["iOS app: a paired client over the link, TestFlight pipeline in the repo", "Android app"];
const NOT = [
  "Discord or WhatsApp",
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
                phone through the relay, Slack, Signal, Telegram, your editor. One
                binary, one data directory, one set of approvals.
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
                card in the TUI or the app, Allow and Deny in Slack or Telegram, a
                thumbs-up on Signal. The class is the tool's, not the effect's: a browser
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
