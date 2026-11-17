// Startup-steps proof, dev only: `bunx vite` then open `/src/dev/launch-fixture.html?scene=<scene>` (`&theme=dark`,
// `&still` pauses every animation for pixel-stable screenshots). The real `StartupScreen` and `QuitOverlay` against
// stubbed connection/engine stores, with `mockIPC` answering `connection` / `launch_log` the way the shell does.
// Scenes: starting · reused · connecting · loading · reconnecting · failed · stuck (wait 8 s) · link · link-failure · quit.
import '@/styles.css'
import { mockIPC } from '@tauri-apps/api/mocks'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QuitOverlay, useQuit } from '@/components/launch/QuitOverlay'
import { StartupScreen } from '@/components/launch/StartupScreen'
import { INITIAL, type LinkState } from '@/lib/link-status'
import { type Engine, useEngines } from '@/link/engines'
import type { Info } from '@/rpc/bindings/method.rpc_2dcompose.output'
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

mockIPC(
  (cmd) => {
    if (cmd === 'launch_log') return LOG[scene] ?? ''
    if (cmd === 'connection' || cmd === 'relaunch') return { url: 'ws://127.0.0.1:7777', token: 'fixture', data_dir: '', owned: scene !== 'reused' }
    return null
  },
  { shouldMockEvents: true },
)

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
    error: 'The engine did not open 127.0.0.1:52811 within 15 s. A login shell may be waiting for input: see the launch log.',
  },
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
