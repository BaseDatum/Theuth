import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type { ApprovalRequested } from "@/rpc/bindings/event.rpc_2estream.output"
import type { AskRequested } from "@/rpc/bindings/method.roster.output"
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { active } from '@/store/engine-scope'
import { dispatch } from '@/store/sessions'
import { ApprovalCard, offersAlwaysAsk, relayedLine } from './ApprovalCard'

const CLOUD = 'c10d'.repeat(16)
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<object>()), useApi: () => ({ list_sessions: async () => [] }) }))
vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: { self_id: 'me', peers: [{ id: CLOUD, name: 'cloud-1', reachable: true }] } }) }))
// A static render reads the stores' initial state, so the engine's methods are stubbed here: only `set_always_ask` is offered.
const can = vi.hoisted(() => ({ mark: false }))
vi.mock('@/rpc/capabilities', async (original) => ({ ...(await original<object>()), useQuerySupport: (methods: string[]) => can.mark && methods.includes('set_always_ask') }))

const approval = (session: string): ApprovalRequested => ({ type: 'ApprovalRequested', id: `ap-${session}`, session, tool: 'shell', args: { command: 'cargo test -p replica' }, risk: 'mutating', human_only: false })
const render = (session: string, rows: Pick<SessionSummary, 'id' | 'owner' | 'here'>[]) => {
  const client = new QueryClient()
  client.setQueryData(queryKeys.sessions, rows)
  return renderToStaticMarkup(createElement(QueryClientProvider, { client, children: createElement(ApprovalCard, { approval: approval(session) }) as ReactNode }))
}

test('183 §12.3 / mock panel 3: a card whose session is owned elsewhere is the owner asking, relayed, with its deadline', () => {
  const t0 = Date.now()
  dispatch([{key: 'tools.approval-requested@1', event: approval('remote')}]) // arrived live
  const html = render('remote', [{ id: 'remote', owner: CLOUD, here: false }])
  expect(html).toContain('>cloud-1 asks: shell</div>')
  expect(html).toMatch(/>mutating · relayed from cloud-1 · denies at \d{1,2}:\d{2}[^<]*<\/div>/)
  expect(html).toContain('>Allow</button>')
  expect(html).toContain('>Deny</button>')
  // A session owned here keeps today's card, no relayed line.
  const local = render('local', [{ id: 'local', owner: 'me', here: true }])
  expect(local).toContain('>approval: shell (mutating)</div>')
  expect(local).not.toContain('relayed from')
  // W-6: the owner's 300 s, read as its local clock time.
  const at = new Date(t0 + 300_000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  expect(relayedLine('destructive', 'xeus', t0 + 300_000)).toBe(`destructive · relayed from xeus · denies at ${at}`)
})

test('183 R1: a card that came from a snapshot has no deadline; resolving or withdrawing prunes the arrival', () => {
  const rows = [{ id: 'snap', owner: CLOUD, here: false }]
  dispatch([{key: 'rpc.stream@1', event: { type: 'PendingPrompts', approvals: [approval('snap')], asks: [], secrets: [] }}])
  const html = render('snap', rows)
  expect(html).toContain('>mutating · relayed from cloud-1</div>')
  expect(html).not.toContain('denies at')
  const st = active().approvals
  dispatch([{key: 'tools.approval-requested@1', event: approval('live')}])
  expect(st.getState().arrived.has('ap-live')).toBe(true)
  dispatch([{key: 'tools.approval-resolved@1', event: { type: 'ApprovalResolved', id: 'ap-live', decision: 'allow', timed_out: false, dismissed: false }}])
  expect(st.getState().arrived.has('ap-live')).toBe(false)
  dispatch([{key: 'tools.approval-requested@1', event: approval('gone')}])
  st.getState().withdraw(new Set(['gone']))
  expect(st.getState().arrived.has('ap-gone')).toBe(false)
})

test('183 R2: a prompt the engine flags `replay: true` (a refocus replay) has no arrival, so no deadline; a live one keeps it', () => {
  const st = active().approvals
  const asks = active().asks
  dispatch([{key: 'tools.approval-requested@1', event: { ...approval('again'), replay: true } as ApprovalRequested}])
  expect(st.getState().pending.map((p) => p.id)).toContain('ap-again')
  expect(st.getState().arrived.has('ap-again')).toBe(false)
  expect(render('again', [{ id: 'again', owner: CLOUD, here: false }])).not.toContain('denies at')
  dispatch([{key: 'tools.approval-requested@1', event: approval('fresh')}])
  expect(st.getState().arrived.has('ap-fresh')).toBe(true)
  // Asks share the store shape: a replayed one is pending without an arrival too.
  dispatch([{key: 'tools.ask-requested@1', event: { type: 'AskRequested', id: 'k1', session: 'again', questions: [], replay: true } as AskRequested}])
  expect(asks.getState().pending.map((p) => p.id)).toContain('k1')
  expect(asks.getState().arrived.has('k1')).toBe(false)
})

test('219 §6.1: an always-ask card carries the badge and no offer; any other card offers "Always ask for <tool>"', () => {
  can.mark = true
  try {
    const card = (always_ask: boolean) => {
      const client = new QueryClient()
      client.setQueryData(queryKeys.sessions, [{ id: 'here', owner: 'me', here: true }])
      return renderToStaticMarkup(createElement(QueryClientProvider, { client, children: createElement(ApprovalCard, { approval: { ...approval('here'), always_ask } }) as ReactNode }))
    }
    const always = card(true)
    expect(always).toContain('>approval: shell (mutating)</div>')
    expect(always).toMatch(/<span[^>]*data-slot="badge"[^>]*>always ask<\/span>/)
    expect(always).toContain('>Allow</button>')
    expect(always).toContain('>Deny</button>')
    expect(always).not.toContain('Always ask for')
    const plain = card(false)
    expect(plain).not.toContain('>always ask</span>')
    expect(plain).toContain('>Always ask for shell</button>')
  } finally {
    can.mark = false
  }
})

test('219 §6.1: no "Always ask for" offer on a human-only card, the nil session, the removal card or an ACP title', () => {
  const base = approval('0190a5b2-0000-7000-8000-000000000001')
  expect(offersAlwaysAsk(base)).toBe(true)
  expect(offersAlwaysAsk({ ...base, always_ask: true })).toBe(false)
  expect(offersAlwaysAsk({ ...base, human_only: true })).toBe(false)
  expect(offersAlwaysAsk({ ...base, session: '00000000-0000-0000-0000-000000000000' })).toBe(false)
  expect(offersAlwaysAsk({ ...base, tool: 'always_ask.remove' })).toBe(false)
  expect(offersAlwaysAsk({ ...base, tool: 'rm -rf build' })).toBe(false)
  can.mark = true
  try {
    const client = new QueryClient()
    client.setQueryData(queryKeys.sessions, [{ id: base.session, owner: 'me', here: true }])
    const html = renderToStaticMarkup(createElement(QueryClientProvider, { client, children: createElement(ApprovalCard, { approval: { ...base, human_only: true } }) as ReactNode }))
    expect(html).toContain('>Allow</button>')
    expect(html).not.toContain('Always ask for')
  } finally {
    can.mark = false
  }
})
