import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import type { EscalateModel } from "@/rpc/bindings/method.agent.output"
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { Helpers } from './Helpers'

// The picker itself is ModelCombobox's own suite; here only what each row hands it.
type Effort = { levels: string[]; current?: string | null; onSelect: (level: string | null) => void }
type Props = { 'aria-label': string; value?: string; effort?: Effort; onChange: (v: string | null) => void }
const pickers: Record<string, Props> = {}
vi.mock('@/components/chat/ModelCombobox', () => ({
  ModelCombobox: (p: Props) => {
    pickers[p['aria-label']] = p
    return null
  },
}))

const think = { id: 'p/think', name: 'Think', image: true, efforts: ['off', 'low', 'high'] } as ModelInfo
const plain = { id: 'p/plain', name: 'Plain', image: true, efforts: [] as string[] } as ModelInfo
let title: ModelInfo | null = think
let escalate: EscalateModel = { model: think, effective: think }
let harvester: ModelInfo | null = null
const api = {
  list_models: vi.fn(async () => [think, plain]),
  title_model: vi.fn(async () => title),
  title_effort: vi.fn(async (): Promise<string | null> => 'high'),
  vision_model: vi.fn(async () => think),
  vision_effort: vi.fn(async (): Promise<string | null> => null),
  escalate_model: vi.fn(async () => escalate),
  escalate_effort: vi.fn(async (): Promise<string | null> => 'low'),
  set_title_model: vi.fn(async () => null),
  set_title_effort: vi.fn(async (e: string | null) => e),
  set_vision_model: vi.fn(async () => null),
  set_vision_effort: vi.fn(async (e: string | null) => e),
  set_escalate_model: vi.fn(async () => null),
  set_escalate_effort: vi.fn(async (e: string | null) => e),
  knowledge_harvester_model: vi.fn(async () => harvester),
  knowledge_harvester_effort: vi.fn(async (): Promise<string | null> => 'low'),
  set_knowledge_harvester_model: vi.fn(async () => null),
  set_knowledge_harvester_effort: vi.fn(async (e: string | null) => e),
}
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => api,
}))

async function render(): Promise<QueryClient> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Helpers) as ReactNode }))
  paint()
  for (const query of qc.getQueryCache().findAll()) await qc.fetchQuery({ ...query.options, queryKey: query.queryKey })
  paint()
  return qc
}

/** Resolves once every mutation started on `qc` has settled (its `onSuccess` invalidations included). */
function settled(qc: QueryClient): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>()
  const off = qc.getMutationCache().subscribe((e) => {
    if (e.type === 'updated' && (e.action.type === 'success' || e.action.type === 'error') && qc.isMutating() === 0) {
      off()
      resolve()
    }
  })
  return promise
}

beforeEach(() => {
  for (const k of Object.keys(pickers)) delete pickers[k]
  title = think
  escalate = { model: think, effective: think }
  harvester = null
})

test('194: Title, Vision fallback and Escalation carry their model’s levels and their stored effort', async () => {
  await render()
  expect(pickers['Title model']?.effort).toMatchObject({ levels: think.efforts, current: 'high' })
  expect(pickers['Vision fallback model']?.effort).toMatchObject({ levels: think.efforts, current: null })
  expect(pickers['Escalation model']?.effort).toMatchObject({ levels: think.efforts, current: 'low' })
})

test('194: a model without levels gets no chips; Escalation reads the row’s own model, not the inherited one', async () => {
  title = plain
  escalate = { model: null, effective: think }
  await render()
  expect(pickers['Title model']?.effort).toBeUndefined()
  expect(pickers['Escalation model']?.effort).toBeUndefined()
})

test('194: a chip calls its own setter; the Default chip sends null', async () => {
  const qc = await render()
  const done = settled(qc)
  pickers['Title model']?.effort?.onSelect('low')
  pickers['Vision fallback model']?.effort?.onSelect(null)
  pickers['Escalation model']?.effort?.onSelect('high')
  await done
  expect(api.set_title_effort).toHaveBeenLastCalledWith('low')
  expect(api.set_vision_effort).toHaveBeenLastCalledWith(null)
  expect(api.set_escalate_effort).toHaveBeenLastCalledWith('high')
})

test('194 D4c/D4d: a model change or Off refetches that row’s effort (the engine may have dropped it)', async () => {
  const qc = await render()
  const done = settled(qc)
  pickers['Title model']?.onChange('p/plain')
  pickers['Vision fallback model']?.onChange(null)
  pickers['Escalation model']?.onChange(null)
  await done
  expect(api.set_title_model).toHaveBeenLastCalledWith('p/plain')
  expect(api.set_vision_model).toHaveBeenLastCalledWith(null)
  expect(api.set_escalate_model).toHaveBeenLastCalledWith(null)
  for (const key of [queryKeys.titleEffort, queryKeys.visionEffort, queryKeys.escalateEffort]) expect(qc.getQueryState(key)?.isInvalidated, String(key)).toBe(true)
})

test('Knowledge harvester model: unset reads the chat’s own model; a pick carries its levels; a change or reset refetches its effort', async () => {
  await render()
  expect(pickers['Knowledge harvester model']).toBeDefined()
  expect(pickers['Knowledge harvester model']?.value).toBeUndefined()
  expect(pickers['Knowledge harvester model']?.effort).toBeUndefined()
  harvester = think
  const qc = await render()
  expect(pickers['Knowledge harvester model']).toMatchObject({ value: 'p/think', effort: { levels: think.efforts, current: 'low' } })
  const done = settled(qc)
  pickers['Knowledge harvester model']?.effort?.onSelect('high')
  pickers['Knowledge harvester model']?.onChange(null)
  await done
  expect(api.set_knowledge_harvester_effort).toHaveBeenLastCalledWith('high')
  expect(api.set_knowledge_harvester_model).toHaveBeenLastCalledWith(null)
  expect(qc.getQueryState(queryKeys.knowledgeHarvesterEffort)?.isInvalidated).toBe(true)
})
