import { defaultFilter } from 'cmdk'
import { expect, test } from 'vitest'
import type { ConfigOption } from '@/rpc/bindings/ConfigOption'
import { dialogPlacement, harnessCurrent, harnessGroups, harnessItemValue, pinnedFilter, slashTarget } from './ModelMenu'

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

test('127 §6 dialogPlacement: touch anchors the dialog to the top of the visible band and lets the list fill it; else centred', () => {
  expect(dialogPlacement(false)).toEqual({ content: '', list: '', picker: '', footer: '' })
  const phone = dialogPlacement(true)
  expect(phone.content.split(' ')).toEqual(expect.arrayContaining(['top-[max(0.5rem,env(safe-area-inset-top))]', 'translate-y-0', 'flex', 'flex-col', 'max-h-[calc(var(--visible-band)-1rem)]']))
  expect(phone.content).not.toMatch(/translate-x|left-/)
  expect(phone.list.split(' ')).toEqual(expect.arrayContaining(['max-h-none', 'min-h-0', 'flex-1']))
  // 125 F2: the Command root stops claiming the whole band (`h-full`) so the Advisor row stays a visible footer.
  expect(phone.picker.split(' ')).toEqual(expect.arrayContaining(['h-auto', 'min-h-0', 'flex-1']))
  expect(phone.footer.split(' ')).toContain('shrink-0')
})

test('157 D1 slashTarget: /model reaches the focused pane, any draft, and a page-own picker whatever is focused', () => {
  expect(slashTarget('s1', 's1', false)).toBe(true)
  expect(slashTarget('s1', 's2', false)).toBe(false)
  expect(slashTarget('s1', null, false)).toBe(false)
  expect(slashTarget(null, 's2', false)).toBe(true)
  // The notebook's chat column: no split pane ever focuses its thread, so it answers on its own.
  expect(slashTarget('s1', 's2', true)).toBe(true)
  expect(slashTarget('s1', null, true)).toBe(true)
})
