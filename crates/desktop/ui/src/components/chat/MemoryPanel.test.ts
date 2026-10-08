import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SessionMemory, ViewLine } from '@/rpc/endless-stubs'
import { INITIAL, MemoryBadge, badgeTip, flatten, highlight, reduce } from './MemoryPanel'

const line = (id: number, n: number, text: string, built = true): ViewLine => ({ id, n, text, built, level: Math.log2(n) })

const view = [line(0, 64, 'the beginning'), line(64, 2, 'two recent'), line(66, 1, 'the last one', false)]

const memory: SessionMemory = {
  mode: 'endless',
  view,
  settling: false,
  unbuilt: 0,
  units: 67,
  stats: { calls: 12, guard_trips: { copy: 1, grounding: 0, status: 0, format: 2, empty: 0, size: 0 }, input_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, output_tokens: 0, usd_estimate: 0.12 },
  model: 'anthropic/claude-haiku-4-5',
  view_bytes: 128_000,
  backfill: null,
}

const summary = (memory: 'endless' | 'epochs'): SessionSummary =>
  ({ id: 's1', title: 'x', memory }) as unknown as SessionSummary

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ session_memory: async () => memory }),
}))

test('expanding a line puts its two children right under it, indented; collapsing hides them but keeps them cached', () => {
  const kids = [line(0, 32, 'first half'), line(32, 32, 'second half')]
  let t = reduce(INITIAL, { type: 'toggle', key: '0+64' })
  expect(flatten(view, t.kids, t.open).map((r) => r.key)).toEqual(['0+64', '64+2', '66+1']) // loading: nothing below yet
  t = reduce(t, { type: 'loaded', key: '0+64', lines: kids })
  expect(flatten(view, t.kids, t.open).map((r) => [r.key, r.depth, r.parent])).toEqual([
    ['0+64', 0, null],
    ['0+32', 1, '0+64'],
    ['32+32', 1, '0+64'],
    ['64+2', 0, null],
    ['66+1', 0, null],
  ])
  // Grandchildren follow their parent, recursively.
  t = reduce(reduce(t, { type: 'toggle', key: '32+32' }), { type: 'loaded', key: '32+32', lines: [line(32, 16, 'a'), line(48, 16, 'b')] })
  expect(flatten(view, t.kids, t.open).map((r) => r.key)).toEqual(['0+64', '0+32', '32+32', '32+16', '48+16', '64+2', '66+1'])
  t = reduce(t, { type: 'toggle', key: '0+64' })
  expect(flatten(view, t.kids, t.open).map((r) => r.key)).toEqual(['0+64', '64+2', '66+1'])
  expect(t.kids['0+64']).toBe(kids)
  expect(t.open.has('32+32')).toBe(true)
  // A failed zoom closes the line again.
  t = reduce(reduce(t, { type: 'toggle', key: '64+2' }), { type: 'failed', key: '64+2' })
  expect(t.open.has('64+2')).toBe(false)
})

test('recall hits lead the keyboard order under their own prefix, so a hit on a line also in the view does not collide', () => {
  const t = reduce(INITIAL, { type: 'hits', hits: { query: 'last', lines: [line(66, 1, 'the last one')] } })
  const rows = [...flatten(t.hits!.lines, t.kids, t.open, 'hit:'), ...flatten(view, t.kids, t.open)]
  expect(rows.map((r) => r.key)).toEqual(['hit:66+1', '0+64', '64+2', '66+1'])
  expect(reduce(t, { type: 'hits', hits: null }).hits).toBeNull()
})

test('highlight marks every query word, case-insensitively, with regex characters taken literally', () => {
  const html = renderToStaticMarkup(createElement('p', null, highlight('Error in src/lib.rs: Index out of range', 'error INDEX lib.rs')))
  expect(html).toBe('<p><mark class="rounded-sm bg-info-bg px-0.5 text-foreground">Error</mark> in src/<mark class="rounded-sm bg-info-bg px-0.5 text-foreground">lib.rs</mark>: <mark class="rounded-sm bg-info-bg px-0.5 text-foreground">Index</mark> out of range</p>')
  expect(highlight('plain', 'zzz')).toBe('plain')
  expect(highlight('plain', 'a')).toBe('plain') // one-letter words do not highlight
})

test('the badge tooltip names size, line count and settle state', () => {
  expect(badgeTip(memory)).toMatch(/^Endless memory · view \d+ B · 3 lines · up to date$/)
  expect(badgeTip({ ...memory, settling: true, unbuilt: 3 })).toContain('· settling 3 lines')
  expect(badgeTip({ ...memory, settling: true, unbuilt: 1 })).toContain('· settling 1 line')
  expect(badgeTip({ ...memory, backfill: { done: 12, total: 80, eta_secs: 300, usd_estimate: 0.4 } })).toContain('· backfill 12/80')
})

const render = (summaryRow: SessionSummary, data: SessionMemory) => {
  const qc = new QueryClient()
  qc.setQueryData(queryKeys.sessionMemory('s1'), data)
  return renderToStaticMarkup(
    createElement(QueryClientProvider, {
      client: qc,
      children: createElement(TooltipProvider, null, createElement(MemoryBadge, { session: 's1', summary: summaryRow, open: false, onToggle() {} }) as ReactNode),
    }),
  )
}

test('the ∞ badge shows for an endless session only, and pulses (motion-safe) while settling', () => {
  expect(render(summary('epochs'), memory)).toBe('')
  const quiet = render(summary('endless'), memory)
  expect(quiet).toMatch(/<button[^>]*aria-label="Endless memory"[^>]*aria-expanded="false"[^>]*>∞<\/button>/)
  expect(quiet).not.toContain('animate-pulse')
  expect(render(summary('endless'), { ...memory, settling: true, unbuilt: 2 })).toContain('motion-safe:animate-pulse')
})
