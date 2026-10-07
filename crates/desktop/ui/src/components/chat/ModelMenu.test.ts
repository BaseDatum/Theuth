import { defaultFilter } from 'cmdk'
import { expect, test } from 'vitest'
import type { ConfigOption } from '@/rpc/bindings/ConfigOption'
import { harnessCurrent, harnessGroups, harnessItemValue, pinnedFilter } from './ModelMenu'

const opt = (id: string, category: string | null, current: string, values: [string, string][]): ConfigOption => ({
  id,
  name: id,
  category,
  current,
  options: values.map(([value, name]) => ({ value, name, description: null })),
})

test('harness picker groups the model select and the thought_level option; a queued change shows as pending', () => {
  // omp: mode, model, thinking; opencode without variants: model, mode only.
  const omp = [
    opt('mode', 'mode', 'default', [['default', 'Default']]),
    opt('model', 'model', 'anthropic/claude-opus-4-8', [['anthropic/claude-opus-4-8', 'Claude Opus 4.8'], ['openai/gpt-5.4', 'GPT-5.4']]),
    opt('thinking', 'thought_level', 'high', [['off', 'Off'], ['high', 'High'], ['xhigh', 'Extra high']]),
  ]
  const g = harnessGroups(omp)
  expect(g.model?.id).toBe('model')
  expect(g.effort?.id).toBe('thinking')
  expect(harnessGroups(omp.slice(0, 2)).effort).toBeUndefined()
  // No category: the id/name fallback finds the model select.
  expect(harnessGroups([opt('llm', null, 'x', [['x', 'X']]), opt('Active model', null, 'y', [['y', 'Y']])]).model?.id).toBe('Active model')

  expect(harnessCurrent(g.model, undefined)).toBe('Claude Opus 4.8')
  expect(harnessCurrent(g.model, 'openai/gpt-5.4')).toBe('→ openai/gpt-5.4 (after this turn)')
  expect(harnessCurrent(opt('m', 'model', 'raw/id', []), undefined)).toBe('raw/id')
  expect(harnessCurrent(undefined, undefined)).toBeUndefined()
})

test('pinnedFilter keeps the pinned value at 1 and defers to defaultFilter otherwise', () => {
  const f = pinnedFilter('Claude Opus 4.8 anthropic/claude-opus-4-8')
  expect(f('Claude Opus 4.8 anthropic/claude-opus-4-8', 'gpt')).toBe(1)
  expect(f('  claude opus 4.8 ANTHROPIC/claude-opus-4-8 ', 'gpt')).toBe(1)
  expect(f('GPT-5.4 openai/gpt-5.4', 'zzz')).toBe(0)
  const hit = f('GPT-5.4 openai/gpt-5.4', 'gpt')
  expect(hit).toBeGreaterThan(0)
  expect(hit).toBe(defaultFilter('GPT-5.4 openai/gpt-5.4', 'gpt'))
  const plain = pinnedFilter(undefined)
  for (const [v, s] of [
    ['GPT-5.4 openai/gpt-5.4', 'gpt'],
    ['Claude Opus 4.8 anthropic/claude-opus-4-8', 'zzz'],
    ['Model High high', 'hi'],
  ] as const) {
    expect(plain(v, s)).toBe(defaultFilter(v, s))
  }
})

test('harness item values are the exact cmdk strings', () => {
  expect(harnessItemValue('Model', { name: 'GPT-5.4', value: 'openai/gpt-5.4' })).toBe('Model GPT-5.4 openai/gpt-5.4')
})
