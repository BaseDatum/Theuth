import { useEffect, useState } from 'react'
import { Pyramid } from './Pyramid'

declare const __APP_VERSION__: string

const GITHUB = 'https://github.com/skymoore/theuth'
const APP = 'https://app.theuth.io'
const RELEASES = 'https://s3.rwx.dev/theuth/releases'
const dmg = (v: string) => `${RELEASES}/${v}/theuth_${v}_macos-aarch64.dmg`

/** The brand mark: compact skull, one disk for an eye, a beak that tapers into a reed-pen nib. Bone on soot, soot on bone. */
function Ibis({ size = 22 }: { size?: number }) {
  return (
    <svg viewBox="60 60 904 904" width={size} height={size} aria-hidden>
      <g fill="currentColor">
        <circle cx="640" cy="340" r="165" />
        <path d="M 660 178 C 400 196, 230 470, 186 846 L 244 890 C 300 570, 400 380, 600 500 Z" />
      </g>
      <circle cx="610" cy="288" r="36" className="fill-accent" />
    </svg>
  )
}

/** Tiny beak stroke: the surfaces' index mark. */
function Beak() {
  return (
    <svg viewBox="180 170 500 730" width="8" height="12" aria-hidden className="text-fg-3">
      <path fill="currentColor" d="M 660 178 C 400 196, 230 470, 186 846 L 244 890 C 300 570, 400 380, 600 500 Z" />
    </svg>
  )
}

const TABS = [
  {
    id: 'macos',
    label: 'macOS arm64',
    cmd: 'brew update\nbrew install basedatum/tap/theuth',
    note: null,
  },
  {
    id: 'linux',
    label: 'Linux',
    cmd: 'brew update\nbrew install basedatum/tap/theuth',
    note: 'Tarballs exist for linux-x86_64 and linux-aarch64. glibc 2.35+: Ubuntu 22.04+, Debian 12+, Fedora 36+.',
  },
  {
    id: 'app',
    label: 'macOS app',
    cmd: 'brew update\nbrew install --cask basedatum/tap/theuth',
    note: 'Monterey+ in the cask; the bundle’s minimumSystemVersion is 15.0. Arm64 only.',
  },
] as const

function Install({ dmgUrl }: { dmgUrl: string }) {
  const [tab, setTab] = useState<(typeof TABS)[number]['id']>('macos')
  const [copied, setCopied] = useState(false)
  const active = TABS.find((t) => t.id === tab) ?? TABS[0]

  const copy = () => {
    navigator.clipboard.writeText(active.cmd).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1400)
    })
  }

  return (
    <section id="install" className="mx-auto max-w-3xl scroll-mt-24 px-6 pb-32">
      <div className="border border-well bg-raised">
        <div role="tablist" className="grid grid-cols-3 border-b border-well bg-well text-[13px] font-medium">
          {TABS.map((t) => (
            <button
              key={t.id}
              role="tab"
              aria-selected={t.id === tab}
              onClick={() => setTab(t.id)}
              className={
                t.id === tab
                  ? 'bg-raised py-3 text-fg shadow-[inset_0_-2px_0_var(--accent)] transition-colors duration-[120ms]'
                  : 'py-3 text-fg-3 transition-colors duration-[120ms] hover:text-fg-2'
              }
            >
              {t.label}
            </button>
          ))}
        </div>
        <div className="flex items-start justify-between gap-4 p-5">
          <pre className="font-mono text-[13px] leading-6 text-fg">
            {active.cmd.split('\n').map((line) => (
              <div key={line}>
                <span className="select-none text-fg-3">$ </span>
                {line}
              </div>
            ))}
          </pre>
          <button onClick={copy} className="btn h-8 px-3 text-[12px]" aria-live="polite">
            {copied ? 'copied' : 'copy'}
          </button>
        </div>
        {active.note && <p className="border-t border-well px-5 py-3 text-[13px] text-fg-3">{active.note}</p>}
        {tab === 'app' && (
          <p className="border-t border-well px-5 py-3 text-[13px] text-fg-3">
            Or{' '}
            <a href={dmgUrl} className="text-fg-2 underline decoration-fg-3 underline-offset-4 hover:text-fg">
              download the .dmg
            </a>
            .
          </p>
        )}
      </div>
      <dl className="mt-8 grid gap-x-8 gap-y-3 font-mono text-[13px] text-fg-3 sm:grid-cols-2">
        <div className="flex gap-3">
          <dt className="text-fg">t</dt>
          <dd>opens the TUI</dd>
        </div>
        <div className="flex gap-3">
          <dt className="text-fg">t --update</dt>
          <dd>checks updates</dd>
        </div>
        <div className="flex gap-3">
          <dt className="text-fg">~/.theuth</dt>
          <dd>default data dir</dd>
        </div>
        <div className="flex gap-3">
          <dt className="text-fg">wss://relay.theuth.io</dt>
          <dd>default relay</dd>
        </div>
      </dl>
    </section>
  )
}

const SURFACES = [
  { name: 't', body: 'TUI. Default.' },
  { name: 'theuth.app', body: 'macOS menubar, arm64. Looks like a chat page. JSON-RPC to the daemon.' },
  {
    name: 'phone / browser',
    body: 'Pair through the relay; no open port. iOS 17+ TestFlight app is the link-device SPA, not a local daemon.',
    href: APP,
  },
  { name: 'Slack', body: 't --headless after t --slack-setup. /theuth. theuth agent, in your workspace.' },
  { name: 'Signal', body: 't --link-signal then headless.' },
  { name: 'editors', body: 't --acp, agent name theuth.' },
]

const FEATURES = [
  {
    title: 'One process.',
    body: 'The daemon is t. Plugins are Rust crates on a typed bus. Any frontend attaches over JSON-RPC. Not a coding agent.',
  },
  {
    title: 'Tools that act.',
    body: 'Shell, PTY jobs, files, browser isolate, real desktop (macOS TCC), web/scrape/routes, knowledge store, schedules, secrets vault, artifacts and clickable apps.',
  },
  {
    title: 'Ask before it bites.',
    body: 'Policy is read_only: allow, mutating: ask, destructive: ask. Approvals live in the thread.',
  },
  {
    title: 'Memory as marks.',
    body: 'Knowledge is markdown + FTS + history under ~/.theuth/knowledge. Sessions in sqlite. Compaction is summary, cut, or snap.',
  },
  {
    title: 'Other machines.',
    body: 'Link a phone through wss://relay.theuth.io. Nodes: another box running t --install-service. Subagents only if agent.communication is on.',
  },
  {
    title: 'Any model you already pay for.',
    body: 'Default anthropic/claude-sonnet-4-5. Logins for OpenAI, Anthropic, Cursor, xAI. Keys in the vault. Bedrock via AWS profile, not keys in the UI.',
  },
]

const NOT = [
  'No Windows app.',
  'No Linux GUI.',
  'No Discord, Telegram, or WhatsApp.',
  'No docker run of the harness.',
  'No Intel Mac.',
  'Meetings are macOS 15 only.',
]

export function App() {
  const [dmgUrl, setDmgUrl] = useState(dmg(__APP_VERSION__))
  useEffect(() => {
    // latest.json is the only mutable object in the bucket; when it is reachable, follow it instead of the build-time version.
    fetch(`${RELEASES}/latest.json`)
      .then((r) => r.json() as Promise<{ version: string }>)
      .then(({ version }) => setDmgUrl(dmg(version)))
      .catch(() => {})
  }, [])

  return (
    <>
      <Pyramid />
      <header className="sticky top-0 z-10 flex h-16 items-center justify-between border-b border-well/60 bg-page/80 px-6 backdrop-blur-sm">
        <a href="#" className="flex items-center gap-2.5 text-[15px] font-medium">
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
            install
          </a>
        </nav>
      </header>

      <main>
        <section className="mx-auto grid min-h-[calc(100vh-4rem)] max-w-7xl grid-cols-1 items-center gap-12 px-6 py-20 md:grid-cols-[minmax(0,7fr)_minmax(0,5fr)]">
          <div>
            <p className="index mb-8">what the harness</p>
            <h1 className="display text-[clamp(72px,11vw,120px)]">
              The harness
              <br />
              that does
              <br />
              things
            </h1>
            <p className="mt-8 max-w-[28em] text-[18px] leading-relaxed text-fg-2">
              A single-process AI harness that does things for the user.
            </p>
            <p className="mt-3 max-w-[28em] text-[14px] text-fg-3">
              A single-process Rust harness. Plugins over a typed bus. Frontends attach over JSON-RPC.
            </p>
            <div className="mt-10 flex flex-wrap gap-3">
              <a href="#install" className="btn btn-primary">
                install via terminal
              </a>
              <a href={dmgUrl} className="btn">
                download for macOS
              </a>
            </div>
          </div>
          <div className="relative hidden min-h-[60vh] md:block">
            <div className="absolute top-0 right-0 text-fg-2">
              <Ibis size={40} />
            </div>
          </div>
        </section>

        <Install dmgUrl={dmgUrl} />

        <section className="mx-auto max-w-6xl px-6 pb-32">
          <p className="index mb-8">surfaces</p>
          <ul className="grid gap-px border border-well bg-well sm:grid-cols-2 lg:grid-cols-3">
            {SURFACES.map((s) => (
              <li key={s.name} className="bg-page/85 p-6 backdrop-blur-[2px]">
                <div className="flex items-center gap-2 font-mono text-[13px] text-fg">
                  <Beak />
                  {s.href ? (
                    <a href={s.href} className="underline decoration-fg-3 underline-offset-4 hover:text-fg-2">
                      {s.name}
                    </a>
                  ) : (
                    s.name
                  )}
                </div>
                <p className="mt-3 text-[14px] leading-relaxed text-fg-3">{s.body}</p>
              </li>
            ))}
          </ul>
          <div className="mt-6 flex flex-wrap items-center justify-between gap-4 border border-well bg-raised px-6 py-4 text-[14px] text-fg-3">
            <span>iOS and Android apps: coming soon.</span>
            <button disabled className="btn h-9 px-4 text-[13px]">
              join the testing group · coming soon
            </button>
          </div>
        </section>

        <section className="mx-auto max-w-6xl px-6 pb-32">
          <ol className="grid gap-x-12 gap-y-14 md:grid-cols-2 lg:grid-cols-3">
            {FEATURES.map((f, i) => (
              <li key={f.title}>
                <p className="index">{String(i + 1).padStart(2, '0')}</p>
                <h2 className="mt-3 text-[20px] font-medium text-fg">{f.title}</h2>
                <p className="mt-3 text-[15px] leading-relaxed text-fg-2">{f.body}</p>
              </li>
            ))}
          </ol>
        </section>

        <section className="mx-auto max-w-6xl px-6 pb-32">
          <div className="max-w-xs">
            <p className="index mb-6">what it is not</p>
            <ul className="space-y-2 text-[14px] text-fg-3">
              {NOT.map((n) => (
                <li key={n}>{n}</li>
              ))}
            </ul>
          </div>
        </section>
      </main>

      <footer className="border-t border-well/60 bg-page/80 px-6 py-8 font-mono text-[13px] text-fg-3 backdrop-blur-sm">
        <div className="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-3">
          <span>
            theuth {__APP_VERSION__} · t ·{' '}
            <a href={GITHUB} className="hover:text-fg">
              github.com/skymoore/theuth
            </a>{' '}
            ·{' '}
            <a href={APP} className="hover:text-fg">
              app.theuth.io
            </a>{' '}
            · relay.theuth.io
          </span>
          <span className="index">what the harness</span>
        </div>
      </footer>
    </>
  )
}
