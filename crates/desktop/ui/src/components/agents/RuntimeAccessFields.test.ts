import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { RuntimeFields } from './RuntimeAccessFields'
import { type Draft, emptyDraft, toFile } from './wizard'

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
  expect(html).toContain('Memory summary model')
  expect(html).toContain('<span aria-label="Memory summary model">anthropic/claude-haiku-4-5</span>')
  pick['Memory summary model']?.('ollama/qwen3:8b')
  expect(JSON.parse(JSON.stringify(toFile(d, false))).memory_model).toBe('ollama/qwen3:8b')
  pick['Memory summary model']?.(null)
  expect(JSON.parse(JSON.stringify(toFile(d, false)))).not.toHaveProperty('memory_model')
  expect(renderToStaticMarkup(createElement(RuntimeFields, { d, set }))).toContain('<span aria-label="Memory summary model">Use default</span>')
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
  expect(chips['Memory summary model']?.levels).toEqual(['low', 'high'])
  chips['Memory summary model']?.onSelect('high')
  expect(JSON.parse(JSON.stringify(toFile(d, false))).memory_effort).toBe('high')
  renderToStaticMarkup(createElement(RuntimeFields, { d, set }))
  expect(chips['Memory summary model']?.current).toBe('high')
  pick['Memory summary model']?.('ollama/qwen3:8b')
  expect(JSON.parse(JSON.stringify(toFile(d, false)))).not.toHaveProperty('memory_effort')
  renderToStaticMarkup(createElement(RuntimeFields, { d, set }))
  expect(chips['Memory summary model']).toBeUndefined()
  d = { ...d, memory_model: 'anthropic/claude-haiku-5-5', memory_effort: 'low' }
  renderToStaticMarkup(createElement(RuntimeFields, { d, set }))
  pick['Memory summary model']?.(null)
  expect(JSON.parse(JSON.stringify(toFile(d, false)))).not.toHaveProperty('memory_effort')
  models.data = []
})
