import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { HarnessGlyph, harnessKey, shortModel } from './harnessGlyph'

test('harness keys resolve case-insensitively by prefix, ignore an @node suffix, cover observed sources, and fall back to the terminal', () => {
  expect(harnessKey(null)).toBe('theuth')
  expect(harnessKey(undefined)).toBe('theuth')
  expect(harnessKey('')).toBe('theuth')
  expect(harnessKey('theuth@studio')).toBe('theuth')
  expect(harnessKey('omp')).toBe('omp')
  expect(harnessKey('omp@P50')).toBe('omp')
  expect(harnessKey('OMP')).toBe('omp')
  expect(harnessKey('omp_task')).toBe('omp')
  expect(harnessKey('claude')).toBe('claude')
  expect(harnessKey('claude-acp')).toBe('claude')
  expect(harnessKey('Claude-ACP@xeus')).toBe('claude')
  expect(harnessKey('opencode')).toBe('opencode')
  expect(harnessKey('opencode-v2')).toBe('opencode')
  expect(harnessKey('opencode_ext')).toBe('opencode')
  expect(harnessKey('opencode_task')).toBe('opencode')
  expect(harnessKey('codex')).toBe('codex')
  expect(harnessKey('codex-acp')).toBe('codex')
  expect(harnessKey('gemini-cli')).toBe('terminal')
  expect(harnessKey('rfd')).toBe('terminal')
  expect(harnessKey('@P50')).toBe('theuth')
})

test('every glyph is a single-colour line drawing (currentColor stroke, no fill)', () => {
  for (const agent of [null, 'omp', 'claude-acp', 'opencode', 'codex', 'mystery']) {
    const svg = renderToStaticMarkup(createElement(HarnessGlyph, { agent, className: 'size-3.5' }))
    expect(svg).toMatch(/^<svg/)
    expect(svg).toContain('stroke="currentColor"')
    expect(svg).toContain('fill="none"')
    expect(svg).toContain('size-3.5')
  }
})

test('shortModel drops the provider path and date suffixes, dots split versions and abbreviates deepseek', () => {
  expect(shortModel('anthropic/claude-opus-5-5')).toBe('opus-5.5')
  expect(shortModel('anthropic/claude-sonnet-4-5-20250929')).toBe('sonnet-4.5')
  expect(shortModel('claude-3-5-sonnet-20241022')).toBe('3.5-sonnet')
  expect(shortModel('claude-haiku-4-5@20251001')).toBe('haiku-4.5')
  expect(shortModel('rwx/deepseek-ai/DeepSeek-V4.1-Flash')).toBe('ds-v4.1-flash')
  expect(shortModel('openai/gpt-5.1-codex')).toBe('gpt-5.1-codex')
  expect(shortModel('gpt-4o-2024-08-06')).toBe('gpt-4o')
  expect(shortModel('google/gemini-2.5-pro-latest')).toBe('gemini-2.5-pro')
  expect(shortModel('qwen/qwen3-coder-480b-a35b')).toBe('qwen3-coder-480b-a35b')
  expect(shortModel('')).toBe('')
  expect(shortModel(null)).toBe('')
})
