import { expect, test } from 'vitest'
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { modelsOfProvider, providersOf, rankModels, searchTerms, termTier } from './model-search'

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
  local: false,
  ...extra,
})

// 124 §2.3 fixture, in catalog order (provider, id, name).
const catalog = [
  model('amazon-bedrock', 'amazon-bedrock/amazon.nova-pro-v1:0', 'Nova Pro'),
  model('amazon-bedrock', 'amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0', 'Claude Sonnet 4.5'),
  model('anthropic', 'anthropic/claude-opus-4-1', 'Claude Opus 4.1'),
  model('anthropic', 'anthropic/claude-sonnet-4-5', 'Claude Sonnet 4.5'),
  model('cursor', 'cursor/gpt-5.4', 'GPT-5.4'),
  model('google', 'google/gemini-2.5-flash-thinking', 'Gemini 2.5 Flash Thinking'),
  model('google', 'google/gemini-2.5-pro', 'Gemini 2.5 Pro'),
  model('openai', 'openai/gpt-5.4', 'GPT-5.4'),
  model('openai', 'openai/gpt-5.4-mini', 'GPT-5.4 mini'),
  model('openai', 'openai/o3-pro', 'o3-pro'),
]

test('124 §2.3 vectors: every row of the table, ids in order', () => {
  const vectors: [string, string[]][] = [
    ['gpt', ['cursor/gpt-5.4', 'openai/gpt-5.4', 'openai/gpt-5.4-mini']],
    ['pro', ['amazon-bedrock/amazon.nova-pro-v1:0', 'google/gemini-2.5-pro', 'openai/o3-pro']],
    ['anthropic sonnet', ['anthropic/claude-sonnet-4-5', 'amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0']],
    ['sonnet 4.5', ['amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0', 'anthropic/claude-sonnet-4-5']],
    ['claude', ['amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0', 'anthropic/claude-opus-4-1', 'anthropic/claude-sonnet-4-5']],
    ['mini', ['openai/gpt-5.4-mini']],
    ['aude', ['amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0', 'anthropic/claude-opus-4-1', 'anthropic/claude-sonnet-4-5']],
    ['GPT-5.4', ['cursor/gpt-5.4', 'openai/gpt-5.4', 'openai/gpt-5.4-mini']],
    ['anth', ['anthropic/claude-opus-4-1', 'anthropic/claude-sonnet-4-5', 'amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0']],
    ['openai mini', ['openai/gpt-5.4-mini']],
    ['nova pro', ['amazon-bedrock/amazon.nova-pro-v1:0']],
    ['v1:0', ['amazon-bedrock/amazon.nova-pro-v1:0', 'amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0']],
    ['xyz', []],
  ]
  for (const [query, ids] of vectors) {
    expect(rankModels(catalog, query).map((m) => m.id), query).toEqual(ids)
  }
  // Browse mode: no terms, no search.
  expect(searchTerms('  ')).toEqual([])
  expect(rankModels(catalog, '  ')).toBe(catalog)
})

test('124 §2.3 scores: the tiers the table quotes', () => {
  const byId = (id: string) => catalog.find((m) => m.id === id)!
  expect(termTier(byId('cursor/gpt-5.4'), 'gpt')).toBe(90)
  expect(termTier(byId('google/gemini-2.5-pro'), 'pro')).toBe(80)
  expect(termTier(byId('anthropic/claude-sonnet-4-5'), 'anthropic')).toBe(70)
  expect(termTier(byId('anthropic/claude-sonnet-4-5'), 'anth')).toBe(65)
  expect(termTier(byId('amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0'), 'anthropic')).toBe(60)
  expect(termTier(byId('google/gemini-2.5-flash-thinking'), 'mini')).toBe(40)
  expect(termTier(byId('anthropic/claude-opus-4-1'), 'thropic')).toBe(30)
  expect(termTier(byId('cursor/gpt-5.4'), 'gpt-5.4')).toBe(100)
  expect(termTier(byId('openai/gpt-5.4-mini'), 'gpt-5.4')).toBe(90)
  expect(termTier(byId('google/gemini-2.5-flash-thinking'), 'gpt')).toBe(0)
})

test('providersOf counts A→Z; modelsOfProvider sorts by name then id', () => {
  expect(providersOf(catalog)).toEqual([
    { id: 'amazon-bedrock', count: 2 },
    { id: 'anthropic', count: 2 },
    { id: 'cursor', count: 1 },
    { id: 'google', count: 2 },
    { id: 'openai', count: 3 },
  ])
  expect(providersOf([])).toEqual([])
  expect(modelsOfProvider(catalog, 'openai').map((m) => m.id)).toEqual(['openai/gpt-5.4', 'openai/gpt-5.4-mini', 'openai/o3-pro'])
  const ties = [model('p', 'p/b', 'Same'), model('p', 'p/a', 'same'), model('p', 'p/c', 'Alpha')]
  expect(modelsOfProvider(ties, 'p').map((m) => m.id)).toEqual(['p/c', 'p/a', 'p/b'])
  expect(modelsOfProvider(catalog, 'nope')).toEqual([])
})
