import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { SETTINGS } from '@/components/autoresearch/fixtures'
import type { AutoresearchConfig } from '@/rpc/bindings/AutoresearchConfig'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { RpcError } from '@/rpc/client'
import { Autoresearch, awaitsRestart } from './Autoresearch'

const sonnet = { id: 'anthropic/claude-sonnet-4-5', name: 'Claude Sonnet 4.5', efforts: ['low', 'high'] } as ModelInfo
let cfg: AutoresearchConfig = SETTINGS
const set = vi.fn(async (c: AutoresearchConfig) => c)
vi.mock('@/components/autoresearch/queries', () => ({
  KEY: ['autoresearch'],
  useAutoresearchApi: () => ({ 'autoresearch.settings.set': set }),
  useAutoresearchSettings: () => ({ data: cfg, isError: false }),
  useMechanism: () => ({ isSuccess: true, error: null }),
}))
vi.mock('@/components/agents/queries', () => ({ useModels: () => ({ data: [sonnet] }) }))
vi.mock('@/components/settings/Mechanism', () => ({ Mechanism: () => null }))
vi.mock('@tanstack/react-router', () => ({ Link: () => null }))
// Static markup drops handlers: each picker registers its effort setter by label.
const chips: Record<string, ((level: string | null) => void) | undefined> = {}
vi.mock('@/components/chat/ModelCombobox', () => ({
  ModelCombobox: ({ effort, 'aria-label': label }: { effort?: { levels: string[]; current: string | null | undefined; onSelect(l: string | null): void }; 'aria-label': string }) => {
    chips[label] = effort?.onSelect
    return createElement('span', { 'aria-label': label, 'data-effort': effort ? (effort.current ?? 'default') : undefined })
  },
}))

const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient(), children: createElement(Autoresearch) }))

const running = { isSuccess: true, error: null }
const off = { isSuccess: false, error: new RpcError(-32001, 'autoresearch is not running') }

test('188 F3: a saved value the engine does not run waits for a restart; one it runs does not', () => {
  expect(awaitsRestart(true, off)).toBe(true)
  expect(awaitsRestart(false, running)).toBe(true)
  expect(awaitsRestart(true, running)).toBe(false)
  expect(awaitsRestart(false, off)).toBe(false)
})

test('188 F3: unknown until both sides answered — settings loading, the probe loading or failing for another reason', () => {
  expect(awaitsRestart(undefined, off)).toBeNull()
  expect(awaitsRestart(true, { isSuccess: false, error: null })).toBeNull()
  expect(awaitsRestart(true, { isSuccess: false, error: new RpcError(-32000, 'ask failed') })).toBeNull()
  expect(awaitsRestart(false, { isSuccess: false, error: new Error('NOT_RUNNING') })).toBeNull()
})

test('194 row 19: a role picked on a model with levels shows its own effort as chips; a chip saves the section with that role’s effort', async () => {
  cfg = { ...SETTINGS, models: { ...SETTINGS.models, executor: sonnet.id, executor_effort: 'high' }, review: { ...SETTINGS.review, model: sonnet.id } }
  const html = paint()
  expect(html).toContain('aria-label="Executor model" data-effort="high"')
  expect(html).toContain('aria-label="Reviewer model" data-effort="default"')
  expect(html).not.toContain('aria-label="Advisor model" data-effort')
  chips['Executor model']?.('low')
  await vi.waitFor(() => expect(set).toHaveBeenLastCalledWith({ ...cfg, models: { ...cfg.models, executor_effort: 'low' } }))
  chips['Reviewer model']?.('high')
  await vi.waitFor(() => expect(set).toHaveBeenLastCalledWith({ ...cfg, review: { ...cfg.review, effort: 'high' } }))
  chips['Executor model']?.(null)
  await vi.waitFor(() => expect(set).toHaveBeenLastCalledWith({ ...cfg, models: { ...cfg.models, executor_effort: null } }))
})
