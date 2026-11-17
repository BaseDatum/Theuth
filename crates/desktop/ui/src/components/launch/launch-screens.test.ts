import { createElement as h } from 'react'
import type * as ReactDom from 'react-dom'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import { INITIAL, type LinkState } from '@/lib/link-status'
import type { QuitState } from '@/lib/quit-progress'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import { type Engine, useEngines } from '@/link/engines'
import { QuitOverlay, useQuit } from './QuitOverlay'
import { LocalStartupView, StartupScreen } from './StartupScreen'
import { STUCK_MS, type StartupInput, startupView } from './startup-progress'

// The overlay portals into `<body>`; render it in place.
vi.mock('react-dom', async (original) => ({ ...(await original<typeof ReactDom>()), createPortal: (node: unknown) => node }))

beforeEach(() => useToolRuns.setState({ disclosed: {} }))

const QUIT: QuitState = {
  restart: false,
  owned: true,
  at: 1_000,
  turns: [
    { id: 'a', title: 'Refactor the parser', done: true },
    { id: 'b', title: 'Weekly report', done: false },
  ],
  companion: true,
  step: 'companion',
  forced: true,
}

test('the quit overlay markup is unchanged by the shared launch card (snapshot taken on 887e5f5f3)', () => {
  vi.spyOn(Date, 'now').mockReturnValue(4_000)
  vi.stubGlobal('document', { body: {} })
  // `renderToStaticMarkup` reads a `create` store's initial state (one object, returned by reference): preset that.
  const initial = useQuit.getInitialState()
  initial.state = QUIT
  expect(renderToStaticMarkup(h(QuitOverlay))).toMatchSnapshot()
  initial.state = { ...QUIT, step: 'closing' }
  expect(renderToStaticMarkup(h(QuitOverlay))).toMatchSnapshot()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

// ------------------------------------------------------------------------------------------------- the local startup

const LOG = 'Last login: Tue Nov 17 on ttys004\r\n\u001b[1;32mtheuth\u001b[0m 2.31.0\r\nopening store\r\n'
const BASE: StartupInput = { started: false, owned: null, state: 'connecting', info: false, log: LOG, waited: 0 }
const local = (over: Partial<StartupInput> = {}) => {
  const input = { ...BASE, ...over }
  return renderToStaticMarkup(h(LocalStartupView, { view: startupView(input), log: input.log, onQuit() {}, onRelaunch() {} }))
}
const SPINNER = 'lucide-loader-circle'
const DANGER = 'lucide-circle-x'
const count = (html: string, s: string) => html.split(s).length - 1

test('starting: spinner title, the engine step active with the last log line, the log collapsed, Quit only', () => {
  const html = local()
  expect(html).toContain('>Starting theuth</h2>')
  expect(count(html, SPINNER)).toBe(2)
  expect(html).toContain('aria-live="polite"')
  expect(html).toContain('<p class="truncate text-xs text-fg-secondary">opening store</p>')
  expect(html).toContain('Show launch log')
  expect(html).toContain('aria-expanded="false"')
  expect(html).not.toContain('<pre')
  expect(html).toContain('>Quit</button>')
  expect(html).not.toContain('Relaunch')
})

test('reused engine: the engine step done with its note; no log, no disclosure', () => {
  const html = local({ started: true, owned: false, log: '' })
  expect(html).toContain('lucide-circle-check')
  expect(html).toContain('Using the engine that is already running')
  expect(html).toContain('Connecting<span class="sr-only">, in progress</span>')
  expect(html).toContain('Loading<span class="sr-only">, waiting</span>')
  expect(html).not.toContain('launch log')
})

test('reconnecting: its title, Relaunch offered, the log still collapsed', () => {
  const html = local({ started: true, owned: true, state: 'reconnecting', info: true })
  expect(html).toContain('>Reconnecting to theuth</h2>')
  expect(html).toContain('Relaunch')
  expect(html).not.toContain('<pre')
})

test('failed: danger header and step, the error as its detail, the log open, Relaunch', () => {
  const html = local({ error: 'The engine did not open its port within 15 s' })
  expect(html).toContain('>theuth could not start</h2>')
  expect(html).not.toContain(SPINNER)
  expect(count(html, DANGER)).toBe(2)
  expect(html).toContain('<p class="text-sm text-danger">Starting the engine<span class="sr-only">, failed</span></p>')
  expect(html).toContain('<p class="text-xs text-fg-secondary">The engine did not open its port within 15 s</p>')
  expect(html).toContain('Hide launch log')
  expect(html).toMatch(/<pre[^>]*>Last login/)
  expect(html).toContain('Relaunch')
})

test('stuck past 8 s: the log opens itself; a reader who closed it keeps it closed', () => {
  expect(local({ waited: STUCK_MS })).toContain('<pre')
  useToolRuns.getState().setDisclosed(disclosureKey('startup', 'launch-log'), false)
  expect(local({ waited: STUCK_MS })).not.toContain('<pre')
  expect(local({ error: 'boom' })).not.toContain('<pre')
})

// -------------------------------------------------------------------------------------------------- the link screen

const studio: Engine = { id: 'n1', name: 'studio', relayHost: 'relay.example', secure: true, pairedAt: 1, lastSeen: 0, order: 0, kind: 'engine' }
const link = (state: LinkState) => {
  // `renderToStaticMarkup` reads a `create` store's initial state (one object, returned by reference): preset that.
  Object.assign(useEngines.getInitialState(), { engines: [studio], active: studio.id, link: { [studio.id]: state } })
  return renderToStaticMarkup(h(StartupScreen, { onQuit() {}, onRelaunch() {} }))
}
const BUTTONS = ['Copy diagnostics', 'Forget this node', 'Pair with a mesh…']

test('link connecting: one active step reaching the node, with the explanation; the buttons unchanged', () => {
  const html = link({ ...INITIAL, phase: 'connecting', attempts: 1 })
  expect(html).toContain('>Connecting to studio</h2>')
  expect(html).toContain('Reaching studio<span class="sr-only">, in progress</span>')
  expect(html).toContain('The node is offline or the relay is unreachable. This screen closes when it is back.')
  for (const b of BUTTONS) expect(html).toContain(`>${b}</button>`)
})

test('link failure: the step fails and the named failure renders under it', () => {
  const html = link({ ...INITIAL, phase: 'connecting', attempts: 3, sticky: 'stale-pairing' })
  expect(html).toContain('<p class="text-sm text-danger">Reaching studio<span class="sr-only">, failed</span></p>')
  expect(html).toContain('This pairing no longer matches an engine on the relay')
  expect(html).toContain('Re-pair')
  expect(html).not.toContain('This screen closes when it is back')
  for (const b of BUTTONS) expect(html).toContain(`>${b}</button>`)
})
