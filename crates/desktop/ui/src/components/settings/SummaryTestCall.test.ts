import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { SummaryTest } from '@/rpc/bindings/SummaryTest'
import { summaryTestStats } from './providers'
import { SummaryTestResult, SummaryTestView } from './SummaryTestCall'

const MODEL = 'mlx/LFM3-1.7B-MLX-bf16'
const ok: SummaryTest = { ok: true, model: MODEL, latency_ms: 2_140n, tokens_in: 1_873, tokens_out: 41, reasoning_tokens: null, finish_reason: 'stop', reasoning_seen: false, thinking_off_sent: true, line: 'Sky asks why omlx refuses max_tokens 0.', error: null, hint: null }
const failed: SummaryTest = { ...ok, ok: false, latency_ms: 21_600n, tokens_out: 2_048, reasoning_tokens: 2_048, finish_reason: 'length', reasoning_seen: true, line: null, error: `${MODEL} used its whole 2048-token reply on thinking (finish_reason=length)`, hint: 'Turn thinking off for this model in omlx.' }
const text = (html: string) => html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')
const view = (p: Partial<Parameters<typeof SummaryTestView>[0]>) => renderToStaticMarkup(createElement(SummaryTestView, { model: MODEL, pending: false, result: undefined, onTest() {}, ...p }))
const button = (html: string) => html.match(/<button[^>]*>/)?.[0] ?? ''

test('177 U5: a successful test shows the line, latency, tokens in and out, the finish reason and "thinking off sent"; a failed one the error and the hint', () => {
  const result = (t: SummaryTest) => text(renderToStaticMarkup(createElement(SummaryTestResult, { t })))
  expect(result(ok)).toContain('Summary call worked Sky asks why omlx refuses max_tokens 0. mlx/LFM3-1.7B-MLX-bf16 · 2.1 s · 1,873 in · 41 out · finish stop · thinking off sent')
  const bad = result(failed)
  expect(bad).toContain('mlx/LFM3-1.7B-MLX-bf16 used its whole 2048-token reply on thinking (finish_reason=length) Turn thinking off for this model in omlx.')
  expect(bad).toContain('21.6 s · 1,873 in · 2,048 out · 2,048 thinking · finish length · thinking off sent')
  expect(bad).not.toContain('Summary call worked')
  // Unreported numbers are left out, not shown as 0.
  expect(summaryTestStats({ ...ok, tokens_in: null, tokens_out: null, finish_reason: null, thinking_off_sent: false, latency_ms: 900n })).toBe('0.9 s')
})

test('177 R1-10: disabled without a model; busy while the call runs ("Testing…", disabled, the wait note, no stale result); then the result', () => {
  const none = view({ model: null })
  expect(button(none)).toContain('disabled=""')
  expect(text(none)).toContain('Test summary call')
  const idle = view({})
  expect(button(idle)).not.toContain('disabled=""')
  const busy = view({ pending: true, result: ok })
  expect(button(busy)).toContain('disabled=""')
  expect(text(busy)).toContain('Testing…')
  expect(text(busy)).toContain(`Waiting for ${MODEL}; a local model can take tens of seconds.`)
  expect(text(busy)).not.toContain('Summary call worked')
  expect(text(view({ result: failed }))).toContain('used its whole 2048-token reply on thinking')
})
