import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactElement, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import type * as Rpc from '@/lib/rpc'
import type { DraftField } from '@/rpc/bindings/DraftField'
import type { DraftView } from '@/rpc/bindings/DraftView'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { DraftPanel, LeafModel, footerText } from './DraftPanel'
import { studies } from './fixtures'
import { KEY } from './queries'
import { ADVANCED } from './setup'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({}),
}))
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof Router>()),
  Link: ({ children }: { children?: unknown }) => createElement('a', { href: '#' }, children as never),
}))

function paint(el: ReactElement, seed?: (qc: QueryClient) => void): string {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  seed?.(qc)
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: el }))
}

const study = studies()[0]!
const field = (name: string, state: DraftField['state'], value = `${name} value`, evidence: string | null = null): DraftField => ({ name, value, state, evidence })

/** A mid-setup draft (the mock's second screen): every §5 state once, Scope and When still proposed. */
const DRAFT: DraftView = {
  draft_id: 'tui-startup',
  spec: { ...study.detail.spec, slug: 'tui-startup', window: null },
  origin: 'setup-1',
  fields: [
    field('goal', 'confirmed', 'First frame faster; output unchanged'),
    field('repo', 'verified', '~/Develop/theuth @ HEAD', '584660bf · clean tree'),
    field('metric', 'confirmed', 'startup_ms · lower · keep ≥ 3%'),
    field('bench', 'verified', 'bench.sh · 10 launches', '5 runs · 412 ms · ±1.6%'),
    field('checks', 'missing', ''),
    field('holdout', 'optional', ''),
    field('scope', 'proposed', 'crates/tui/src/** − render/**'),
    field('budget', 'default', '$20 · 120 exp · 10 h'),
    field('window', 'proposed', 'Idle window, tonight'),
    field('parallel', 'default', '1'),
  ],
  settled: 6,
  total: 9,
  ready: false,
  blocking: ['Check is missing', 'Scope needs your OK', 'When needs your OK'],
  dirty: false,
  partial: true,
  version: 4,
  updated_at: '2026-10-09T12:00:00Z',
  probes: [{ kind: 'bench', ok: true, values: [410, 415, 412, 409, 413], median: 412, noise_pct: 1.6, unit: 'ms', samples: 5, duration_ms: 2100, exit: 0, log_tail: '', at: '2026-10-09T12:00:00Z', fingerprint: 'f' }],
  started: false,
}

test('draft panel: a badge per state in row order, Advanced collapsed, the sparkline on the verified bench, OK only on proposed rows', () => {
  const html = paint(createElement(DraftPanel, { draft: DRAFT, session: 's1' }))
  const badges = [...html.matchAll(/data-row="([a-z_]+)" data-state="([a-z]+)"/g)].map((m) => `${m[1]}:${m[2]}`)
  expect(badges).toEqual(['goal:confirmed', 'repo:verified', 'metric:confirmed', 'bench:verified', 'checks:missing', 'holdout:optional', 'scope:proposed', 'budget:default', 'window:proposed'])
  for (const word of ['Confirmed', 'Verified', 'Missing', 'Optional', 'Needs your OK', 'Default']) expect(html).toContain(`>${word}<`)
  expect(html).toContain('6 of 9 settled')
  expect(html).toContain('584660bf · clean tree')
  expect(html).toContain('<polyline')
  expect((html.match(/aria-label="OK /g) ?? []).length).toBe(2)
  expect(html).toContain('aria-label="OK Scope"')
  expect(html).toContain(' Advanced</button>')
  expect(html).not.toContain('data-row="parallel"')
})

test('draft panel: Start is disabled while not ready and the footer names what blocks it; ready unlocks and a dirty tree adds the third button', () => {
  const html = paint(createElement(DraftPanel, { draft: DRAFT, session: 's1' }))
  expect(html).toContain('Start unlocks when: Check is missing · Scope needs your OK · When needs your OK')
  expect(html).toMatch(/<button[^>]*disabled=""[^>]*>[^<]*<svg[^>]*>.*?<\/svg> Start now<\/button>/)
  expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Start in idle window<\/button>/)
  expect(html).not.toContain('Start with my uncommitted changes')
  expect(html).toContain(' Discard</button>')
  const ready = paint(createElement(DraftPanel, { draft: { ...DRAFT, ready: true, blocking: [], dirty: true }, session: 's1' }))
  expect(ready).not.toMatch(/<button[^>]*disabled=""[^>]*>Start in idle window<\/button>/)
  expect(ready).toMatch(/<button[^>]*>Start in idle window<\/button>/)
  expect(ready).toContain('Start with my uncommitted changes')
  // R1 H5: ready, but theuth's proposal still stands on some rows — the footer names them, never "Everything is settled".
  expect(ready).toContain('Ready to start · Scope, When are theuth’s proposals — OK or edit them')
  expect(ready).not.toContain('Everything is settled')
  const budget = { ...DRAFT, ready: true, blocking: [], fields: DRAFT.fields.map((f) => (f.name === 'budget' ? { ...f, state: 'proposed' as const } : f.state === 'proposed' ? { ...f, state: 'confirmed' as const } : f)) }
  expect(footerText(budget)).toBe('Ready to start · Budget is theuth’s proposal — OK or edit it')
  expect(footerText({ ...budget, fields: budget.fields.map((f) => (f.name === 'budget' ? { ...f, state: 'confirmed' as const } : f)) })).toBe('Everything is settled. Nothing runs on your repo until you press Start.')
  expect(footerText({ ...budget, ready: false, blocking: ['Check is missing'] })).toBe('Start unlocks when: Check is missing')
})

test('draft panel (R1 U1): each row is label + badge + controls on one line with the value below; Edit on every row, OK only on proposed ones', () => {
  const html = paint(createElement(DraftPanel, { draft: DRAFT, session: 's1' }))
  expect((html.match(/aria-label="Edit /g) ?? []).length).toBe(DRAFT.fields.filter((f) => !ADVANCED[f.name]).length)
  expect(html).toContain('aria-label="Edit Budget"')
  // The value is its own full-width block after the header line, mono only for commands/paths.
  expect(html).toMatch(/data-row="bench"[^>]*>.*?<\/span><\/div><div class="[^"]*font-mono[^"]*">bench\.sh · 10 launches<\/div>/)
  expect(html).toMatch(/data-row="goal"[^>]*>.*?<\/span><\/div><div class="(?![^"]*font-mono)[^"]*">First frame faster; output unchanged<\/div>/)
})

test('draft panel: skeleton rows before the first draft; the status card from autoresearch.get once started', () => {
  const empty = paint(createElement(DraftPanel, { draft: undefined, session: null }))
  expect(empty).toContain('theuth fills this in as you talk.')
  expect(empty).not.toContain('Start now')
  const slug = study.info.slug
  const started = paint(createElement(DraftPanel, { draft: { ...DRAFT, draft_id: slug, started: true }, session: 's1' }), (qc) => qc.setQueryData([...KEY, slug, 'detail'], study.detail))
  expect(started).toContain('>Study<')
  expect(started).not.toContain('Needs your OK')
  expect(started).toContain('Experiments')
  expect(started).toContain(`${study.info.experiments}/${study.info.budget.experiments}`)
  expect(started).toContain('Open study')
  expect(started).toMatch(/Pause|Resume/)
  expect(started).toContain('Stop')
})

test('194 row 20: a `models.*` leaf picker carries its `_effort` leaf as chips; a model that does not take the level drops it, Default clears both', () => {
  const sonnet = { id: 's/sonnet', name: 'Sonnet', efforts: ['low', 'high'] } as ModelInfo
  const plain = { id: 'p/plain', name: 'Plain', efforts: [] as string[] } as ModelInfo
  type Props = { effort?: { levels: string[]; current: string | null; onSelect(l: string | null): void }; onChange(id: string | null): void }
  const values = { 'models.executor': sonnet.id, 'models.executor_effort': 'high' }
  const setValues = vi.fn()
  const picker = (v: Record<string, string>) => (LeafModel({ leaf: 'models.executor', values: v, setValues, models: [sonnet, plain] }) as ReactElement<Props>).props
  expect(picker(values).effort).toMatchObject({ levels: ['low', 'high'], current: 'high' })
  picker(values).effort!.onSelect('low')
  expect(setValues).toHaveBeenLastCalledWith({ ...values, 'models.executor_effort': 'low' })
  picker(values).onChange(plain.id)
  expect(setValues).toHaveBeenLastCalledWith({ 'models.executor': plain.id, 'models.executor_effort': '' })
  picker(values).onChange(null)
  expect(setValues).toHaveBeenLastCalledWith({ 'models.executor': '', 'models.executor_effort': '' })
  expect(picker({ ...values, 'models.executor': plain.id }).effort).toBeUndefined()
  // A row without the effort leaf (a kind row's model) gets no chips.
  expect(picker({ 'models.executor': sonnet.id }).effort).toBeUndefined()
})
