// Startup-steps proof, dev only: `bunx vite` then open `/src/dev/launch-fixture.html?scene=<scene>` (`&theme=dark`,
// `&still` pauses every animation for pixel-stable screenshots). The real `StartupScreen` and `QuitOverlay` against
// stubbed connection/engine stores, with `mockIPC` answering `connection` / `launch_log` / `startup_status` the way the
// shell does. Scenes: starting · reused · connecting · loading · reconnecting · failed · stuck (wait 8 s) · link ·
// link-failure · quit; 222 (`startup.json`): migrating (3/78, live: a step a second) · stalled · engine-failed · interrupt.
import '@/styles.css'
import { emit } from '@tauri-apps/api/event'
import { mockIPC } from '@tauri-apps/api/mocks'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QuitOverlay, useQuit } from '@/components/launch/QuitOverlay'
import { StartupScreen } from '@/components/launch/StartupScreen'
import { INITIAL, type LinkState } from '@/lib/link-status'
import { type Engine, useEngines } from '@/link/engines'
import type { Info } from '@/rpc/bindings/method.rpc_2dcompose.output'
import type { StartupProgress, StartupStatus } from '@/lib/tauri'
import type { RpcClient } from '@/rpc/client'
import type { ConnState } from '@/rpc/transport'
import { scope } from '@/store/engine-scope'

const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')
if (query.has('still')) {
  const style = document.createElement('style')
  style.textContent = '*, *::before, *::after { animation-play-state: paused !important; transition: none !important; }'
  document.head.append(style)
}
const scene = query.get('scene') ?? 'starting'

// What a login shell prints before `exec t --headless` takes over the pty (colours and a title sequence included).
const BOOT_LOG =
  'Last login: Tue Nov 17 11:02:41 on ttys004\r\n' +
  '\u001b]0;theuth\u0007\u001b[1;32mtheuth\u001b[0m 2.31.0 (headless)\r\n' +
  'data dir /Users/sky/Library/Application Support/theuth\r\n' +
  '\u001b[2mloading 14 skills, 6 MCP servers\u001b[0m\r\n' +
  'opening store\u001b[K\r\n'
const RC_LOG =
  'Last login: Tue Nov 17 11:02:41 on ttys004\r\n' +
  '\u001b[1;34m[oh-my-zsh]\u001b[0m Would you like to update? [Y/n] '
const LOG: Record<string, string> = { starting: BOOT_LOG, connecting: BOOT_LOG, loading: BOOT_LOG, reconnecting: BOOT_LOG, failed: RC_LOG, stuck: RC_LOG }

// 222: what the engine writes to `startup.json` 41 s into a boot after an upgrade, migrating agent.db.
const BOOT_MS = Date.now() - 41_000
const MIGRATING: StartupStatus = {
  v: 1,
  pid: 52811,
  started_ms: BOOT_MS,
  seq: 24,
  hb_ms: Date.now(),
  state: 'running',
  phase: 'migrate:agent',
  label: 'Migrating agent.db',
  step: { n: 3, of: 78 },
  done: [
    { phase: 'lock', label: 'Taking the data-dir lock', ms: 4 },
    { phase: 'config', label: 'Reading config', ms: 18 },
    { phase: 'runtime-init', label: 'Starting the runtime', ms: 1_240 },
    { phase: 'plugin:secrets', label: 'Opening secrets', ms: 310 },
    { phase: 'migrate:secrets', label: 'Migrating secrets.db', ms: 2_870 },
    { phase: 'plugin:tasks', label: 'Opening tasks', ms: 96 },
    { phase: 'migrate:tasks', label: 'Migrating tasks.db', ms: 11_400 },
    { phase: 'plugin:roster', label: 'Opening roster', ms: 41 },
    { phase: 'migrate:roster', label: 'Migrating roster.db', ms: 6_020 },
  ],
  error: null,
}
const PROGRESS: Record<string, StartupProgress> = {
  migrating: { status: MIGRATING, alive: true, stalled: false, waited_ms: 41_000 },
  interrupt: { status: MIGRATING, alive: true, stalled: false, waited_ms: 41_000 },
  stalled: { status: { ...MIGRATING, started_ms: Date.now() - 160_000 }, alive: true, stalled: true, waited_ms: 160_000 },
  'engine-failed': {
    status: { ...MIGRATING, state: 'failed', step: { n: 12, of: 78 }, error: 'migration 0012_threads_fts of agent.db failed: database or disk is full' },
    alive: false,
    stalled: false,
    waited_ms: 41_000,
  },
}
const NO_PROGRESS: StartupProgress = { status: null, alive: true, stalled: false, waited_ms: 0 }

mockIPC(
  (cmd) => {
    if (cmd === 'launch_log') return LOG[scene] ?? ''
    if (cmd === 'startup_status') return PROGRESS[scene] ?? NO_PROGRESS
    if (cmd === 'connection' || cmd === 'relaunch') return { url: 'ws://127.0.0.1:7777', token: 'fixture', data_dir: '', owned: scene !== 'reused' }
    return null
  },
  { shouldMockEvents: true },
)

// The shell's tailer: one migration a second, the way `startup-progress` arrives.
if (scene === 'migrating' && !query.has('still')) {
  let n = MIGRATING.step!.n
  setInterval(() => {
    n = Math.min(n + 1, MIGRATING.step!.of)
    const status = { ...MIGRATING, seq: MIGRATING.seq + n, hb_ms: Date.now(), step: { n, of: MIGRATING.step!.of } }
    void emit('startup-progress', { ...PROGRESS.migrating!, status })
  }, 1_000)
}

const client = {} as RpcClient
const info = {} as Info
type Conn = { client: RpcClient | null; state: ConnState; info: Info | null; error?: string }
const CONN: Record<string, Conn> = {
  starting: { client: null, state: 'connecting', info: null },
  stuck: { client: null, state: 'connecting', info: null },
  reused: { client, state: 'connecting', info: null },
  connecting: { client, state: 'connecting', info: null },
  loading: { client, state: 'open', info: null },
  reconnecting: { client, state: 'reconnecting', info },
  failed: {
    client: null,
    state: 'connecting',
    info: null,
    error: 'The engine did not open 127.0.0.1:52811 within 60 s. A login shell may be waiting for input: see the launch log.',
  },
  migrating: { client: null, state: 'connecting', info: null },
  stalled: { client: null, state: 'connecting', info: null },
  'engine-failed': { client: null, state: 'connecting', info: null, error: 'The engine stopped during startup (exit code 1).' },
  // A plain Relaunch the shell refused (222): the confirm.
  interrupt: { client: null, state: 'connecting', info: null, error: 'starting: Migrating agent.db (3/78)' },
}

const studio: Engine = { id: 'node-studio', name: 'studio', relayHost: 'relay.theuth.dev', secure: true, pairedAt: 1, lastSeen: 0, order: 0, kind: 'engine' }
const LINK: Record<string, LinkState> = {
  link: { ...INITIAL, phase: 'connecting', attempts: 1 },
  'link-failure': { ...INITIAL, phase: 'connecting', attempts: 3, sticky: 'stale-pairing' },
}

if (scene in LINK) useEngines.setState({ engines: [studio], active: studio.id, link: { [studio.id]: LINK[scene]! }, hydrated: true })
else scope('local').connection.setState(CONN[scene] ?? CONN.starting!)

if (scene === 'quit') {
  // Two chats draining (one already saved), the companion closing; `at` far enough back that the drain bar is full.
  useQuit.setState({
    state: {
      restart: false,
      owned: true,
      at: Date.now() - 60_000,
      turns: [
        { id: 'a', title: 'Refactor the parser', done: true },
        { id: 'b', title: 'Weekly report', done: false },
      ],
      companion: true,
      step: 'companion',
      forced: false,
    },
  })
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {scene === 'quit' ? (
      <QuitOverlay />
    ) : (
      <StartupScreen onRelaunch={() => console.log('relaunch')} onQuit={() => console.log('quit')} />
    )}
  </StrictMode>,
)
