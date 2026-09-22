import { useEffect, useState } from "react";
import engraving from "./engraving.webp";
import { Pyramid } from "./Pyramid";

declare const __APP_VERSION__: string;

const GITHUB = "https://github.com/skymoore/theuth";
const APP = "https://app.theuth.io";
const RELEASES = "https://s3.rwx.dev/theuth/releases";
const dmg = (v: string) => `${RELEASES}/${v}/theuth_${v}_macos-aarch64.dmg`;

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

const TABS = [
  {
    id: "macos",
    label: "macOS arm64",
    cmd: "brew update\nbrew install basedatum/tap/theuth",
    note: "The CLI: t, the daemon and the terminal UI in one binary. It keeps itself current.",
  },
  {
    id: "linux",
    label: "Linux",
    cmd: "brew update\nbrew install basedatum/tap/theuth",
    note: "Tarballs exist for linux-x86_64 and linux-aarch64. glibc 2.39 or newer: Ubuntu 24.04, Debian 13, Fedora 40 and later.",
  },
  {
    id: "app",
    label: "macOS app",
    cmd: "brew update\nbrew install --cask basedatum/tap/theuth",
    note: "The cask lists Monterey and later; the bundle itself wants macOS 15. Apple silicon only.",
  },
] as const;

function Install({ dmgUrl }: { dmgUrl: string }) {
  const [tab, setTab] = useState<(typeof TABS)[number]["id"]>("macos");
  const [copied, setCopied] = useState(false);
  const active = TABS.find((t) => t.id === tab) ?? TABS[0];

  const copy = () => {
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
          {TABS.map((t) => (
            <button
              key={t.id}
              role="tab"
              aria-selected={t.id === tab}
              onClick={() => setTab(t.id)}
              className={
                t.id === tab
                  ? "bg-raised py-3 text-fg shadow-[inset_0_-2px_0_var(--accent)] transition-colors duration-[120ms]"
                  : "py-3 text-fg-3 transition-colors duration-[120ms] hover:text-fg-2"
              }
            >
              {t.label}
            </button>
          ))}
        </div>
        <div className="flex items-start justify-between gap-4 p-5">
          <pre className="font-mono text-[13px] leading-6 text-fg">
            {active.cmd.split("\n").map((line) => (
              <div key={line}>
                <span className="select-none text-fg-3">$ </span>
                {line}
              </div>
            ))}
          </pre>
          <button
            onClick={copy}
            className="btn h-8 px-3 text-[12px]"
            aria-live="polite"
          >
            {copied ? "copied" : "copy"}
          </button>
        </div>
        <p className="border-t border-well px-5 py-3 text-[13px] leading-relaxed text-fg-3">
          {active.note}
          {tab === "app" && (
            <>
              {" "}
              Or{" "}
              <a
                href={dmgUrl}
                className="text-fg-2 underline decoration-fg-3 underline-offset-4 hover:text-fg"
              >
                download the disk image
              </a>
              .
            </>
          )}
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
    </section>
  );
}

const FLAGS = [
  "t",
  "t --headless",
  "t --acp",
  "t --link",
  "t --install-service",
  "t --slack-setup",
  "t --link-signal",
];

const TOOL_FAMILIES = [
  ["shell", "background jobs"],
  ["pty", "start, send, screen, wait"],
  ["files", "read, write, list, grep, edit"],
  ["browser", "sixteen tools, screenshots"],
  ["computer", "eight tools, your desktop"],
  ["web", "search_web, read_url"],
  ["scrape", "scrape_url, crawl, map_site"],
  ["mcp", "any mcp.json, hot-reloaded"],
  ["python", "state kept across turns"],
];

const SURFACES = [
  {
    group: "At the machine",
    items: [
      { name: "t", body: "The terminal UI. The default." },
      {
        name: "theuth.app",
        body: "macOS menubar app, Apple silicon. Looks like a chat page and speaks JSON-RPC to the daemon. A globe in the header opens a live view of the browser the agent is driving.",
      },
      {
        name: "t --acp",
        body: "Agent Client Protocol on stdio, agent name theuth. Zed and any other ACP client.",
      },
    ],
  },
  {
    group: "Away from it",
    items: [
      {
        name: "app.theuth.io",
        href: APP,
        body: "Your phone or any browser, paired with a QR through the relay. Nothing listens on your machine; the relay sees ciphertext only.",
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
    body: "Markdown, CSV tables, images, SVG, and PDFs compiled in-process from markdown or Typst. Every card is a versioned snapshot; the model gets the first page back as an image and fixes what it sees.",
  },
  {
    name: "app",
    title: "Screens you work in",
    body: "A dashboard, a control panel, a form. The agent publishes an interactive app whose buttons read your installed tools live, without another model turn. The press is the approval.",
  },
  {
    name: "schedule",
    title: "Runs and loops",
    body: "One markdown file per schedule with cron, every, or at in the front matter and the prompt as the body. Add until or max_runs and it repeats until the job is done, each report delivered to you.",
  },
  {
    name: "voice",
    title: "Read aloud, voice notes, dictation",
    body: "Pocket TTS, Parakeet and Nemotron streaming speech models run inside the harness, on your machine. Nothing leaves it.",
  },
];

/** The whole ledger: what the tool is, what it lets you do, why that matters. Facts from the README only. */
const LEDGER: { group: string; items: [string, string, string][] }[] = [
  {
    group: "It acts",
    items: [
      [
        "shell",
        "Runs commands as background jobs. Anything that finishes in three seconds answers inline; anything longer hands back a job id and wakes the model when it exits.",
        "Start the build, walk away, and the result finds you. Nothing blocks on a slow command.",
      ],
      [
        "pty",
        "Starts a program on a pseudo-terminal and drives it: type into it, read the rendered 120 by 40 screen, wait for a prompt or a port.",
        "REPLs, dev servers, installers and TUIs, not just one-shot commands.",
      ],
      [
        "files",
        "Read, write, list, grep and edit on the machine it runs on.",
        "The unglamorous majority of real work.",
      ],
      [
        "browser",
        "Sixteen tools on an embedded headless browser: open, read, click, fill, type, select, scroll, eval, screenshot, tabs. Stealth TLS fingerprint, tracker blocklist, optional proxy, timezone and geolocation. With import_cookies on it loads your real browser profile and acts as the signed-in you.",
        "Sites with no API. Forms, dashboards, portals. Watch it live from the app's globe button.",
      ],
      [
        "computer",
        "One screenshot of every display, then native mouse and keyboard on your own desktop. macOS asks for Screen Recording and Accessibility once.",
        "Apps with no CLI, no API and no web page still get done.",
      ],
      [
        "web",
        "search_web fans out to Google, Bing, Brave, Qwant, DuckDuckGo, Wikipedia, arXiv and GitHub without a key, merges by reciprocal rank, then reads the top hits and returns the passages that answer the query. read_url fetches with an SSRF guard.",
        "No search API subscription. One call replaces search, click, skim.",
      ],
      [
        "scrape",
        "scrape_url, crawl and map_site on a vendored crawler: stays under the seed, honours robots.txt, seeds from sitemaps, two engine tiers when a site needs a real browser.",
        "A whole documentation site or catalogue in context, politely.",
      ],
      [
        "python",
        "A persistent runtime where every tool is an async function and state survives across turns. The worker is the t binary itself; nothing to install.",
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
        "ask_user",
        "Puts one or more multiple-choice questions to you: pick, type your own, skip one or dismiss. Works in the TUI, the app, a Slack thread and over Signal.",
        "Fewer confident wrong guesses on the decisions that matter.",
      ],
    ],
  },
  {
    group: "It hands back",
    items: [
      [
        "show",
        "Files the agent makes for you appear as cards: markdown, code, CSV tables, images, SVG, and PDFs compiled in-process from markdown or Typst. Every card is an immutable versioned snapshot.",
        "The transcript never lies about what you were shown. Copy, save, open, reveal.",
      ],
      [
        "app",
        "Interactive screens in OpenUI Lang: a dashboard, a control panel, a form. Buttons and inputs bind to your installed tools and run them live, without another model turn. The press is the approval.",
        "A tool the agent builds once and you use for months.",
      ],
      [
        "schedule",
        "One markdown file per schedule with cron, every or at in the front matter and the prompt as the body. Add until or max_runs and it loops until done; approvals: false for unattended runs.",
        "Morning reports, nightly checks, a job that retries itself until the test passes.",
      ],
      [
        "voice",
        "Read aloud with Pocket TTS, voice notes with Parakeet, live dictation with Nemotron. All three run inside the harness on your machine.",
        "Talk to it and hear it back, with nothing sent to a speech API.",
      ],
      [
        "knowledge",
        "Durable facts about your machines, projects, accounts and decisions as plain markdown concepts under ~/.theuth/knowledge, full-text indexed, every write a git commit made in-process. Harvested automatically every few tool-using turns; surfaced as hints at the start of a turn.",
        "It remembers your setup across sessions, and you can read, edit, diff and restore every word of it in any editor.",
      ],
      [
        "search",
        "One fuzzy search over sessions, shown artifacts, current knowledge and knowledge history. Command K in the app, /search in the TUI, and the same tool for the model.",
        "Find the deployment plan you were shown last week. A paired phone gets results, never the corpus.",
      ],
      [
        "skills",
        "SKILL.md directories, loose markdown or git clones under ~/.theuth/skills; /skills add a URL and /skills sync pulls. A harness.md manual of this install is written on every start.",
        "Your procedures, loaded on demand instead of pasted every time.",
      ],
      [
        "subagents",
        "spawn_subagent runs a child session, on this machine or a node. External harnesses in acp/agents.yaml run the same way. Each Slack subagent gets its own thread with a live board in the parent.",
        "Fan out, delegate, and keep the approvals in one place. Off until agent.communication is on.",
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
    items: [
      [
        "approvals",
        "read_only runs, mutating asks, destructive asks. The card lands in the thread you are in. /approvals off for a session, or for good.",
        "It never bites without asking, and the dial is yours.",
      ],
      [
        "secrets",
        "An encrypted, versioned vault; the key lives in the OS keychain. The model uses a secret as {{secret:NAME}} in any tool argument and the value is swapped in at run time and scrubbed from output. secret_request opens a masked box for you to type into.",
        "API keys, tokens and passwords the model can use but never read or leak.",
      ],
      [
        "context",
        "Compact by summary, cut or snap at a threshold you set. /undo and /redo a turn. /shake images, documents or tool output out of context without losing what came after. /edit a message and replay. Type while it works to queue; interject to redirect now.",
        "Long sessions stay cheap and steerable instead of restarting from zero.",
      ],
      [
        "models",
        "Sign in with the OpenAI, Anthropic, Cursor or xAI subscription you have. Keys in the vault. Bedrock through an AWS profile with SSO, no credentials in the UI. Any OpenAI-compatible gateway or a local model through models.json. Per-model effort levels.",
        "Use what you already pay for. Switch with /model mid-session.",
      ],
      [
        "vision fallback",
        "Set agent.vision_model and a text-only main model still handles images: it writes a question, an image-capable model answers, the answer joins the turn.",
        "Cheap or local models keep working when a screenshot comes in.",
      ],
      [
        "failures",
        "Denied model access, expired AWS sessions and stalled streams stay in the transcript with guidance. They are never fed to the model as conversation. Interrupted replies are salvaged and resumed.",
        "You see what went wrong; the model does not hallucinate around it.",
      ],
      [
        "update",
        "Signed release manifest, silent verification, binary replaced on disk and live at the next start. t --update forces a check.",
        "Always current, no reinstall, no unsigned code.",
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
    items: [
      [
        "link",
        "Pair a phone or any browser with a QR through the relay. The uplink dials out; nothing listens on your machine. Noise end-to-end encryption; the relay keeps no state and no accounts and sees ciphertext only. Devices listed, renamed and revoked from /link. Self-host the relay with one compose file.",
        "Drive the harness from a phone without opening a port.",
      ],
      [
        "nodes",
        "On another box: install, set a key, t --install-service. It writes a launchd or systemd user unit through your login shell, turns link on and prints a pairing key. /nodes add on your main harness.",
        "Give the model, and your phone, another machine. Subagents run there; approvals land in front of you.",
      ],
      [
        "slack",
        "t --slack-setup creates the app from a manifest and verifies the tokens. DMs and mentions open sessions that stream into threads with a card per tool call, Allow and Deny buttons, and /theuth for the commands.",
        "theuth agent, in your workspace, with the same approvals.",
      ],
      [
        "signal",
        "t --link-signal pairs the daemon as a linked device. Texts become turns in a per-contact session; eyes while working, a check when done; approvals answered with a thumbs-up; files land in an inbox.",
        "The agent in your pocket over the messenger you already trust.",
      ],
      [
        "acp",
        "t --acp speaks the Agent Client Protocol on stdio, agent name theuth.",
        "Zed and any other ACP editor, no plugin.",
      ],
      [
        "theuth.app",
        "A macOS menubar app: sessions with the subagent tree, streamed markdown with tool cards, inline approvals, attachments, a settings dialog for every connection, a knowledge explorer with history and a raw editor, artifact and app panels, a live view of the agent's browser. No Dock icon; the tray or Option Space toggles it. Reuses a running daemon or starts one through your login shell.",
        "Everything the terminal does, as a chat window that is always one keystroke away.",
      ],
      [
        "t",
        "The terminal UI: slash commands for everything above, Ctrl V pastes an image, drop a file to attach it, mouse selection is terminal-native, the history stays put while a reply streams.",
        "One binary. Works over ssh.",
      ],
    ],
  },
];

const SOON = ["Windows app", "Linux GUI", "Telegram", "iOS and Android apps"];
const NOT = ["Discord", "WhatsApp", "docker run of the harness", "Intel Macs"];

export function App() {
  const [dmgUrl, setDmgUrl] = useState(dmg(__APP_VERSION__));
  useEffect(() => {
    // latest.json is the only mutable object in the bucket; when it is reachable, follow it instead of the build-time version.
    fetch(`${RELEASES}/latest.json`)
      .then((r) => r.json() as Promise<{ version: string }>)
      .then(({ version }) => setDmgUrl(dmg(version)))
      .catch(() => {});
  }, []);

  return (
    <>
      <Pyramid />
      <header className="sticky top-0 z-10 flex h-16 items-center justify-between border-b border-well/60 bg-page/80 px-6 backdrop-blur-sm md:px-10">
        <a
          href="#"
          className="flex items-center gap-2.5 text-[15px] font-medium"
        >
          <Ibis />
          theuth
        </a>
        <nav className="flex items-center gap-6 text-[14px] text-fg-2">
          <a href={GITHUB} className="hover:text-fg">
            github
          </a>
          <a href={APP} className="hover:text-fg">
            app
          </a>
          <a href="#install" className="btn h-9 px-4 text-[13px]">
            Install
          </a>
        </nav>
      </header>

      <main>
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
              <a href={dmgUrl} className="btn">
                Download for macOS
              </a>
            </div>
          </div>
          <div className="relative hidden min-h-[60vh] md:col-span-5 md:block">
            <div className="absolute top-0 right-0 text-fg-2">
              <Ibis size={40} />
            </div>
          </div>
        </section>

        <Install dmgUrl={dmgUrl} />

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
              open. It asks before anything bites.
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
            </li>
            <li className="bg-page/85 p-7 backdrop-blur-[2px] md:col-span-4 md:p-9">
              <p className="index">03</p>
              <h2 className="mt-3 text-[22px] font-medium">
                Ask before it bites.
              </h2>
              <p className="mt-4 text-[15px] leading-relaxed text-fg-2">
                Read-only tools run. Mutating and destructive ones ask, in the
                thread you are in: a card in the TUI or the app, Allow and Deny
                in Slack, a thumbs-up on Signal. Secrets live in an encrypted
                vault; the model uses them by name and never sees a value.
              </p>
              <pre className="mt-7 border border-well bg-well/60 p-4 font-mono text-[12.5px] leading-6 text-fg-2">
                {
                  "tools:\n  policy:\n    read_only:   allow\n    mutating:    ask\n    destructive: ask"
                }
              </pre>
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
            </li>
            <li className="bg-page/85 p-7 backdrop-blur-[2px] md:col-span-4 md:p-9">
              <p className="index">05</p>
              <h2 className="mt-3 text-[22px] font-medium">Other machines.</h2>
              <p className="mt-4 text-[15px] leading-relaxed text-fg-2">
                Pair a phone or any browser with a QR through
                wss://relay.theuth.io. Nothing listens on your machine; the link
                is end-to-end encrypted with Noise, and the relay keeps no state
                and no accounts. Run t --install-service on another box and it
                becomes a node: subagents run there, approvals land in front of
                you. Model-to-model talk stays off until agent.communication is
                on.
              </p>
            </li>
            <li className="bg-page/85 p-7 backdrop-blur-[2px] md:col-span-12 md:grid md:grid-cols-12 md:gap-10 md:p-9">
              <div className="md:col-span-5">
                <p className="index">06</p>
                <h2 className="mt-3 text-[22px] font-medium">
                  Any model you already pay for.
                </h2>
              </div>
              <p className="mt-4 text-[15px] leading-relaxed text-fg-2 md:col-span-7 md:mt-0">
                Sign in with the OpenAI, Anthropic, Cursor or xAI subscription
                you have. Put API keys in the vault. Use Bedrock through an AWS
                profile, SSO included, with no credentials in the UI. Add any
                OpenAI-compatible gateway, or a local model. A text-only model
                can borrow a vision model's eyes when an image comes in.
              </p>
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
                </div>
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
            The whole ledger. Each line is a tool or a switch that ships in
            1.7.0, what it lets you do, and why you would care.
          </p>
          <div className="mt-14 border border-well bg-page/85 backdrop-blur-[2px]">
            {LEDGER.map((g) => (
              <div
                key={g.group}
                className="border-t border-well first:border-t-0 md:grid md:grid-cols-12"
              >
                <h3 className="index p-7 md:col-span-2 md:p-9">{g.group}</h3>
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
              <p className="mt-5 text-[14px] text-fg-3">
                Meetings need macOS 15.
              </p>
            </div>
          </div>
        </section>
      </main>

      <footer className="border-t border-well/60 bg-page/80 px-6 py-10 backdrop-blur-sm md:px-10">
        <div className="mx-auto flex max-w-[1400px] flex-wrap items-center justify-between gap-6">
          <a
            href="#"
            className="flex items-center gap-2.5 text-[15px] font-medium"
          >
            <Ibis />
            theuth
          </a>
          <nav className="flex flex-wrap gap-x-8 gap-y-2 font-mono text-[13px] text-fg-3">
            <a href={GITHUB} className="hover:text-fg">
              github.com/skymoore/theuth
            </a>
            <a href={APP} className="hover:text-fg">
              app.theuth.io
            </a>
            <span>relay.theuth.io</span>
          </nav>
        </div>
      </footer>
    </>
  );
}
