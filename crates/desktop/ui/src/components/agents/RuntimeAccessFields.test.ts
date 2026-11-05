import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { RuntimeFields } from './RuntimeAccessFields'
import { type Draft, emptyDraft, toFile } from './wizard'

const pick: Record<string, (id: string | null) => void> = {}
vi.mock('@/components/chat/ModelCombobox', () => ({
  ModelCombobox: ({ value, none, onChange, 'aria-label': label }: { value: string | null | undefined; none?: { label: string }; onChange(id: string | null): void; 'aria-label'?: string }) => {
    if (label) pick[label] = onChange
    return createElement('span', { 'aria-label': label }, value ?? none?.label)
  },
}))
vi.mock('./queries', () => ({ useModels: () => ({ data: [] }), useHarnesses: () => ({ data: [] }), useMcpServers: () => ({ data: [] }) }))
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
