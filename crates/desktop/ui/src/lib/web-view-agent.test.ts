import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type { WebViewCallAnswer, WebViewCallRequested, WebViewCallResolved } from '@/lib/web-view-call-wire'
import { useWebViews } from '@/lib/web-views'
import { IDLE, LINGER_MS, REFUSE_MS, STOPPED, acting, ended, startWebViewAgent, started, stopAgent, stopped, useTileAgent } from './web-view-agent'

const shell = vi.hoisted(() => ({
  held: new Set<string>(),
  native: true,
  /** Each `web_view_call`, with its settle handles. */
  calls: [] as { id: string; op: string; args: unknown; resolve: (v: unknown) => void; reject: (e: unknown) => void }[],
  closed: null as null | ((s: { label: string }) => void),
}))
vi.mock('@/lib/web-view-native', () => ({
  nativeWebViews: () => shell.native,
  webLabel: (id: string) => `web-${id}`,
  holds: (id: string) => shell.native && shell.held.has(`web-${id}`),
  release: (label: string) => void shell.held.delete(label),
  viewCall: (id: string, op: string, args: unknown) => {
    const { promise, resolve, reject } = Promise.withResolvers<unknown>()
    shell.calls.push({ id, op, args, resolve, reject })
    return promise
  },
  onClosed: async (cb: NonNullable<typeof shell.closed>) => {
    shell.closed = cb
    return () => (shell.closed = null)
  },
  close: async () => {},
}))
const engine = vi.hoisted(() => ({
  answers: [] as [string, WebViewCallAnswer][],
  supports: true,
  /** What the engine answers a claim: `false` when another client claimed first. */
  wins: true,
  api: null as null | object,
}))
engine.api = {
  answer_web_view_call: vi.fn(async (id: string, a: WebViewCallAnswer) => {
    engine.answers.push([id, a])
    return a.kind === 'claimed' ? engine.wins : true
  }),
}
vi.mock('@/store/connection', () => ({ useConnection: { getState: () => ({ api: engine.api, state: 'open', discovery: {} }) } }))
vi.mock('@/rpc/discovery', () => ({
  supports: (_d: unknown, r: { methods?: string[]; topics?: string[] }) =>
    engine.supports && r.methods?.[0] === 'answer_web_view_call' && r.topics?.[0] === 'artifact.web-view-call-requested@1',
}))
vi.mock('@/lib/web-profile', () => ({ profileDevice: () => 'iPad-abc123' }))
const events = vi.hoisted(() => ({
  requested: null as null | ((e: WebViewCallRequested) => void),
  resolved: null as null | ((e: WebViewCallResolved) => void),
}))
vi.mock('@/store/sessions', () => ({
  on: (topic: string, type: string, cb: never) => {
    if (topic === 'artifact.web-view-call-requested@1' && type === 'WebViewCallRequested') events.requested = cb
    else if (topic === 'artifact.web-view-call-resolved@1' && type === 'WebViewCallResolved') events.resolved = cb
    else throw new Error(`unexpected ${topic} ${type}`)
    return () => {
      if (topic.endsWith('requested@1')) events.requested = null
      else events.resolved = null
    }
  },
}))

const TILE = '0123456789abcdef'
let seq = 0
const request = (over: Partial<WebViewCallRequested> = {}): WebViewCallRequested => ({
  type: 'WebViewCallRequested',
  id: `call-${++seq}`,
  session: 's1',
  view: TILE,
  op: 'script',
  args: { world: 'isolated', source: 'return cmd', arguments: { cmd: 'read', params: {} }, timeout_ms: 1000 },
  ...over,
})
/** Let the agent's promise chains (the shell call, then the answer) run. */
const flush = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve()
}
let stop: () => void = () => {}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] })
  vi.setSystemTime(1_000_000)
  shell.held = new Set([`web-${TILE}`])
  shell.native = true
  shell.calls = []
  engine.answers = []
  engine.supports = true
  engine.wins = true
  useTileAgent.setState({ tiles: {} })
  useWebViews.setState({ views: {} })
  stop = startWebViewAgent()
})
afterEach(async () => {
  // Settle every call a test left in flight, so none carries into the next one.
  for (const c of shell.calls) c.resolve(null)
  await flush()
  stop()
  vi.useRealTimers()
})

test('a call for a tile this window holds is claimed at once, run in the shell once the claim wins, then answered Done', async () => {
  const e = request()
  events.requested!(e)
  // Claimed before anything runs.
  expect(engine.answers).toEqual([[e.id, { kind: 'claimed', client: 'iPad-abc123' }]])
  expect(shell.calls).toEqual([])
  await flush()
  expect(shell.calls.map((c) => [c.id, c.op, c.args])).toEqual([[TILE, 'script', e.args]])
  expect(acting(useTileAgent.getState().tiles[TILE], Date.now())).toBe(true)
  shell.calls[0]!.resolve({ value: 1 })
  await flush()
  expect(engine.answers).toEqual([
    [e.id, { kind: 'claimed', client: 'iPad-abc123' }],
    [e.id, { kind: 'done', result: { value: 1 } }],
  ])
  expect(useTileAgent.getState().tiles[TILE]).toMatchObject({ calls: [], endedAt: Date.now() })
})

test('a lost claim (another device showing the same tile claimed first) runs nothing and shows no bar', async () => {
  engine.wins = false
  events.requested!(request())
  await flush()
  expect(engine.answers.map(([, a]) => a.kind)).toEqual(['claimed'])
  expect(shell.calls).toEqual([])
  expect(acting(useTileAgent.getState().tiles[TILE], Date.now())).toBe(false)
})

test('a snapshot answers its PNG beside the result', async () => {
  const e = request({ op: 'snapshot', args: {} })
  events.requested!(e)
  await flush()
  shell.calls[0]!.resolve({ png: 'iVBORw0KGgo=', width: 390, height: 600 })
  await flush()
  expect(engine.answers.at(-1)).toEqual([e.id, { kind: 'done', result: { width: 390, height: 600 }, png: 'iVBORw0KGgo=' }])
})

test('a shell failure answers Failed with its reason', async () => {
  const e = request()
  events.requested!(e)
  await flush()
  shell.calls[0]!.reject('script timed out after 1000 ms')
  await flush()
  expect(engine.answers.at(-1)).toEqual([e.id, { kind: 'failed', reason: 'script timed out after 1000 ms' }])
  expect(useTileAgent.getState().tiles[TILE]?.calls).toEqual([])
})

test('a window claims only tiles it holds: not another tile, not one popped out of it, not one closed since', async () => {
  events.requested!(request({ view: 'fedcba9876543210' }))
  // The main window still knows the view, but a pop-out window holds it now.
  useWebViews.getState().open({ id: TILE, session: 's1', url: 'https://example.com/', title: 'Example' })
  useWebViews.getState().patch(TILE, { popped: true })
  events.requested!(request())
  useWebViews.getState().patch(TILE, { popped: false })
  shell.closed!({ label: `web-${TILE}` })
  events.requested!(request())
  await flush()
  expect(engine.answers).toEqual([])
  expect(shell.calls).toEqual([])
})

test('a window without native web views, or on an engine without the events, never answers', async () => {
  engine.supports = false
  events.requested!(request())
  stop()
  shell.native = false
  engine.supports = true
  stop = startWebViewAgent()
  expect(events.requested).toBeNull()
  await flush()
  expect(engine.answers).toEqual([])
  expect(shell.calls).toEqual([])
})

test('Stop fails the call in flight, drops its late result, and refuses the tile for 60 s without running anything', async () => {
  const e = request()
  events.requested!(e)
  await flush()
  stopAgent(TILE)
  expect(engine.answers.at(-1)).toEqual([e.id, { kind: 'failed', reason: STOPPED }])
  expect(acting(useTileAgent.getState().tiles[TILE], Date.now())).toBe(false)
  shell.calls[0]!.resolve({ value: 'late' })
  await flush()
  expect(engine.answers.filter(([id]) => id === e.id).map(([, a]) => a.kind)).toEqual(['claimed', 'failed'])

  vi.setSystemTime(1_000_000 + REFUSE_MS - 1)
  const refused = request()
  events.requested!(refused)
  await flush()
  expect(engine.answers.filter(([id]) => id === refused.id)).toEqual([[refused.id, { kind: 'failed', reason: STOPPED }]])
  expect(shell.calls).toHaveLength(1)

  vi.setSystemTime(1_000_000 + REFUSE_MS)
  const again = request()
  events.requested!(again)
  await flush()
  expect(engine.answers.at(-1)).toEqual([again.id, { kind: 'claimed', client: 'iPad-abc123' }])
  expect(shell.calls).toHaveLength(2)
})

test('Stop while the claim is out: the call never runs', async () => {
  const e = request()
  events.requested!(e)
  stopAgent(TILE)
  await flush()
  expect(engine.answers.map(([, a]) => a.kind)).toEqual(['claimed', 'failed'])
  expect(shell.calls).toEqual([])
})

test('the engine resolving a call ends it here; its late result is not answered', async () => {
  const e = request()
  events.requested!(e)
  await flush()
  events.resolved!({ type: 'WebViewCallResolved', id: e.id })
  expect(useTileAgent.getState().tiles[TILE]?.calls).toEqual([])
  shell.calls[0]!.resolve({ value: 1 })
  await flush()
  expect(engine.answers.map(([, a]) => a.kind)).toEqual(['claimed'])
})

test('the bar shows while a call is in flight and 3 s after the last, and goes at once on Stop', () => {
  let a = started(started(IDLE, 'c1'), 'c2')
  expect(acting(undefined, 0)).toBe(false)
  expect(acting(IDLE, 0)).toBe(false)
  expect(acting(a, 0)).toBe(true)
  a = ended(a, 'c1', 100)
  expect(acting(a, 100 + LINGER_MS * 10)).toBe(true)
  a = ended(a, 'c2', 200)
  expect(acting(a, 200 + LINGER_MS - 1)).toBe(true)
  expect(acting(a, 200 + LINGER_MS)).toBe(false)
  expect(ended(a, 'c2', 900)).toBe(a)
  expect(acting(stopped(300), 300)).toBe(false)
})

test('arguments, results and failures never reach the console', async () => {
  const SECRET = 'hunter2-form-value'
  const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((m) => vi.spyOn(console, m).mockImplementation(() => {}))
  events.requested!(request({ args: { world: 'isolated', source: 'return cmd', arguments: { cmd: 'fill', params: { value: SECRET } }, timeout_ms: 1000 } }))
  await flush()
  shell.calls[0]!.resolve({ value: SECRET })
  await flush()
  events.requested!(request({ args: { world: 'page', source: SECRET, arguments: {}, timeout_ms: 1000 } }))
  await flush()
  shell.calls[1]!.reject(`script failed near ${SECRET}`)
  await flush()
  expect(engine.answers.map(([, a]) => a.kind)).toEqual(['claimed', 'done', 'claimed', 'failed'])
  const logged = spies.flatMap((s) => s.mock.calls).map((c) => JSON.stringify(c))
  expect(logged.filter((l) => l.includes(SECRET))).toEqual([])
  for (const s of spies) s.mockRestore()
})
