import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type { Entry } from '@/store/sessions'
import { foldRuns, isDeliverable, summarize, ToolRunCard } from './ToolRunCard'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children }))

type Tool = Extract<Entry, { kind: 'tool' }>
const tool = (n: number, extra: Partial<Tool> = {}): Tool => ({
  id: `e${n}`,
  kind: 'tool',
  callId: `c${n}`,
  name: `t${n}`,
  args: { n },
  pending: false,
  child: false,
  result: { output: `out ${n}\nmore`, isError: false, files: [] },
  ...extra,
})
const text = (id: string): Entry => ({ id, kind: 'assistant', text: 'so', open: false })

test('foldRuns: assistant text splits runs, reasoning rides along, a lone call stays a plain entry', () => {
  const think: Entry = { id: 'r', kind: 'reasoning', text: 'hm', done: true }
  const [a, b, c, d, e, f] = [tool(1), tool(2), text('x'), tool(3), think, tool(4)]
  expect(foldRuns([a, b, c, d, e, f])).toEqual([[a, b], c, [d, e, f]])
  expect(foldRuns([c, a, c])).toEqual([c, a, c])
  // A trailing reasoning row is not part of the run.
  expect(foldRuns([a, b, e])).toEqual([[a, b], e])
})

test('183 §12.4: foldRuns splits a run at a call that carries a restart note, so its card stays in view', () => {
  const [a, b, c, d] = [tool(1), tool(2), tool(3), tool(4)]
  const restarted = tool(9, { restart: { clock: '09:00:00', after: 9, cause: 'restart', replayed: 1, interrupted: 0, text: '' } })
  expect(foldRuns([a, b, restarted, c, d])).toEqual([[a, b], restarted, [c, d]])
  expect(foldRuns([restarted, a, restarted])).toEqual([restarted, a, restarted])
})

test('foldRuns: a pinned thinking block splits a run like a deliverable; unpinned still rides along; a lone tool beside it stays plain', () => {
  const think: Entry = { id: 'r', kind: 'reasoning', text: 'hm', done: false }
  const other: Entry = { id: 'r2', kind: 'reasoning', text: 'ok', done: true }
  const [a, b, c, d] = [tool(1), tool(2), tool(3), tool(4)]
  const pinned = new Set(['r'])
  expect(foldRuns([a, b, think, c, d], pinned)).toEqual([[a, b], think, [c, d]])
  expect(foldRuns([a, b, other, c, d], pinned)).toEqual([[a, b, other, c, d]])
  // A lone tool on either side of the pinned block is a plain entry, not a one-tool run.
  expect(foldRuns([a, think, c, d], pinned)).toEqual([a, think, [c, d]])
  expect(foldRuns([a, b, think, c], pinned)).toEqual([[a, b], think, c])
  // The streaming tail: pinned or not, a trailing block is outside the run.
  expect(foldRuns([a, b, think], pinned)).toEqual([[a, b], think])
  // Deliverables still split as before alongside a pin; no pins, the old shape.
  const s = tool(9, { name: 'show' })
  expect(foldRuns([a, s, b, think, c, d], pinned)).toEqual([a, s, b, think, [c, d]])
  expect(foldRuns([a, b, think, c, d])).toEqual([[a, b, think, c, d]])
})

test('foldRuns: a deliverable never joins a run — it splits one; a board create still does; no deliverable, no change', () => {
  const [a, b, c, d] = [tool(1), tool(2), tool(3), tool(4)]
  for (const name of ['show', 'app', 'agent_draft', 'task_draft', 'routine_draft', 'send_message']) {
    const s = tool(9, { name })
    expect(isDeliverable(s)).toBe(true)
    expect(foldRuns([a, b, s, c, d])).toEqual([[a, b], s, [c, d]])
    expect(foldRuns([a, b, s, c])).toEqual([[a, b], s, c])
    expect(foldRuns([s, a, s])).toEqual([s, a, s])
  }
  const board = tool(8, { name: 'doc_create', args: { kind: 'board', title: 'T' } })
  expect(isDeliverable(board)).toBe(true)
  expect(foldRuns([a, board, b, c])).toEqual([a, board, [b, c]])
  expect(isDeliverable(tool(7, { name: 'doc_create', args: { kind: 'note' } }))).toBe(false)
  expect(foldRuns([a, b, c, d])).toEqual([[a, b, c, d]])
})

test('foldRuns: an answered ask_user is a deliverable — it splits a run and a lone one between text rows stays a plain entry', () => {
  const [a, b, c, d] = [tool(1), tool(2), tool(3), tool(4)]
  const ask = tool(9, { name: 'ask_user', args: { questions: [{ id: 'q', text: 'Which?' }] }, result: { output: 'answered', isError: false, files: [] } })
  expect(isDeliverable(ask)).toBe(true)
  expect(foldRuns([a, b, ask, c, d])).toEqual([[a, b], ask, [c, d]])
  expect(foldRuns([text('x'), ask, text('y')])).toEqual([text('x'), ask, text('y')])
  for (const name of ['secret_request', 'desktop_handoff']) expect(isDeliverable(tool(8, { name }))).toBe(false)
})

type Peer = Extract<Entry, { kind: 'peer' }>
const peer = (id: string, from: string, body = id, extra: Partial<Peer> = {}): Peer => ({ id, kind: 'peer', from, text: body, ...extra })

test('foldRuns: adjacent peer cards from one sender fold; another sender, any other row or a removed row ends the fold', () => {
  const [a1, a2, a3] = [peer('a1', 'session:a'), peer('a2', 'session:a'), peer('a3', 'session:a')]
  const b = peer('b', 'session:b')
  expect(foldRuns([a1, a2, a3, b])).toEqual([[a1, a2, a3], b])
  expect(foldRuns([a1, text('x'), a2])).toEqual([a1, text('x'), a2])
  expect(foldRuns([a1, b, a2])).toEqual([a1, b, a2])
  const removed = peer('r', 'session:a', 'r', { context: { target: { kind: 'message', seq: 3 }, active: false, removed: true, remove_with: [], attachments: [] } } as Partial<Peer>)
  expect(foldRuns([a1, removed, a2])).toEqual([a1, removed, a2])
  // The authenticated endpoint decides, not the label.
  const o1 = peer('o1', 'label', 'x', { origin: { id: 'm1', from: 'session:a', reply_to: null, scope: null, contact_mode: null } })
  const o2 = peer('o2', 'other label', 'y', { origin: { id: 'm2', from: 'session:a', reply_to: null, scope: null, contact_mode: null } })
  expect(foldRuns([o1, o2])).toEqual([[o1, o2]])
  // A send between two of its answers keeps them apart and stays its own card.
  const send = tool(5, { name: 'send_message', args: { to: 'session:a', message: 'hi' } })
  expect(foldRuns([tool(1), send, tool(2), a1, send, a2])).toEqual([tool(1), send, tool(2), a1, send, a2])
})

test('summarize: live status names the current call and its position; the ending summary counts, times and names failures', () => {
  const live = summarize([tool(1, { startedAt: 1000, finishedAt: 1400 }), tool(2, { result: undefined }), tool(3, { pending: true, result: undefined })])
  expect(live).toMatchObject({ text: 'running `t2` · 2 of 3', running: true, failed: false, hint: '✓ out 1  (+1 lines)' })
  const done = summarize([
    tool(1, { startedAt: 1000 }),
    tool(2, { result: { output: 'boom', isError: true, files: [] } }),
    tool(3, { finishedAt: 3100 }),
  ])
  expect(done).toMatchObject({ text: '3 tools · 2.1 s · 1 failed (t2)', running: false, failed: false, hint: '✓ out 3  (+1 lines)' })
  // History rows carry no times: no duration.
  expect(summarize([tool(1), tool(2)]).text).toBe('2 tools')
})

const fail = (n: number): Tool => tool(n, { result: { output: 'boom', isError: true, files: [] } })
/** `n` finished calls, the first `failed` of them failed; numbered from `from`. */
const finished = (n: number, failed: number, from = 1): Tool[] => Array.from({ length: n }, (_, i) => (i < failed ? fail(from + i) : tool(from + i)))

test('summarize: red only when more completed calls failed than succeeded; the header still names every failure', () => {
  expect(summarize(finished(34, 2))).toMatchObject({ text: '34 tools · 2 failed (t1, t2)', failed: false })
  expect(summarize(finished(1, 1)).failed).toBe(true)
  // A tie is not a majority.
  expect(summarize(finished(2, 1)).failed).toBe(false)
  expect(summarize(finished(3, 2)).failed).toBe(true)
  // Running: only completed calls count.
  const pending = tool(99, { result: undefined })
  expect(summarize([...finished(1, 1), pending])).toMatchObject({ running: true, failed: true })
  expect(summarize([...finished(3, 1), pending])).toMatchObject({ running: true, failed: false })
})

test('ToolRunCard: the red border follows the majority, not any failure', () => {
  const render = (run: Tool[]) => renderToStaticMarkup(createElement(ToolRunCard, { run, prompts: null, session: 's' }))
  const few = render(finished(34, 2, 101))
  expect(few).toContain('2 failed (t101, t102)')
  expect(few).not.toContain('border-danger-border')
  expect(render(finished(3, 2, 201))).toContain('border-danger-border')
})

test('ToolRunCard: collapsed shows one header and no cards; expanded shows all three', () => {
  const run = [tool(1), tool(2), tool(3)]
  const render = () => renderToStaticMarkup(createElement(ToolRunCard, { run, prompts: null, session: 's' }))
  const collapsed = render()
  expect(collapsed).toContain('3 tools')
  expect(collapsed).toContain('aria-expanded="false"')
  expect(collapsed).not.toContain('>t1<')
  useToolRuns.setState({ disclosed: { [disclosureKey('run', 'c1')]: true } })
  const expanded = render()
  expect(expanded).toContain('aria-expanded="true"')
  for (const name of ['t1', 't2', 't3']) expect(expanded).toContain(`>${name}<`)
})
