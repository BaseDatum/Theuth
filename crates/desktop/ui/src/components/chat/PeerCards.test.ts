import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h, type ReactElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type { Entry } from '@/store/sessions'
import { MessagePeer } from './MessagePeer'
import { SendCard } from './SendCard'
import { ToolRunCard } from './ToolRunCard'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children, useNavigate: () => () => {} }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))

type Tool = Extract<Entry, { kind: 'tool' }>
const A = '01a1c2c6-8f43-74dc-bc91-3bfa9d876af5'
const B = '01a1c2c6-8f43-74dc-bc91-3bfa9d876af6'
const C = '01a1c2c6-8f43-74dc-bc91-3bfa9d876af7'
const KID = '01a1c28b-8e10-7757-afd9-db253e4f5e73'

let qc: QueryClient
beforeEach(() => {
  qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  qc.setQueryData(queryKeys.sessions, [{ id: KID, title: 'r183-sec', name: null, parent: null, owner: '', here: true }])
  useToolRuns.setState({ disclosed: {} })
})
const render = (el: ReactElement) => renderToStaticMarkup(h(QueryClientProvider, { client: qc }, el))
/** The live record `peer_message` answers, as the PeerMessageChanged refetch would leave it in the cache. */
const record = (id: string, state: string, extra: Record<string, unknown> = {}) =>
  qc.setQueryData(queryKeys.peerMessage('s', id), { receipt: { id, to: 'x', state, reply_to: null, reply: null, task_id: null, error: null, answered_by: null, ...extra }, from: 'session:s', session: 's', text: '', created_at: '', request_scope: null })
const send = (to: unknown, output?: string): Tool => ({
  id: 'e1',
  kind: 'tool',
  callId: 'c1',
  name: 'send_message',
  args: { to, message: 'Final security check of 183 Phase 3' },
  pending: false,
  child: false,
  ...(output !== undefined && { result: { output, isError: false, files: [] } }),
})

test('send card, one recipient: You → name, the preview, the live track, the answer inline', () => {
  const tool = send(`session:${KID}`, `session:${KID} accepted id=${A}`)
  const before = render(h(SendCard, { tool, session: 's' }))
  expect(before).toContain('You →')
  // The name in words; the raw address only in the hover title.
  expect(before).toContain('>r183-sec</span>')
  expect(before.replace(/title="[^"]*"/g, '')).not.toContain(KID)
  expect(before).toContain('Final security check of 183 Phase 3')
  expect(before).toContain('Delivery: sent')
  // The cached record moved on: the card follows it, not the tool's first receipt.
  record(A, 'replied', { reply: 'VERDICT: CHANGES.' })
  const after = render(h(SendCard, { tool, session: 's' }))
  expect(after).toContain('Delivery: answered')
  expect(after).toContain('VERDICT: CHANGES.')
})

test('send card: held warns with the receipt error; failed is danger with the error text and no retry', () => {
  record(A, 'held', { error: "Held on 333: sky hasn't allowed messages yet." })
  const held = render(h(SendCard, { tool: send('peer:sky/hi-there', `peer:sky/hi-there accepted id=${A}`), session: 's' }))
  expect(held).toContain("Held on 333: sky hasn&#x27;t allowed messages yet.")
  expect(held).toContain('border-warning-border')
  const failed = render(h(SendCard, { tool: send('peer:ivan', `peer:ivan failed id=${B} error=ivan's machine is offline`), session: 's' }))
  expect(failed).toContain('Not delivered')
  expect(failed).toContain('ivan&#x27;s machine is offline')
  expect(failed).not.toContain('Retry')
  // Still running (an awaited send): no receipt yet. The spinner is a <div>, so its line must not be a <p> (React warns).
  const pending = render(h(SendCard, { tool: { ...send('parent'), args: { to: 'parent', message: 'q', await_reply: true } }, session: 's' }))
  expect(pending).toContain('waiting for the answer')
  for (const html of [held, failed, pending]) expect(html).not.toMatch(/<p[^>]*>(?:(?!<\/p>).)*<div/)
})

test('send card, several recipients: one chip per recipient, each with its own state', () => {
  record(B, 'consumed')
  const html = render(h(SendCard, { tool: send(['a', 'b', 'c'], `session:${KID} replied id=${A}\nok\npeer:lab accepted id=${B}\npeer:ivan failed id=${C} error=offline`), session: 's' }))
  expect(html).toContain('3 recipients')
  expect(html).toContain('r183-sec · answered')
  expect(html).toContain('lab · read')
  expect(html).toContain('ivan · not delivered')
  expect(html).toContain('offline')
})

type Peer = Extract<Entry, { kind: 'peer' }>
const peer = (id: string, text: string, at: number): Peer => ({ id, kind: 'peer', from: `session:${KID}`, text, at, origin: { id: `m-${id}`, from: `session:${KID}`, reply_to: null, scope: null } })

test('a folded group: one header, one line per message, a repeat marked, a clicked line opens its card', () => {
  const run = [peer('p1', 'DONE Z3-R4 e815e520\nmore', 0), peer('p2', 'HANDOFF WRITTEN', 60_000), peer('p3', ' HANDOFF   WRITTEN ', 60_000)]
  const closed = render(h(ToolRunCard, { run, prompts: null, session: 's' }))
  expect(closed).toContain('3 messages')
  expect(closed).toContain('DONE Z3-R4 e815e520')
  expect(closed).not.toContain('more')
  expect(closed).toContain('Same as the message above')
  expect(closed.match(/Same as the message above/g)).toHaveLength(1)
  useToolRuns.setState({ disclosed: { [disclosureKey('p1', 'line')]: true } })
  expect(render(h(ToolRunCard, { run, prompts: null, session: 's' }))).toContain('more')
})

test('an incoming reply quotes the first line of what this chat sent, Markdown marks removed', () => {
  qc.setQueryData(queryKeys.peerMessage('s', A), { receipt: { id: A, to: `session:${KID}`, state: 'replied', reply_to: null, reply: null, task_id: null, error: null, answered_by: null }, from: 'session:s', session: 's', text: '\n**Z4-R1**, the last round.\nmore', created_at: '', request_scope: null })
  const origin = { id: B, from: `session:${KID}`, reply_to: A, scope: null }
  const html = render(h(MessagePeer, { from: origin.from, text: 'DONE', origin, session: 's' }))
  expect(html).toContain('Replying to <b class="font-semibold text-foreground">you</b>: Z4-R1, the last round.')
})
