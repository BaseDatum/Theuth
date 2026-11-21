import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { RuntimeFields } from './RuntimeAccessFields'
import { type Draft, emptyDraft, toFile, validate } from './wizard'

type Effort = { levels: string[]; current?: string | null; onSelect(level: string | null): void }
const pick: Record<string, (id: string | null) => void> = {}
const chips: Record<string, Effort | undefined> = {}
vi.mock('@/components/chat/ModelCombobox', () => ({
  ModelCombobox: ({ value, none, onChange, effort, 'aria-label': label }: { value: string | null | undefined; none?: { label: string }; onChange(id: string | null): void; effort?: Effort; 'aria-label'?: string }) => {
    if (label) {
      pick[label] = onChange
      chips[label] = effort
    }
    return createElement('span', { 'aria-label': label }, value ?? none?.label)
  },
}))
const models = vi.hoisted(() => ({ data: [] as { id: string; efforts: string[] }[] }))
vi.mock('./queries', () => ({ useModels: () => models, useHarnesses: () => ({ data: [] }), useMcpServers: () => ({ data: [] }) }))
vi.mock('@/components/settings/sandbox-queries', () => ({ useSandboxes: () => ({ data: [] }) }))
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children }))

test('137G: the memory summary model shows the file value, writes memory_model, and "Use default" drops the key from the saved file', () => {
  let d: Draft = { ...emptyDraft(), name: 'ops', title: 'Ops', body: 'x', memory_model: 'anthropic/claude-haiku-4-5' }
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => {
    d = { ...d, [k]: v }
  }
  const html = renderToStaticMarkup(createElement(RuntimeFields, { d, set }))
  expect(html).toContain('Context summary model')
  expect(html).toContain('<span aria-label="Context summary model">anthropic/claude-haiku-4-5</span>')
  pick['Context summary model']?.('ollama/qwen3:8b')
  expect(JSON.parse(JSON.stringify(toFile(d, false))).memory_model).toBe('ollama/qwen3:8b')
  pick['Context summary model']?.(null)
  expect(JSON.parse(JSON.stringify(toFile(d, false)))).not.toHaveProperty('memory_model')
  expect(renderToStaticMarkup(createElement(RuntimeFields, { d, set }))).toContain('<span aria-label="Context summary model">Use default</span>')
})

test('194: the memory summary model carries its effort chips, writes memory_effort, and a model without that effort (or "Use default") drops it', () => {
  models.data = [
    { id: 'anthropic/claude-haiku-5-5', efforts: ['low', 'high'] },
    { id: 'ollama/qwen3:8b', efforts: [] },
  ]
  let d: Draft = { ...emptyDraft(), name: 'ops', title: 'Ops', body: 'x', memory_model: 'anthropic/claude-haiku-5-5' }
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => {
    d = { ...d, [k]: v }
  }
  renderToStaticMarkup(createElement(RuntimeFields, { d, set }))
  expect(chips['Context summary model']?.levels).toEqual(['low', 'high'])
  chips['Context summary model']?.onSelect('high')
  expect(JSON.parse(JSON.stringify(toFile(d, false))).memory_effort).toBe('high')
  renderToStaticMarkup(createElement(RuntimeFields, { d, set }))
  expect(chips['Context summary model']?.current).toBe('high')
  pick['Context summary model']?.('ollama/qwen3:8b')
  expect(JSON.parse(JSON.stringify(toFile(d, false)))).not.toHaveProperty('memory_effort')
  renderToStaticMarkup(createElement(RuntimeFields, { d, set }))
  expect(chips['Context summary model']).toBeUndefined()
  d = { ...d, memory_model: 'anthropic/claude-haiku-5-5', memory_effort: 'low' }
  renderToStaticMarkup(createElement(RuntimeFields, { d, set }))
  pick['Context summary model']?.(null)
  expect(JSON.parse(JSON.stringify(toFile(d, false)))).not.toHaveProperty('memory_effort')
  models.data = []
})

test("201: the agent's tail sizing shows only the chosen mode's pair, names a refused pair, blocks the step, and writes the keys", () => {
  const base: Draft = { ...emptyDraft(), name: 'ops', title: 'Ops', body: 'x' }
  const paint = (d: Draft) => renderToStaticMarkup(createElement(RuntimeFields, { d, set: () => {} }))
  const inherit = paint(base)
  expect(inherit).toContain('aria-label="Context tail"')
  expect(inherit).toContain('aria-label="Compactor profile"')
  expect(inherit).not.toContain('Kept after a trim in')
  const fixed: Draft = { ...base, tail_mode: 'fixed', tail_low_bytes: 64_000, tail_high_bytes: 32_000, profile: 'small' }
  const html = paint(fixed)
  expect(html).toMatch(/aria-label="Kept after a trim in KB"[^>]*value="64"/)
  expect(html).toMatch(/aria-label="Trim at in KB"[^>]*value="32"/)
  expect(html).toContain('Trim at (32 KB) must be above Kept after a trim (64 KB).')
  expect(validate('runtime', fixed, []).tail_mode).toBe('Trim at (32 KB) must be above Kept after a trim (64 KB).')
  const scaled = paint({ ...base, tail_mode: 'scaled', tail_low_share: 0.1, tail_high_share: 0.25 })
  expect(scaled).toMatch(/aria-label="Kept after a trim in %"[^>]*value="10"/)
  expect(scaled).toMatch(/aria-label="Trim at in %"[^>]*value="25"/)
  expect(validate('runtime', { ...base, tail_mode: 'scaled', tail_low_share: 0.1, tail_high_share: 0.25 }, [])).toEqual({})
  const file = JSON.parse(JSON.stringify(toFile({ ...fixed, tail_high_bytes: 128_000 }, false)))
  expect([file.tail_mode, file.tail_low_bytes, file.tail_high_bytes, file.profile]).toEqual(['fixed', 64_000, 128_000, 'small'])
})
