import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { Dictation } from './Voice'

// What the Polish row hands the picker (the picker itself is ModelCombobox's own suite).
type Effort = { levels: string[]; current?: string | null; onSelect: (level: string | null) => void }
type Props = { 'aria-label': string; effort?: Effort; onChange: (v: string | null) => void }
const pickers: Record<string, Props> = {}
vi.mock('@/components/chat/ModelCombobox', () => ({
  ModelCombobox: (p: Props) => {
    pickers[p['aria-label']] = p
    return null
  },
}))

const think = { id: 'p/think', name: 'Think', efforts: ['off', 'low', 'high'] } as ModelInfo
const plain = { id: 'p/plain', name: 'Plain', efforts: [] as string[] } as ModelInfo
let polish: ModelInfo | null = think
const api = {
  voice_settings: vi.fn(async () => ({ shape: {}, end_commands: { send: [], scratch: [], literal: 'literal' } })),
  list_models: vi.fn(async () => [think, plain]),
  polish_model: vi.fn(async () => polish),
  polish_effort: vi.fn(async (): Promise<string | null> => 'low'),
  set_polish_model: vi.fn(async () => null),
  set_polish_effort: vi.fn(async (e: string | null) => e),
  voice_mlx_status: vi.fn(async () => ({ engine: 'onnx', available: false, state: { state: 'unavailable', reason: 'test' } })),
}
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => api,
}))

async function render(): Promise<QueryClient> {
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  const paint = () => renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Dictation) as ReactNode }))
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
  polish = think
})

test('194: the Polish row carries the polish model’s levels and `polish_effort`; a model without levels gets none', async () => {
  await render()
  expect(pickers['Polish model']?.effort).toMatchObject({ levels: think.efforts, current: 'low' })
  polish = plain
  await render()
  expect(pickers['Polish model']?.effort).toBeUndefined()
})

test('194: a chip calls set_polish_effort (Default = null); a model change or the chain row refetches the effort', async () => {
  let qc = await render()
  let done = settled(qc)
  pickers['Polish model']?.effort?.onSelect(null)
  await done
  expect(api.set_polish_effort).toHaveBeenLastCalledWith(null)

  qc = await render()
  done = settled(qc)
  pickers['Polish model']?.onChange(null)
  await done
  expect(api.set_polish_model).toHaveBeenLastCalledWith(null)
  expect(qc.getQueryState(['polish-effort'])?.isInvalidated).toBe(true)
})
