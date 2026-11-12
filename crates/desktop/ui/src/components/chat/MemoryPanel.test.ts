import { MutationObserver, QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { toast } from 'sonner'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { Api } from '@/rpc/api'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { SessionMemory } from '@/rpc/bindings/SessionMemory'
import type { Stalled } from '@/rpc/bindings/Stalled'
import type { ViewLine } from '@/rpc/bindings/ViewLine'
import { INITIAL, LineNode, MemoryBadge, MemoryFooter, SummaryModel, ViewNote, badgeTip, effortMutation, flatten, foldMutation, highlight, modelMutation, nextTry, pauseMutation, reduce, retryMutation } from './MemoryPanel'
import { isFoldedNote } from '@/lib/endless'
import type { EndlessBacklog } from '@/rpc/bindings/EndlessBacklog'
import type * as Combobox from './ModelCombobox'
import type { Effort } from './ModelPicker'

// 177 R1-10: what the summary picker was given (its closed popover renders no list to read the order from).
const pickers = vi.hoisted(() => [] as { 'aria-label'?: string; effort?: Effort }[])
vi.mock('./ModelCombobox', async (original) => {
  const m = await original<typeof Combobox>()
  return { ...m, ModelCombobox: (p: Parameters<typeof m.ModelCombobox>[0]) => (pickers.push(p), createElement(m.ModelCombobox, p)) }
})

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
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => () => {} }))
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }) }))

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

const NOW = Date.parse('2026-11-04T22:00:00Z')
const failing: Stalled = { failures: 4, error: 'ollama/qwen3:8b: request timed out after 120 s', next_retry_at: '2026-11-04T22:00:40Z', model: 'ollama/qwen3:8b', blocking: true }

const footer = (data: SessionMemory) => {
  const qc = new QueryClient()
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(TooltipProvider, null, createElement(MemoryFooter, { session: 's1', m: data })) as ReactNode }))
}

test('a failing settle: warning badge without the pulse, "summaries failing" in tip and menu row; cleared, the pulse is back', () => {
  const stalled = { ...memory, settling: true, unbuilt: 3, stalled: failing }
  const html = render(summary('endless'), stalled)
  expect(html).toContain('text-warning')
  expect(html).not.toContain('animate-pulse')
  expect(badgeTip(stalled)).toMatch(/· summaries failing$/)
  const qc = new QueryClient()
  qc.setQueryData(queryKeys.sessionMemory('s1'), stalled)
  const row = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(MemoryBadge, { session: 's1', summary: summary('endless'), row: true, open: false, onToggle() {} }) as ReactNode }))
  expect(row).toContain('summaries failing')
  const cleared = render(summary('endless'), { ...stalled, stalled: null })
  expect(cleared).toContain('motion-safe:animate-pulse')
  expect(cleared).not.toContain('text-warning')
})

test('the footer shows the failing row above the settle bar: count, error, next try, model, the blocking line and both actions', () => {
  vi.useFakeTimers({ now: NOW })
  const html = footer({ ...memory, settling: true, unbuilt: 3, stalled: failing })
  vi.useRealTimers()
  expect(html).toContain('Summaries are failing · 4 attempts · ollama/qwen3:8b: request timed out after 120 s')
  expect(html).toContain('Next try in 40 s · model ollama/qwen3:8b')
  expect(html).toContain('Newer lines wait behind this one.')
  expect(html).toContain('>Retry now</button>')
  expect(html).toContain('>Change model</button>')
  // 177 U5: the failing model can be tried once from here.
  expect(html).toContain('>Test summary call</button>')
  expect(html).toContain('Settling 3 lines')
  const below = footer({ ...memory, stalled: { ...failing, failures: 1, blocking: false } })
  expect(below).toContain('· 1 attempt ·')
  expect(below).not.toContain('Newer lines')
  expect(below).not.toContain('Settling')
  // Cleared: back to the plain settle bar, or nothing once settled.
  const settling = footer({ ...memory, settling: true, unbuilt: 3, stalled: null })
  expect(settling).toContain('Settling 3 lines')
  expect(settling).not.toContain('Summaries are failing')
  expect(footer({ ...memory, stalled: null })).toBe('')
})

test('next try: relative while waiting, "in a moment" once due, "Retrying now…" while the retry runs', () => {
  expect(nextTry(failing, NOW)).toBe('Next try in 40 s')
  expect(nextTry({ ...failing, next_retry_at: '2026-11-04T22:03:00Z' }, NOW)).toBe('Next try in 3 min')
  expect(nextTry(failing, NOW + 39_000)).toBe('Next try in a moment')
  expect(nextTry(failing, NOW + 60_000)).toBe('Next try in a moment')
  expect(nextTry({ ...failing, next_retry_at: null }, NOW)).toBe('Retrying now…')
})

test('Retry now: the reply replaces the cached memory; an error toasts and leaves the cache as it was', async () => {
  const qc = new QueryClient()
  const before = { ...memory, stalled: failing }
  qc.setQueryData(queryKeys.sessionMemory('s1'), before)
  const fresh = { ...before, stalled: { ...failing, next_retry_at: null } }
  const endless_retry = vi.fn(async () => fresh)
  await new MutationObserver(qc, retryMutation({ endless_retry } as unknown as Api, qc, 's1')).mutate()
  expect(endless_retry).toHaveBeenCalledWith('s1')
  expect(qc.getQueryData(queryKeys.sessionMemory('s1'))).toEqual(fresh)

  qc.setQueryData(queryKeys.sessionMemory('s1'), before)
  const cached = qc.getQueryData(queryKeys.sessionMemory('s1'))
  const broken = { endless_retry: async () => Promise.reject(new Error('not allowed')) } as unknown as Api
  await expect(new MutationObserver(qc, retryMutation(broken, qc, 's1')).mutate()).rejects.toThrow('not allowed')
  expect(toast.error).toHaveBeenCalledWith('not allowed')
  expect(qc.getQueryData(queryKeys.sessionMemory('s1'))).toBe(cached)
})

// ---------------------------------------------------------------------------------------------------------------- 137G

const waiting: EndlessBacklog = { lines: 4_812, folded: 4_512, eta_secs: 660, paused: false, paused_by: null }
const backlogged: SessionMemory = { ...memory, units: 5_868, settling: true, unbuilt: 4_812, backlog: waiting, spend_today_usd: 5.12 }

/** The footer over a cache that may hold the endless settings (batch size, day cap). */
const footerWith = (data: SessionMemory, settings?: object) => {
  const qc = new QueryClient()
  if (settings) qc.setQueryData(queryKeys.endlessSettings, settings)
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(TooltipProvider, null, createElement(MemoryFooter, { session: 's1', m: data })) as ReactNode }))
}

test('backlog row: count, ETA, batch size, the share of lines summarized, what the view shows, Fold now and Pause settling; it replaces the settle bar', () => {
  const html = footerWith(backlogged)
  expect(html).toContain('Settling 4,812 lines')
  expect(html).toContain('~11 min · batches of 16')
  expect(html).toContain('aria-valuenow="18"')
  expect(html).toContain('Newest 300 lines shown in full; older ones are folded until summarized. Model anthropic/claude-haiku-4-5.')
  expect(html).toContain('>Fold now</button>')
  expect(html).toContain('>Pause settling</button>')
  expect(html).toContain('Fold now: turns every waiting line into a short excerpt at once, at no cost. Recall still finds the originals.')
  expect(html).not.toContain('Settling 4812 lines')
  expect(html).not.toContain('%</span>')
  // The batch size is the setting's; no ETA while unknown.
  expect(footerWith({ ...backlogged, backlog: { ...waiting, eta_secs: 0 } }, { batch_units: 8 })).toContain('>batches of 8</span>')
  // Failing and waiting: the failing block keeps its place above, now with Fold now too.
  const both = footerWith({ ...backlogged, stalled: failing })
  expect(both.indexOf('Summaries are failing')).toBeLessThan(both.indexOf('Settling 4,812 lines'))
  expect(both.match(/>Fold now<\/button>/g)).toHaveLength(2)
  expect(both).toContain('Lines that fail 3 times are folded as excerpts automatically.')
})

test('paused by the user: "Settling paused", the waiting count, Resume settling; the badge says so and stops pulsing', () => {
  const m = { ...backlogged, settling: false, backlog: { ...waiting, eta_secs: 0, paused: true, paused_by: 'user' } }
  const html = footerWith(m)
  expect(html).toContain('Settling paused')
  expect(html).toContain('4,812 lines waiting')
  expect(html).toContain('No summary calls run for this chat. Turns still fit: older waiting lines stay folded.')
  expect(html).toContain('>Resume settling</button>')
  expect(html).toContain('>Fold now</button>')
  expect(html).not.toContain('Pause settling')
  expect(badgeTip(m)).toMatch(/· settling paused$/)
  expect(render(summary('endless'), { ...m, settling: true })).not.toContain('animate-pulse')
})

test("paused by spend: the day's limit (the setting, else $5) and today's spend, Resume for today, the link to the setting", () => {
  const m = { ...backlogged, settling: false, backlog: { ...waiting, eta_secs: 0, paused: true, paused_by: 'spend' } }
  const html = footerWith(m)
  expect(html).toContain('Paused: today&#x27;s summary spend limit ($5.00) reached')
  expect(html).toContain('$5.12 spent')
  expect(html).toContain('>Resume for today</button>')
  expect(html).toContain('>Spend limit settings</button>')
  expect(html).not.toContain('Settling paused')
  expect(footerWith(m, { max_usd_per_session_day: 2.5 })).toContain('($2.50) reached')
})

test('Fold now, Pause/Resume and the summary model call their RPC and write the reply into the cache ("Use default" sends null)', async () => {
  const qc = new QueryClient()
  const key = queryKeys.sessionMemory('s1')
  const reply = (patch: Partial<SessionMemory>) => vi.fn(async () => ({ ...backlogged, ...patch }))
  const api = {
    endless_fold: reply({ backlog: null, settling: false }),
    endless_pause: reply({ backlog: { ...waiting, paused: true, paused_by: 'user' } }),
    set_session_compactor_model: reply({ model: 'openai/gpt-5-mini', model_override: 'openai/gpt-5-mini', model_source: 'chat' }),
    set_session_compactor_effort: reply({ effort: 'high' }),
  }
  qc.setQueryData(key, backlogged)
  await new MutationObserver(qc, foldMutation(api as unknown as Api, qc, 's1')).mutate()
  expect(api.endless_fold).toHaveBeenCalledWith('s1')
  expect(qc.getQueryData<SessionMemory>(key)?.backlog).toBeNull()
  await new MutationObserver(qc, pauseMutation(api as unknown as Api, qc, 's1')).mutate(true)
  expect(api.endless_pause).toHaveBeenCalledWith('s1', true)
  expect(qc.getQueryData<SessionMemory>(key)?.backlog?.paused_by).toBe('user')
  await new MutationObserver(qc, pauseMutation(api as unknown as Api, qc, 's1')).mutate(false)
  expect(api.endless_pause).toHaveBeenLastCalledWith('s1', false)
  await new MutationObserver(qc, modelMutation(api as unknown as Api, qc, 's1')).mutate('openai/gpt-5-mini')
  expect(api.set_session_compactor_model).toHaveBeenCalledWith('s1', 'openai/gpt-5-mini')
  expect(qc.getQueryData<SessionMemory>(key)?.model_source).toBe('chat')
  await new MutationObserver(qc, modelMutation(api as unknown as Api, qc, 's1')).mutate(null)
  expect(api.set_session_compactor_model).toHaveBeenLastCalledWith('s1', null)
  // 194: the effort chips; the Default chip sends null.
  await new MutationObserver(qc, effortMutation(api as unknown as Api, qc, 's1')).mutate('high')
  expect(api.set_session_compactor_effort).toHaveBeenCalledWith('s1', 'high')
  expect(qc.getQueryData<SessionMemory>(key)?.effort).toBe('high')
  await new MutationObserver(qc, effortMutation(api as unknown as Api, qc, 's1')).mutate(null)
  expect(api.set_session_compactor_effort).toHaveBeenLastCalledWith('s1', null)
})

test('the summary model picker shows the model in force and which link chose it', () => {
  const paint = (m: SessionMemory) => {
    const qc = new QueryClient()
    qc.setQueryData(queryKeys.models, [{ id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5', provider: 'anthropic', context: 200_000, efforts: [] }])
    return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(SummaryModel, { session: 's1', m }) as ReactNode }))
  }
  const html = paint({ ...memory, model_source: 'agent' })
  expect(html).toMatch(/<button[^>]*id="memory-summary-model"[^>]*aria-label="Summary model"/)
  expect(html).toContain('Claude Haiku 4.5</span>')
  expect(html).toContain('agent default')
  expect(paint({ ...memory, model_source: 'chat' })).toContain('this chat')
  expect(paint({ ...memory, model_source: 'session' })).toContain('chat model')
  // 177 U4: a local model in force carries the tag.
  const qc = new QueryClient()
  qc.setQueryData(queryKeys.models, [{ id: 'mlx/LFM3', name: 'LFM3', provider: 'mlx', context: 0, efforts: [], local: true }])
  const local = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(SummaryModel, { session: 's1', m: { ...memory, model: 'mlx/LFM3' } }) as ReactNode }))
  expect(local).toMatch(/LFM3<\/span><span[^>]*>local<\/span>/)
})

test('194: the summary picker carries the effort chips of the model in force, at the effort in force; none for a model without efforts', () => {
  const paint = (m: SessionMemory, efforts: string[]) => {
    const qc = new QueryClient()
    qc.setQueryData(queryKeys.models, [{ id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5', provider: 'anthropic', context: 200_000, efforts }])
    pickers.length = 0
    renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(SummaryModel, { session: 's1', m }) as ReactNode }))
    return pickers.find((p) => p['aria-label'] === 'Summary model')
  }
  const chips = paint({ ...memory, effort: 'high' }, ['low', 'high'])?.effort
  expect(chips?.levels).toEqual(['low', 'high'])
  expect(chips?.current).toBe('high')
  expect(paint({ ...memory, effort: null }, ['low', 'high'])?.effort?.current).toBeNull()
  expect(paint(memory, [])?.effort).toBeUndefined()
})

test('both placeholder kinds are muted italic notes; a mechanical line carries the excerpt tag, a model line none', () => {
  // `fold.rs` `folded_text` / `prefix_text` (no thousands separator from the engine; the fixture's commas match too).
  expect(isFoldedNote('(4512 lines not yet summarized — recall searches them)')).toBe(true)
  expect(isFoldedNote('(4,512 lines not yet summarized — recall searches them)')).toBe(true)
  expect(isFoldedNote('(1 lines not yet summarized — recall searches them)')).toBe(true)
  expect(isFoldedNote('(300 older lines folded to fit the view — zoom or recall reach them)')).toBe(true)
  expect(isFoldedNote('user: why are 3 lines not yet summarized?')).toBe(false)
  expect(isFoldedNote('(3 older lines were discussed)')).toBe(false)
  const note = renderToStaticMarkup(createElement(ViewNote, { line: line(1_056, 4_512, '(4,512 lines not yet summarized — recall searches them)', false) }))
  expect(note).toMatch(/^<p role="note" class="[^"]*text-fg-tertiary italic/)
  const node = (l: ViewLine) =>
    renderToStaticMarkup(createElement(TooltipProvider, null, createElement(LineNode, { line: l, prefix: '', depth: 0, kids: {}, open: new Set<string>(), cursorKey: undefined, query: '', onToggle() {} })))
  const excerpt = node({ ...line(2, 1, '…desk wake loop: 3 retries …'), mechanical: true })
  expect(excerpt).toContain('aria-label="Excerpt — not a model summary; recall finds the original"')
  expect(excerpt).toContain('>excerpt</span>')
  expect(node(line(2, 1, 'a model summary'))).not.toContain('excerpt')
})

test('after a fold the footer counts the excerpt units from the view itself; none left, no row', () => {
  const folded = { ...memory, units: 70, view: [line(0, 64, 'the beginning'), { ...line(64, 1, '…a … b'), mechanical: true }, { ...line(65, 1, '…c … d'), mechanical: true }, line(66, 1, 'merged')] }
  const html = footerWith(folded)
  expect(html).toContain('Up to date')
  expect(html).toContain('2 lines shown as excerpts')
  expect(footerWith({ ...folded, settling: true, backlog: waiting })).not.toContain('shown as excerpts')
  expect(footerWith(memory)).toBe('')
})
