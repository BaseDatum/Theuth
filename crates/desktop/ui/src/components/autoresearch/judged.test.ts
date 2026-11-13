import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactElement, createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { JudgmentRow } from "@/rpc/bindings/method.autoresearch.output"
import { PairList, PairViewer, pairsOf } from './PairViewer'
import { RatingLadder } from './RatingLadder'

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({ 'autoresearch.judgments': async () => [], 'autoresearch.human_pair': async () => ({}) }),
}))

function paint(el: ReactElement): string {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: el }))
}

const row = (id: number, n: number, caseId: string, over: Partial<JudgmentRow> = {}): JudgmentRow => ({
  id: id,
  slug: 'tj',
  n,
  case_id: caseId,
  split: 'val',
  a_sha: 'exp:1',
  b_sha: 'base000',
  a_first: true,
  model: 'fake/judge',
  score: null,
  winner: 'a',
  reason: 'covers every feature',
  human: false,
  cost_nanos: 4_000_000,
  at: '2026-10-10T16:00:00Z',
  ...over,
})

test('pairsOf: two ordered verdicts make one pair, agreement decides the outcome, probe rows are dropped, pending human pairs sort first', () => {
  const rows = [
    row(1, 0, 'p01', { b_sha: 'base000', a_sha: 'base000' }), // probe
    row(2, 1, 'p07'),
    row(3, 1, 'p07', { a_first: false }),
    row(4, 1, 'p12', { winner: 'a' }),
    row(5, 1, 'p12', { a_first: false, winner: 'b' }),
    row(6, 2, 'p03', { winner: 'b' }),
    row(7, 2, 'p03', { a_first: false, winner: 'b' }),
    row(8, 2, 'p03', { model: 'human', winner: null, reason: '', cost_nanos: 0 }),
  ]
  const pairs = pairsOf(rows)
  expect(pairs.map((p) => `${p.n}:${p.caseId}:${p.outcome}`)).toEqual(['2:p03:b', '1:p07:a', '1:p12:tie'])
  expect(pairs[0]?.human?.human).toBe(false)
  expect(pairs[1]?.verdicts).toHaveLength(2)
  expect(pairs[1]?.cost).toBeCloseTo(0.008)
})

test('rating ladder: one rung per version with its Elo and games, the base named, the agreement figure', () => {
  const html = paint(
    createElement(RatingLadder, {
      view: {
        kind: 'ratings',
        ladder: [
          { sha: 'kept1abc', theta: 1.2, se: 0.3, games: 40, elo: 1208.5, n: 3 },
          { sha: 'base000', theta: 0, se: 0.2, games: 60, elo: 1000, n: null },
        ],
        agreement: { agreed: 2, asked: 3 },
      },
    }),
  )
  expect(html).toContain('data-kind="ratings"')
  expect(html).toContain('data-rung="kept1abc"')
  expect(html).toContain('exp 3')
  expect(html).toContain('Elo 1209')
  expect(html).toContain('base')
  expect(html).toContain('2/3 pairs')
  expect(html).not.toContain('No judged pairs yet')
})

test('rating ladder: empty says so, without an agreement figure', () => {
  const html = paint(createElement(RatingLadder, { view: { kind: 'ratings', ladder: [], agreement: null } }))
  expect(html).toContain('No judged pairs yet')
  expect(html).not.toContain('data-agreement')
})

test('pair viewer: a static paint shows the loading line (the query resolves client-side)', () => {
  const html = paint(createElement(PairViewer, { slug: 'tj', n: null }))
  expect(html).toContain('Loading pairs')
})

test('pair list: a pending human pair is blind — buttons only, no verdicts, reasons, outcome or order labels for that case (R-W3-1)', () => {
  const rows = [
    row(6, 2, 'p03', { winner: 'b', reason: 'the judge liked B' }),
    row(7, 2, 'p03', { a_first: false, winner: 'b', reason: 'still B' }),
    row(8, 2, 'p03', { model: 'human', winner: null, reason: '', cost_nanos: 0 }),
    row(2, 1, 'p07', { a_first: false, reason: 'answered case reason' }),
    row(3, 1, 'p07', { a_first: false }),
    row(4, 1, 'p07', { model: 'human', winner: 'a', human: true, reason: '', cost_nanos: 0 }),
  ]
  const html = paint(createElement(PairList, { pairs: pairsOf(rows), busy: false, onAnswer: () => {} }))
  expect(html).toContain('data-pending="1"')
  expect(html).toContain('data-pair="2:p03" data-human="pending"')
  expect(html).not.toContain('the judge liked B')
  expect(html).not.toContain('still B')
  expect(html).not.toContain('incumbent</span>')
  expect(html).not.toContain('(candidate as A)')
  // The answered case shows the judge's side and the agreement.
  expect(html).toContain('answered case reason')
  expect(html).toContain('agrees with the judge')
})

test('pair list: a pending pair with a_text/b_text shows both texts above the buttons, still blind (159 W3 hand-off)', () => {
  const rows = [
    row(6, 2, 'p03', { winner: 'b', reason: 'the judge liked B' }),
    row(7, 2, 'p03', { a_first: false, winner: 'b', reason: 'still B' }),
    row(8, 2, 'p03', { model: 'human', winner: null, reason: '', cost_nanos: 0, a_text: 'Buy now — ships today.', b_text: 'Order today and it ships tomorrow.' }),
  ]
  const html = paint(createElement(PairList, { pairs: pairsOf(rows), busy: false, onAnswer: () => {} }))
  expect(html).toMatch(/data-side="A"[^>]*><div[^>]*>A<\/div><pre[^>]*>Buy now — ships today.<\/pre>/)
  expect(html).toMatch(/data-side="B"[^>]*><div[^>]*>B<\/div><pre[^>]*>Order today and it ships tomorrow.<\/pre>/)
  expect(html.indexOf('data-side="A"')).toBeLessThan(html.indexOf('>same</button>'))
  expect(html).not.toContain('the judge liked B')
  expect(html).not.toContain('still B')
  expect(html).not.toContain('(candidate as A)')
  // Without texts (the row predates the hand-off) no panes are drawn.
  const bare = paint(createElement(PairList, { pairs: pairsOf(rows.slice(0, 2).concat(row(9, 2, 'p03', { model: 'human', winner: null, reason: '', cost_nanos: 0 }))), busy: false, onAnswer: () => {} }))
  expect(bare).not.toContain('data-side=')
})
