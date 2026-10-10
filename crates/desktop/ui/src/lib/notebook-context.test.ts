import { expect, test } from 'vitest'
import type { ContextEstimate } from '@/rpc/bindings/ContextEstimate'
import { type EstimateItem, contextLine, fmtTokens, remembered, useContextDefaults } from './notebook-context'

const store: Record<string, string> = {}
globalThis.localStorage = {
  getItem: (k: string) => store[k] ?? null,
  setItem: (k: string, v: string) => void (store[k] = v),
} as unknown as Storage

test('set, setAll and clear per (notebook, item); the whole thing round-trips through localStorage', () => {
  const s = useContextDefaults.getState()
  s.set('nb1', 's1', 'source', 'full')
  s.set('nb1', 'n1', 'note', 'off')
  s.set('nb2', 's1', 'source', 'mention')
  expect(remembered(useContextDefaults.getState().defaults, 'nb1')).toEqual([
    { item: 's1', kind: 'source', level: 'full' },
    { item: 'n1', kind: 'note', level: 'off' },
  ])
  s.setAll('nb1', [{ id: 's1', kind: 'source' }, { id: 's2', kind: 'source' }], 'summary')
  s.clear('nb1', 'n1')
  const nb1 = remembered(useContextDefaults.getState().defaults, 'nb1')
  expect(nb1).toEqual([
    { item: 's1', kind: 'source', level: 'summary' },
    { item: 's2', kind: 'source', level: 'summary' },
  ])
  expect(remembered(useContextDefaults.getState().defaults, 'nb2')).toEqual([{ item: 's1', kind: 'source', level: 'mention' }])
  expect(remembered(useContextDefaults.getState().defaults, 'nb3')).toEqual([])
  expect(JSON.parse(store['notebook-context'] as string)).toEqual(useContextDefaults.getState().defaults)
})

const item = (id: string, kind: EstimateItem['kind'], level: EstimateItem['level'], tokens = 100, status: EstimateItem['status'] = 'available'): EstimateItem => ({ id, kind, title: id, level, pinned: false, tokens, status })

test('the §5.2 context line: counts per kind with the level breakdown, ~tokens, omitted, pending changes; empty → the one sentence', () => {
  const e: ContextEstimate = {
    items: [item('s1', 'source', 'summary'), item('s2', 'source', 'summary'), item('s3', 'source', 'full'), item('s4', 'source', 'mention'), item('s5', 'source', 'off', 0, 'omitted'), item('n1', 'note', 'full'), item('n2', 'note', 'full')],
    total_tokens: 12_400,
    omitted: 1,
    empty: false,
    injected: false,
    pending_changes: 0,
  }
  expect(contextLine(e).join(' · ')).toBe('4 sources (2 summary · 1 full · 1 mention) · 2 notes · ~12.4k tokens · 1 omitted')
  expect(contextLine({ ...e, items: [item('s1', 'source', 'summary')], omitted: 0, total_tokens: 940 }).join(' · ')).toBe('1 source (summary) · ~940 tokens')
  expect(contextLine({ ...e, injected: true, pending_changes: 2 }).at(-1)).toBe('2 changes go with your next message')
  expect(contextLine({ ...e, injected: true, pending_changes: 1 }).at(-1)).toBe('1 change goes with your next message')
  expect(contextLine({ ...e, injected: false, pending_changes: 3 })).not.toContain('3 changes go with your next message')
  expect(contextLine({ ...e, injected: true, pending_changes: 0 }).join(' · ')).toBe('4 sources (2 summary · 1 full · 1 mention) · 2 notes · ~12.4k tokens · 1 omitted')
  expect(contextLine({ ...e, empty: true })).toEqual(['nothing in context — the agent sees only the description'])
  expect(fmtTokens(1500)).toBe('1.5k')
})
