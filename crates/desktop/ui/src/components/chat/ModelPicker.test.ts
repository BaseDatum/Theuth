import { expect, test } from 'vitest'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { contextLabel, type View, viewFor } from './ModelPicker'

const model = (provider: string, id: string, name: string, extra: Partial<ModelInfo> = {}): ModelInfo => ({
  id,
  name,
  provider,
  context: 0,
  reasoning: false,
  image: false,
  efforts: [],
  budget_min: null,
  budget_max: null,
  ...extra,
})

const models = [
  model('anthropic', 'anthropic/claude-opus-4-1', 'Claude Opus 4.1', { context: 200_000 }),
  model('anthropic', 'anthropic/claude-sonnet-4-5', 'Claude Sonnet 4.5', { context: 200_000, image: true }),
  model('openai', 'openai/gpt-5.4', 'GPT-5.4', { context: 400_000 }),
  model('openai', 'openai/gpt-5.4-mini', 'GPT-5.4 mini'),
]
const values = (v: View) => v.rows.map((r) => r.value)

test('providers level: Current row, then providers A→Z with counts', () => {
  const v = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: '' })
  expect(v.mode).toBe('providers')
  expect(values(v)).toEqual(['current:openai/gpt-5.4', 'provider:anthropic', 'provider:openai'])
  expect(v.rows[0]).toMatchObject({ kind: 'current', name: 'GPT-5.4', provider: 'openai' })
  expect(v.rows[1]).toMatchObject({ kind: 'provider', count: 2, current: false })
  expect(v.rows[2]).toMatchObject({ kind: 'provider', count: 2, current: true })
  // No current model: no Current row; an unknown current id shows its raw id.
  expect(values(viewFor({ models, model: undefined, provider: null, query: '' }))).toEqual(['provider:anthropic', 'provider:openai'])
  expect(viewFor({ models, model: 'gone/x', provider: null, query: '' }).rows[0]).toMatchObject({ kind: 'current', name: 'gone/x', provider: undefined })
})

test('models level: back row, the provider’s models by name, the current one checked', () => {
  const v = viewFor({ models, model: 'openai/gpt-5.4-mini', provider: 'openai', query: '' })
  expect(v.mode).toBe('models')
  expect(v.provider).toBe('openai')
  expect(values(v)).toEqual(['back', 'model:openai/gpt-5.4', 'model:openai/gpt-5.4-mini'])
  expect(v.rows[2]).toMatchObject({ kind: 'model', current: true })
})

test('search mode: Current row first even when it does not match, never duplicated', () => {
  const v = viewFor({ models, model: 'anthropic/claude-opus-4-1', provider: 'openai', query: 'gpt' })
  expect(v.mode).toBe('search')
  expect(values(v)).toEqual(['current:anthropic/claude-opus-4-1', 'model:openai/gpt-5.4', 'model:openai/gpt-5.4-mini'])
  // The current model matches: it stays the Current row only.
  const self = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: 'gpt-5.4' })
  expect(values(self)).toEqual(['current:openai/gpt-5.4', 'model:openai/gpt-5.4-mini'])
  // Nothing matches: the empty row.
  const none = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: 'xyz' })
  expect(values(none)).toEqual(['current:openai/gpt-5.4', 'empty'])
  // Whitespace only is browse mode; the remembered provider level comes back.
  expect(viewFor({ models, model: 'openai/gpt-5.4', provider: 'openai', query: '  ' }).mode).toBe('models')
})

test('§3.5 single provider: opens on its models with no back row', () => {
  const one = models.filter((m) => m.provider === 'openai')
  const v = viewFor({ models: one, model: 'openai/gpt-5.4', provider: null, query: '' })
  expect(v.mode).toBe('models')
  expect(v.provider).toBe('openai')
  expect(values(v)).toEqual(['model:openai/gpt-5.4', 'model:openai/gpt-5.4-mini'])
  expect(viewFor({ models: one, model: undefined, provider: null, query: 'mini' }).mode).toBe('search')
  expect(viewFor({ models: [], model: undefined, provider: null, query: '' })).toEqual({ mode: 'providers', provider: null, rows: [] })
})

test('forms: the none row sits under Current and is checked when nothing is set; extra ids live under `other`', () => {
  const v = viewFor({ models, model: undefined, provider: null, query: '', none: { label: 'Engine default' }, extra: ['legacy/x', 'openai/gpt-5.4'] })
  expect(values(v)).toEqual(['none', 'provider:anthropic', 'provider:openai', 'provider:other'])
  expect(v.rows[0]).toMatchObject({ kind: 'none', label: 'Engine default', current: true })
  expect(v.rows[3]).toMatchObject({ kind: 'provider', id: 'other', count: 1 })
  const other = viewFor({ models, model: 'legacy/x', provider: 'other', query: '', extra: ['legacy/x'] })
  expect(values(other)).toEqual(['back', 'model:legacy/x'])
  expect(other.rows[1]).toMatchObject({ kind: 'model', current: true, m: { name: 'legacy/x', provider: 'other' } })
  const set = viewFor({ models, model: 'openai/gpt-5.4', provider: null, query: '', none: { label: 'Off' } })
  expect(set.rows[1]).toMatchObject({ kind: 'none', current: false })
  // Search ranks the extra ids too.
  expect(values(viewFor({ models, model: undefined, provider: null, query: 'legacy', extra: ['legacy/x'] }))).toEqual(['model:legacy/x'])
})

test('contextLabel: thousands, empty when unknown', () => {
  expect(contextLabel(200_000)).toBe('200k')
  expect(contextLabel(1_047_576)).toBe('1048k')
  expect(contextLabel(0)).toBe('')
})
