import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactElement, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Router from '@tanstack/react-router'
import type * as Rpc from '@/lib/rpc'
import type { DraftField } from '@/rpc/bindings/DraftField'
import type { DraftView } from '@/rpc/bindings/DraftView'
import { DraftPanel } from './DraftPanel'
import { studies } from './fixtures'
import { KEY } from './queries'

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
  expect(ready).toContain('Everything is settled.')
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
