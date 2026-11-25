import { expect, test } from 'vitest'
import { modelLabels, modelStep } from './model-label'

test('the text steps from a display name: drop effort and a vendor word, then keep the family', () => {
  expect(modelLabels('Claude Opus 5.5', 'medium')).toEqual({ full: 'Claude Opus 5.5 · medium', short: 'Opus 5.5', family: 'Opus' })
  expect(modelLabels('Claude Fable 5.2', 'high')).toEqual({ full: 'Claude Fable 5.2 · high', short: 'Fable 5.2', family: 'Fable' })
  // A versioned first word is the family, not a vendor.
  expect(modelLabels('GPT-6 mini', null)).toEqual({ full: 'GPT-6 mini', short: 'GPT-6 mini', family: 'GPT' })
  expect(modelLabels('DeepSeek-V4.1-Flash')).toEqual({ full: 'DeepSeek-V4.1-Flash', short: 'DeepSeek-V4.1-Flash', family: 'DeepSeek' })
  expect(modelLabels('Gemini 3 Pro').short).toBe('Gemini 3 Pro')
})

test('the text steps from a bare id (no display name)', () => {
  expect(modelLabels('claude-opus-5-5', 'low')).toEqual({ full: 'claude-opus-5-5 · low', short: 'opus-5-5', family: 'opus' })
  expect(modelLabels('gpt-6-mini')).toEqual({ full: 'gpt-6-mini', short: 'gpt-6-mini', family: 'gpt' })
  expect(modelLabels('deepseek-v4.1-flash')).toEqual({ full: 'deepseek-v4.1-flash', short: 'deepseek-v4.1-flash', family: 'deepseek' })
  expect(modelLabels('openrouter/qwen-4-coder')).toEqual({ full: 'openrouter/qwen-4-coder', short: 'qwen-4-coder', family: 'qwen' })
  expect(modelLabels('llama4')).toEqual({ full: 'llama4', short: 'llama4', family: 'llama4' })
})

test('the step is the longest label that fits the room; the icon when none does', () => {
  const widths = [190, 90, 62] as const
  expect(modelStep(400, widths)).toBe(0)
  expect(modelStep(190, widths)).toBe(0)
  expect(modelStep(189, widths)).toBe(1)
  expect(modelStep(62, widths)).toBe(2)
  expect(modelStep(61, widths)).toBe(3)
  expect(modelStep(0, widths)).toBe(3)
})

test('the mock\'s bars: 560 full, 400 `Opus 5.5`, 340 `Opus`, 300 the icon', () => {
  // The room is the bar less its buttons: + · MCP · shield and talk · mic · send (5 × 36 + 28) and the gaps (7 × 4),
  // less the box's 12 px padding each side; the label widths are the trigger's (text + 34 px chrome).
  const buttons = 5 * 36 + 28 + 7 * 4 + 24
  const widths = [34 + 168, 34 + 56, 34 + 30] as const
  const at = (bar: number) => modelStep(bar - buttons, widths)
  expect([560, 400, 340, 300].map(at)).toEqual([0, 1, 2, 3])
})
