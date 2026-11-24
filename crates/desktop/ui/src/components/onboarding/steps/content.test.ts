import { expect, test } from 'vitest'
import { STARTERS, connectedProviders, contextLabel, recommendModels, starterPrompt } from './content'

const m = (id: string, context = 200_000) => ({ id, name: id.split('/')[1] ?? id, provider: id.split('/')[0] ?? '', context })
const catalog = [
  m('anthropic/claude-haiku-4-5'),
  m('anthropic/claude-sonnet-4-5', 1_000_000),
  m('anthropic/claude-opus-4-5'),
  m('openai/gpt-5-mini', 400_000),
  m('openai/gpt-5', 400_000),
  m('openai/text-embedding-3-large', 8000),
  m('openai/gpt-4o-mini-tts', 8000),
  m('openrouter/qwen/qwen3-coder', 256_000),
]

test('the provider just connected leads, flagships before light models, no non-chat models', () => {
  const ids = recommendModels(catalog, 'openai', null).map((x) => x.id)
  expect(ids.slice(0, 2)).toEqual(['openai/gpt-5', 'openai/gpt-5-mini'])
  expect(ids).toHaveLength(4)
  expect(ids.some((id) => /embedding|tts/.test(id))).toBe(false)
})

test('at most two per provider while others are connected, and the current default stays in view', () => {
  const picked = recommendModels(catalog, 'anthropic', 'openrouter/qwen/qwen3-coder')
  expect(picked[0]?.id).toBe('openrouter/qwen/qwen3-coder')
  expect(picked.filter((x) => x.provider === 'anthropic').map((x) => x.id)).toEqual(['anthropic/claude-sonnet-4-5', 'anthropic/claude-opus-4-5'])
})

test('a single-provider catalog still offers three', () => {
  const only = [m('anthropic/claude-haiku-4-5'), m('anthropic/claude-sonnet-4-5'), m('anthropic/claude-opus-4-5')]
  expect(recommendModels(only, null, null)).toHaveLength(3)
  expect(recommendModels([], null, null)).toEqual([])
})

test('context sizes read as people say them', () => {
  expect(contextLabel(200_000)).toBe('200k context')
  expect(contextLabel(1_000_000)).toBe('1M context')
  expect(contextLabel(0)).toBe('')
  expect(connectedProviders(catalog)).toEqual(['anthropic', 'openai', 'openrouter'])
})

test('every starter is a complete, specific prompt; input cards carry their value', () => {
  for (const s of STARTERS) expect(starterPrompt(s.id, 'X').length).toBeGreaterThan(150)
  expect(starterPrompt('research', '  solid-state batteries ')).toMatch(/^Research solid-state batteries for me\./)
  expect(starterPrompt('project', '/Users/me/code/app')).toContain('/Users/me/code/app')
  expect(starterPrompt('tour', 'Mac')).toMatch(/^Give me a tour of this Mac\./)
  expect(starterPrompt('tour')).toMatch(/this computer/)
  expect(starterPrompt('project', 'x')).toMatch(/do not modify/)
})
