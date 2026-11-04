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
  for (const name of ['show', 'app', 'agent_draft', 'task_draft', 'routine_draft']) {
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

test('summarize: live status names the current call and its position; the ending summary counts, times and names failures', () => {
  const live = summarize([tool(1, { startedAt: 1000, finishedAt: 1400 }), tool(2, { result: undefined }), tool(3, { pending: true, result: undefined })])
  expect(live).toMatchObject({ text: 'running `t2` · 2 of 3', running: true, failed: false, hint: '✓ out 1  (+1 lines)' })
  const done = summarize([
    tool(1, { startedAt: 1000 }),
    tool(2, { result: { output: 'boom', isError: true, files: [] } }),
    tool(3, { finishedAt: 3100 }),
  ])
  expect(done).toMatchObject({ text: '3 tools · 2.1 s · 1 failed (t2)', running: false, failed: true, hint: '✓ out 3  (+1 lines)' })
  // History rows carry no times: no duration.
  expect(summarize([tool(1), tool(2)]).text).toBe('2 tools')
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
