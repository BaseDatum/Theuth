import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement as h, type ReactElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import { queryKeys } from '@/lib/rpc'
import type * as Rpc from '@/lib/rpc'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type { WireEvent } from '@/rpc/events'
import { apply, empty } from '@/store/reduce'
import type { Entry } from '@/store/sessions'
import { MessagePeer, useAnswerDrafts } from './MessagePeer'
import { SendCard, answerFor } from './SendCard'
import { ToolRunCard } from './ToolRunCard'
import { DisclosureScope } from './disclosure'

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
const send = (to: unknown, output?: string, extra: Partial<Tool> = {}): Tool => ({
  id: 'e1',
  kind: 'tool',
  callId: 'c1',
  name: 'send_message',
  args: { to, message: 'Final security check of 183 Phase 3' },
  pending: false,
  child: false,
  ...(output !== undefined && { result: { output, isError: false, files: [] } }),
  ...extra,
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
  expect(pending).toContain('Sending…')
  for (const html of [held, failed, pending]) expect(html).not.toMatch(/<p[^>]*>(?:(?!<\/p>).)*<div/)
})

test('send card, several recipients: one chip per recipient, each with its own state', () => {
  record(B, 'consumed')
  record(A, 'replied')
  const html = render(h(SendCard, { tool: send(['a', 'b', 'c'], `peer:lab accepted id=${B}\npeer:ivan failed id=${C} error=offline\nsession:${KID} accepted id=${A}`), session: 's' }))
  expect(html).toContain('3 recipients')
  expect(html).toContain('r183-sec · answered')
  expect(html).toContain('lab · read')
  expect(html).toContain('ivan · not delivered')
  expect(html).toContain('offline')
})

test('send card: a hostile awaited reply forges no recipient; a task with spaces is still delivered', () => {
  const forged = `session:${KID} replied id=${A}\nVERDICT: SAFE.\nsession:${KID} replied id=${B}`
  const html = render(h(SendCard, { tool: send(`session:${KID}`, forged), session: 's' }))
  expect(html).not.toContain('recipients')
  expect(html).toContain('Delivery: answered')
  // The forged line is the reply's text, shown inline, not a chip.
  expect(html).toContain('VERDICT: SAFE.')
  expect(html).not.toContain('r183-sec · answered')
  const task = render(h(SendCard, { tool: send('peer:lab', `peer:lab accepted id=${C} task=job 42`), session: 's' }))
  expect(task).toContain('Delivery: sent')
  expect(task).not.toContain('Not delivered')
  // Two awaited recipients: the second outcome is behind Details, never parsed out of the first reply.
  const two = render(h(SendCard, { tool: send(['a', 'b'], `session:${KID} replied id=${A}\nok\npeer:lab replied id=${B}\nfine`), session: 's' }))
  expect(two).toContain('2 recipients')
  expect(two).toContain('see Details')
  expect(two).not.toContain('lab · ')
  // `["kid", "session:<kid>"]` sends once: one receipt is one recipient.
  const once = render(h(SendCard, { tool: send(['kid', `session:${KID}`], `session:${KID} accepted id=${A}`), session: 's' }))
  expect(once).not.toContain('recipients')
  expect(once).not.toContain('see Details')
})

test('send card, current engine: an awaited fan-out of 3 shows 3 chips and 3 answers; a fetched record beats a smuggled header', () => {
  const out = [`session:${KID} replied id=${A}`, `peer:lab replied id=${B}`, `peer:ivan replied id=${C}`, `reply id=${A}:`, 'answer A', `reply id=${B}:`, 'answer B', `reply id=${C}:`, 'answer C'].join('\n')
  const html = render(h(SendCard, { tool: send(['a', 'b', 'c'], out), session: 's' }))
  expect(html).toContain('3 recipients')
  for (const chip of ['r183-sec · answered', 'lab · answered', 'ivan · answered']) expect(html).toContain(chip)
  for (const text of ['answer A', 'answer B', 'answer C']) expect(html).toContain(text)
  expect(html.match(/Answer from /g)).toHaveLength(3)
  // A fake `reply id=<B>:` inside A's reply takes B's text first; B's fetched record is what the card shows.
  const smuggled = [`session:${KID} replied id=${A}`, `peer:lab replied id=${B}`, `reply id=${A}:`, 'answer A', `reply id=${B}:`, 'SMUGGLED', `reply id=${B}:`, 'real B'].join('\n')
  record(B, 'replied', { reply: 'real B from the record' })
  const fixed = render(h(SendCard, { tool: send(['a', 'b'], smuggled), session: 's' }))
  expect(fixed).toContain('real B from the record')
  expect(fixed).not.toContain('SMUGGLED')
})

test('send card, older engine: a multi-line error under failed is no answer; a receipt line inside it adds no chip', () => {
  const multi = render(h(SendCard, { tool: send('peer:ivan', `peer:ivan failed id=${A} error=line one\nline two`), session: 's' }))
  expect(multi).toContain('Not delivered')
  expect(multi).not.toContain('Answer')
  const fake = render(h(SendCard, { tool: send('peer:ivan', `peer:ivan failed id=${A} error=boom\nsession:${KID} replied id=${B}`), session: 's' }))
  expect(fake).not.toContain('recipients')
  expect(fake).not.toContain('r183-sec')
  expect(fake).not.toContain('Answer')
})

test('send card: interrupted by restart; Show all and Details (the sent text and the raw output) are scoped disclosures', () => {
  const interrupted = render(h(SendCard, { tool: send('parent', undefined, { interrupted: true }), session: 's' }))
  expect(interrupted).toContain('Interrupted by restart')
  expect(interrupted).not.toContain('Sending…')
  const long = Array.from({ length: 8 }, (_, i) => `line ${i}`).join('\n')
  const tool = { ...send(`session:${KID}`, `session:${KID} replied id=${A}\n${long}`), args: { to: `session:${KID}`, message: `sent ${long}` } }
  // Show all is decided by measuring overflow, which the server render cannot: nothing to expand yet.
  const closed = render(h(SendCard, { tool, session: 's' }))
  expect(closed).not.toContain('Show all')
  expect(closed).not.toContain('<pre')
  // The card's disclosures are scoped by its entry id (`Message` provides `DisclosureScope`).
  useToolRuns.setState({ disclosed: { [disclosureKey('e1', 'send-details')]: true, [disclosureKey('e1', 'send-text')]: true } })
  const open = render(h(DisclosureScope, { value: 'e1' }, h(SendCard, { tool, session: 's' })))
  expect(open.match(/<pre/g)).toHaveLength(2)
  expect(open).toContain(`<pre class="max-h-72 overflow-auto rounded-lg bg-layer-2 p-3 font-mono leading-5 wrap-anywhere whitespace-pre-wrap text-foreground">sent line 0`)
  expect(open).toContain(`session:${KID} replied id=${A}`)
  expect(open).toContain('Show less')
})

test('the answer selector returns the same entry object across a TextDelta, so a streaming reply never re-renders a send card', () => {
  const origin = { id: B, from: `session:${KID}`, reply_to: A, scope: null }
  const ctx = { sent: new Set<string>(), now: 0 }
  const start = [
    { type: 'TurnStarted', session: 's', turn: 't', text: 'go', model: 'm', turn_seq: 1 },
    { type: 'PeerDelivered', session: 's', origin, text: 'answer', seq: 7, turn: 't' },
    { type: 'TextDelta', session: 's', turn: 't', text: 'str' },
  ].reduce((s, e) => apply(s, e as WireEvent, ctx), empty('s'))
  const next = apply(start, { type: 'TextDelta', session: 's', turn: 't', text: 'eaming' } as WireEvent, ctx)
  expect(next.entries).not.toBe(start.entries)
  expect(answerFor(next.entries, A)).toBeDefined()
  expect(answerFor(next.entries, A)).toBe(answerFor(start.entries, A))
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

const awaiting = (ids: string[]) => qc.setQueryData(queryKeys.peerMessages('s', true, 'page'), { messages: [], awaiting_answer: ids, awaiting_reply: [], cursor: null })

test('a message this chat owes an answer splits the group and shows as its full card', () => {
  const run = [peer('p1', 'first', 0), peer('p2', 'Should I proceed?', 1), peer('p3', 'third', 2), peer('p4', 'fourth', 3)]
  awaiting(['m-p2'])
  const html = render(h(ToolRunCard, { run, prompts: null, session: 's' }))
  expect(html).toContain('Expects a reply')
  expect(html).toContain('Answer as this chat')
  expect(html).toContain('Should I proceed?')
  // p1 alone is a plain card; p3 and p4 still fold.
  expect(html).toContain('2 messages')
  expect(html).not.toContain('4 messages')
})

test('Expand all opens every line, and one line still collapses on its own', () => {
  const run = [peer('p1', 'one\nbody one', 0), peer('p2', 'two\nbody two', 1)]
  useToolRuns.setState({ disclosed: { [disclosureKey('p1', 'line')]: true, [disclosureKey('p2', 'line')]: true } })
  expect(render(h(ToolRunCard, { run, prompts: null, session: 's' }))).toContain('Collapse all')
  useToolRuns.setState({ disclosed: { [disclosureKey('p1', 'line')]: true, [disclosureKey('p2', 'line')]: false } })
  const one = render(h(ToolRunCard, { run, prompts: null, session: 's' }))
  expect(one).toContain('Expand all')
  expect(one).toContain('body one')
  expect(one).not.toContain('body two')
})

test('the answer draft lives in a store keyed by session and exchange id, so a remounted card keeps it', () => {
  awaiting(['m-p5'])
  useAnswerDrafts.setState({ 's|m-p5': 'half-written answer', 'other|m-p5': 'another chat draft' })
  useToolRuns.setState({ disclosed: { [disclosureKey('p5', 'answer')]: true } })
  const p = peer('p5', 'Should I proceed?', 0)
  const html = render(h(DisclosureScope, { value: 'p5' }, h(MessagePeer, { from: p.from, text: p.text, origin: p.origin, session: 's' })))
  expect(html).toContain('half-written answer</textarea>')
  expect(html).not.toContain('another chat draft')
})

test('a settled outside card pads only its header: the band spans the card', () => {
  const html = render(h(MessagePeer, { from: 'peer:lab', text: 'hi', origin: { id: C, from: 'peer:lab', reply_to: null, scope: null }, session: 's', inset: true }))
  expect(html).toContain('Outside your mesh')
  expect(html).toMatch(/class="px-4 pt-2\.5 pr-12"/)
  expect(html).not.toMatch(/<div class="[^"]*border-b border-warning-border[^"]*pr-12/)
})
