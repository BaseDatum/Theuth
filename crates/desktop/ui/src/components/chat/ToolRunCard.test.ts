import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { Entry } from '@/store/sessions'
import { foldRuns, summarize, ToolRunCard } from './ToolRunCard'

// `renderToStaticMarkup` reads zustand's server snapshot (the initial state), so the open map is a plain object here.
const store = vi.hoisted(() => ({ open: {} as Record<string, boolean>, toggle: () => {} }))
vi.mock('@/lib/tool-runs', () => ({ useToolRuns: (pick: (s: typeof store) => unknown) => pick(store) }))
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
  store.open.c1 = true
  const expanded = render()
  expect(expanded).toContain('aria-expanded="true"')
  for (const name of ['t1', 't2', 't3']) expect(expanded).toContain(`>${name}<`)
})
