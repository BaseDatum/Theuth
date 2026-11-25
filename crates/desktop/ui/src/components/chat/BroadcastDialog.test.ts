import '../settings/test-dom'
import { receiveComponent } from '@/rpc/bindings/receive'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement as h, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { SidebarProvider } from '@/components/ui/sidebar'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import { EMPTY, useDrafts } from '@/lib/drafts'
import { useConnection } from '@/store/connection'
import type { BroadcastReceipt } from "@/rpc/bindings/method.agent.output"
import type { BroadcastRow } from "@/rpc/bindings/method.agent.output"
import type { SessionActivity } from "@/rpc/bindings/method.agent.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { AgentInfo } from '@/store/ui-events'
import { useSessions } from '@/store/sessions'
import type { Api } from '@/rpc/bindings/client'
import type { AgentSummary } from '@/rpc/bindings/method.roster.output'
import { deskList, deskSessions } from '@/lib/desks'
import { activeDescendants, allRoots, BROADCAST_DRAFT, deskRoots, openBroadcastDialog, outcomeLines, runningRoots, summarize, title } from './broadcast'
import { Form } from './BroadcastDialog'

const rpc = vi.hoisted(() => ({ api: {} as Record<string, unknown> }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => rpc.api }))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<object>()),
  useNavigate: () => vi.fn(),
  useRouterState: () => '/',
  Link: ({ to, params, children, ...rest }: { to: string; params: { session: string }; children: ReactNode }) =>
    h('a', { href: to.replace('$session', params.session), ...rest }, children),
}))
const toast = vi.hoisted(() => ({ success: vi.fn(), warning: vi.fn(), error: vi.fn(), message: vi.fn() }))
vi.mock('@/lib/notify', () => ({ notify: toast }))
// The gate itself has its own tests: here the engine offers the Desks group.
vi.mock('@/rpc/capabilities', async (original) => ({ ...(await original<object>()), Capability: ({ children }: { children: ReactNode }) => children }))

const activity = (p: Partial<SessionActivity> = {}): SessionActivity => (receiveComponent("method.agent.output.SessionActivity", {
  state: { state: 'idle' },
  last_active: null,
  turn_started_at: null,
  jobs: 0,
  ptys: 0,
  children: 0,
  asks: null,
  live: true,
  ...p,
}))

const row = (id: string, parent: string, depth: number, a: Partial<SessionActivity> = {}): AgentInfo => (receiveComponent("method.agent.output.AgentInfo", {
  id,
  name: id,
  parent,
  depth,
  agent: null,
  model: 'm',
  state: a.state ?? { state: 'idle' },
  started_at: null,
  finished_at: null,
  last: '',
  activity: activity(a),
}))

// Roots: A runs, B idle with a PTY, C idle and jobless, D awaiting. Children: a1 under A (running), a11 under a1, b1 under B.
const rows = [
  row('A', 'A', 0, { state: { state: 'running' } }),
  row('a1', 'A', 1, { state: { state: 'running' } }),
  row('a11', 'a1', 2),
  row('B', 'B', 0, { ptys: 1 }),
  row('b1', 'B', 1),
  row('C', 'C', 0),
  row('D', 'D', 0, { state: { state: 'awaiting', from: 'x' } }),
]

test('"Running roots" ticks busy roots and roots with a running PTY, never children or idle roots', () => {
  expect(runningRoots(rows)).toEqual(['A', 'B', 'D'])
})

test('"Running roots" ticks an idle root whose subagent works, at any depth; idle/finished subagents and non-roots never', () => {
  const tree = [
    row('R', 'R', 0, { state: { state: 'running' } }), // running itself
    row('P', 'P', 0), // idle, child running
    row('p1', 'P', 1, { state: { state: 'running' } }),
    row('p11', 'p1', 2, { state: { state: 'running' } }),
    row('G', 'G', 0), // idle, grandchild owns a PTY under an idle child
    row('g1', 'G', 1),
    row('g11', 'g1', 2, { ptys: 1 }),
    row('Q', 'Q', 0), // idle, children idle or finished
    row('q1', 'Q', 1),
    row('q2', 'Q', 1, { state: { state: 'cancelled' } }),
    row('S', 'S', 0), // idle, grandchild approval-blocked
    row('s1', 'S', 1, { state: { state: 'cancelled' } }),
    row('s11', 's1', 2, { state: { state: 'approval', tool: 't' } }),
    row('o1', 'gone', 1, { state: { state: 'running' } }), // orphan subagent: not a root, never ticked
  ]
  expect(runningRoots(tree)).toEqual(['R', 'P', 'G', 'S'])
  expect([...activeDescendants(tree)]).toEqual([
    ['P', 2],
    ['G', 1],
    ['S', 1],
  ])
})

test('"All roots" ticks every root, never children', () => {
  expect(allRoots(rows)).toEqual(['A', 'B', 'C', 'D'])
})

const receiptRow = (session: string, a: Partial<SessionActivity>, outcome: BroadcastRow['outcome']): BroadcastRow => ({
  session,
  title: session === 'nameless' ? '' : `T ${session}`,
  parent: null,
  before: activity(a),
  outcome,
})

const receipt = (rows: BroadcastRow[]): BroadcastReceipt => ({ id: '0', at: '2026-09-22T00:00:00Z', rows })

test('dry-run label counts busy by state only: a PTY-owning idle root starts now', () => {
  const r = receipt([
    receiptRow('A', { state: { state: 'running' } }, { outcome: 'skipped', reason: 'dry run' }),
    receiptRow('B', { ptys: 1 }, { outcome: 'skipped', reason: 'dry run' }),
    receiptRow('D', { state: { state: 'approval', tool: 't' } }, { outcome: 'skipped', reason: 'dry run' }),
  ])
  expect(summarize(r)).toBe('3 sessions · 2 busy → queued, 1 idle → start now')
  expect(summarize(receipt([r.rows[0]!]))).toBe('1 session · 1 busy → queued, 0 idle → start now')
})

const summary = (id: string, over: Partial<SessionSummary> = {}): SessionSummary => (receiveComponent("method.agent.output.SessionSummary", {
  id,
  created_at: new Date(2026, 8, 18, 14, 5).toISOString(),
  updated_at: new Date(2026, 8, 19).toISOString(),
  title: '',
  tags: [],
  model: 'm',
  parent: null,
  name: null,
  effort: null,
  agent: null,
  cwd: null,
  pinned: false,
  archived: false,
  project: null,
  notebook: null,
  retention: 'durable',
  running: false,
  owner: '',
  epoch: 1,
  memory: 'epochs',
  here: true,
  ...over,
}))

const ID = '0192a4c1-7e3b-7c0e-9c1f-2b6d1e8f4a10'

test('a recipient is named by the session list: title, else the engine-seeded `Untitled chat · date`; a subagent by its peer name; never the id', () => {
  expect(title(ID, [summary(ID, { title: 'Fix relay reconnect loop' })])).toBe('Fix relay reconnect loop')
  expect(title(ID, [summary(ID)])).toBe('Untitled chat · Sep 18')
  expect(title(ID, [summary(ID, { title: 'Ignored', name: 'Scout', parent: 'p' })])).toBe('Scout')
  // The list does not know it yet: the caller's own label, else the generic name.
  expect(title(ID, undefined, 'Scout')).toBe('Scout')
  expect(title(ID, [])).toBe('Untitled chat')
  for (const sessions of [[summary(ID)], undefined]) expect(title(ID, sessions)).not.toContain(ID.slice(0, 8))
})

test('outcome lines put failures first and name rows like the picker: the list, else the receipt title, else `Untitled chat`', () => {
  const lines = outcomeLines(
    receipt([
      receiptRow('A', {}, { outcome: 'started' }),
      receiptRow('nameless', {}, { outcome: 'failed', error: 'unknown session' }),
      receiptRow('B', {}, { outcome: 'queued', input_id: 'i1', delivery: 'next_turn' }),
      receiptRow('C', {}, { outcome: 'skipped', reason: 'remote node' }),
    ]),
    [summary('A', { title: 'Listed A' })],
  )
  expect(lines).toEqual([
    'Untitled chat — failed: unknown session',
    'Listed A — started',
    'T B — queued (next turn)',
    'T C — skipped: remote node',
  ])
})

// The roster: scout's current desk is B, theuth's is D; maya has no desk yet; ghost is only proposed. Every session
// that ever was a desk keeps `SessionSummary.desk`: C is theuth's earlier desk and E (a root added below) is the desk
// of seed-1, an agent gone from the roster. Only the roster's current desks count.
const agent = (name: string, desk: string | null, status = 'idle') => ({ desk, status, file: { name } }) as unknown as AgentSummary
const ROSTER = [agent('scout', 'B'), agent('theuth', 'D', 'working'), agent('maya', null), agent('ghost', 'G', 'proposed')]
const DESKS = deskSessions(deskList(ROSTER))
const withStale = [...rows, row('E', 'E', 0), row('G', 'G', 0)]

test('the desks are the sidebar\'s: one per active roster agent, at its current desk session, theuth first', () => {
  expect(deskList(ROSTER).map((d) => [d.name, d.id])).toEqual([['theuth', 'D'], ['scout', 'B'], ['maya', null]])
  expect([...DESKS]).toEqual([['D', 'theuth'], ['B', 'scout']])
})

test('"All desks" picks only current desks: an agent\'s older desk, a removed agent\'s desk and a proposed one are not', () => {
  expect(deskRoots(withStale, DESKS)).toEqual(['D', 'B'])
  expect(deskRoots(withStale, new Map())).toEqual([])
})

// ------------------------------------------------------------------------------------------------- the mounted view
// What the sidebar would list: desks B (scout) and D (theuth) as tiles, chats A and C (pinned) with A's subagent a1.
const LIST = [
  summary('A', { title: 'Alpha' }),
  summary('a1', { parent: 'A', name: 'worker' }),
  summary('B', { desk: 'scout' }),
  summary('C', { title: 'Gamma', pinned: true }),
  summary('D', { desk: 'theuth' }),
]
type BroadcastArgs = Parameters<Api['broadcast']>[0]
const broadcasts: BroadcastArgs[] = []
const sends = () => broadcasts.filter((b) => !b.dry_run)

beforeEach(() => {
  broadcasts.length = 0
  vi.clearAllMocks()
  useConnection.setState({ discovery: { ...useConnection.getState().discovery, phase: 'legacy' } })
  useDrafts.getState().set(BROADCAST_DRAFT, EMPTY)
  const answered: Record<string, (...a: never[]) => unknown> = {
    list_activity: async () => rows,
    list_sessions: async () => LIST,
    agents_list: async () => ROSTER,
    broadcast: async (req: BroadcastArgs) => {
      broadcasts.push(req)
      const ids = req.targets.kind === 'sessions' ? req.targets.ids : []
      return receipt(ids.map((id) => receiptRow(id, {}, req.dry_run ? { outcome: 'skipped', reason: 'dry run' } : { outcome: 'started' })))
    },
  }
  // Everything else the composer and the list ask for stays pending: this view's behaviour does not depend on it.
  rpc.api = new Proxy(answered, { get: (t, k: string) => t[k] ?? (() => new Promise(() => {})) })
})
afterEach(() => {
  document.body.innerHTML = ''
})
// jsdom lays nothing out: give every box a height, so the chat list's virtualizer has a window to fill.
Object.defineProperty(window.HTMLElement.prototype, 'offsetHeight', { configurable: true, get: () => 800 })

async function mount(req: Parameters<typeof Form>[0]['req'] = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(['activity'], rows)
  qc.setQueryData(queryKeys.sessions, LIST)
  qc.setQueryData(queryKeys.roster, ROSTER)
  const onDone = vi.fn()
  const host = document.body.appendChild(document.createElement('div'))
  await act(async () =>
    createRoot(host).render(
      h(QueryClientProvider, { client: qc }, h(SidebarProvider, null, h(TooltipProvider, null, h(Form, { req, active: 'A', onDone })))),
    ),
  )
  const box = (label: string) => host.querySelector<HTMLInputElement>(`label[title="${label}"] input[type="checkbox"], input[aria-label^="${label} desk"]`)
  const ticked = () =>
    [...host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]:checked')].map((i) => i.closest('label')!.getAttribute('title') ?? i.getAttribute('aria-label')!.split(' ')[0])
  const click = (el: Element) => act(async () => void (el as HTMLElement).click())
  const button = (text: string) => [...host.querySelectorAll('button')].find((b) => b.textContent === text || b.getAttribute('aria-label') === text)
  return { host, onDone, box, ticked, click, button }
}

test('the view is the new chat\'s composer, then delivery, preview and the presets — no Send button of its own — then the list', async () => {
  const v = await mount()
  const text = v.host.textContent!
  const composer = v.host.querySelector('[data-composer]')!
  expect(composer.querySelector('textarea')).not.toBeNull()
  expect(composer.querySelector('[aria-label="Add"]')).not.toBeNull()
  // The only Send is the composer's.
  expect([...v.host.querySelectorAll('button')].filter((b) => /send/i.test(`${b.textContent} ${b.getAttribute('aria-label')}`)).map((b) => b.closest('[data-composer]') !== null)).toEqual([true])
  const at = (s: string) => text.indexOf(s)
  for (const s of ['Steer (inside the running turn)', 'Running roots', 'All roots', 'None', 'All desks']) expect(at(s), s).toBeGreaterThan(-1)
  const firstBox = v.host.querySelector('input[type="checkbox"]')!
  expect(composer.compareDocumentPosition(firstBox) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  expect(at('Running roots')).toBeLessThan(at('Desks'))
})

test('the list is the sidebar\'s, a checkbox on every row: desk tiles (one without a session cannot be ticked), Pinned, the open chat\'s tree', async () => {
  const v = await mount()
  // Desk tiles in roster order, then the chat list's sections; desk sessions are tiles only.
  expect([...v.host.querySelectorAll('input[aria-label$="idle"], input[aria-label*="desk,"]')].map((i) => i.getAttribute('aria-label')!.split(' ')[0])).toEqual(['theuth', 'scout', 'maya'])
  expect(v.box('maya')!.disabled).toBe(true)
  const text = v.host.textContent!
  expect(text.indexOf('Pinned')).toBeLessThan(text.indexOf('Gamma'))
  expect(text.indexOf('Gamma')).toBeLessThan(text.indexOf('Alpha'))
  expect(v.host.querySelector('label[title="Alpha"] input[type="checkbox"]')).not.toBeNull()
  // The open chat (A) carries its agent tree as in the sidebar; its rows tick too and never link.
  await act(async () => {
    useSessions.getState().setChildren('A', [row('a1', 'A', 1, { state: { state: 'running' } })])
  })
  const sub = [...v.host.querySelectorAll('label')].find((l) => l.textContent?.includes('a1'))!
  expect(sub.querySelector('input[type="checkbox"]')).not.toBeNull()
  expect(v.host.querySelector('a[href]')).toBeNull()
})

test('the preset ticks its boxes; a chip replaces the ticks, a click on a row toggles it', async () => {
  const v = await mount()
  // Untouched "running": A, B and D (busy, PTY, awaiting).
  expect(v.ticked().sort()).toEqual(['Alpha', 'scout', 'theuth'])
  await v.click(v.button('None')!)
  expect(v.ticked()).toEqual([])
  await v.click(v.button('All desks')!)
  expect(v.ticked().sort()).toEqual(['scout', 'theuth'])
  await v.click(v.box('Gamma')!)
  expect(v.ticked().sort()).toEqual(['Gamma', 'scout', 'theuth'])
  await v.click(v.box('scout')!)
  expect(v.ticked().sort()).toEqual(['Gamma', 'theuth'])
  await v.click(v.button('All roots')!)
  expect(v.ticked().sort()).toEqual(['Alpha', 'Gamma', 'scout', 'theuth'])
})

test('the composer\'s send broadcasts the message to the ticked sessions with the chosen delivery, then closes', async () => {
  openBroadcastDialog({ text: 'wrap up' })
  expect(useDrafts.getState().by[BROADCAST_DRAFT]?.text).toBe('wrap up')
  const v = await mount({ preset: 'desks' })
  await v.click(v.button('Next turn')!)
  await v.click(v.box('Gamma')!)
  await v.click(v.button('Send')!)
  await vi.waitFor(() => expect(v.onDone).toHaveBeenCalled())
  const sent = sends()
  expect(sent).toEqual([{ targets: { kind: 'sessions', ids: ['D', 'B', 'C'] }, text: 'wrap up', files: [], delivery: 'next_turn', dry_run: false }])
  expect(toast.success).toHaveBeenCalledWith('Sent to 3 sessions', expect.anything())
})

test('with nothing ticked the send goes nowhere and says why; the draft stays', async () => {
  openBroadcastDialog({ text: 'hello' })
  const v = await mount()
  await v.click(v.button('None')!)
  await v.click(v.button('Send')!)
  await vi.waitFor(() => expect(toast.error).toHaveBeenCalledWith('Pick at least one session'))
  expect(sends()).toEqual([])
  expect(v.onDone).not.toHaveBeenCalled()
  expect(useDrafts.getState().by[BROADCAST_DRAFT]?.text).toBe('hello')
})
