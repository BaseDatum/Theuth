// 160 §6.1–6.4: the costs data never loads in front of the user. `CostsPrefetch` (mounted once in the shell) paints the
// IndexedDB snapshot, fetches `cost_cube`, follows `CostCubeChanged` with deltas and writes the merged cube back at idle;
// the page reads it synchronously through `useCostIndex`. `?sim=costs` serves the synthetic fixture instead.

import { keepPreviousData, useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query'
import { useRouter } from '@tanstack/react-router'
import { useEffect } from 'react'
import { costCallsFixture, costsFixture, limitsFixture } from '@/dev/costs-fixture'
import { useApi } from '@/lib/rpc'
import type { CostCallsQuery } from '@/rpc/bindings/CostCallsQuery'
import type { CostCallsPage } from '@/rpc/bindings/CostCallsPage'
import type { CostCube } from '@/rpc/bindings/CostCube'
import { useScope } from '@/store/engine-scope'
import { on } from '@/store/sessions'
import { type Index, emptyCube, index, merge } from './cube'
import { loadCube, saveCube } from './persist'

export const CUBE_KEY = ['costs', 'cube'] as const
export const LIMITS_KEY = ['costs', 'limits'] as const
export const CALLS_KEY = ['costs', 'calls'] as const

export type CostsSim = 'costs' | 'costs-file' | null

/**
 * `?sim=costs`: the committed synthetic fixture. `?sim=costs-file` (dev only): a cube JSON at `VITE_COSTS_FIXTURE_URL`,
 * for local screenshots against a real-shaped cube that is never committed.
 */
export function costsSim(search = typeof location === 'undefined' ? '' : location.search): CostsSim {
  const v = new URLSearchParams(search).get('sim')
  return v === 'costs' ? 'costs' : v === 'costs-file' && import.meta.env.DEV && import.meta.env.VITE_COSTS_FIXTURE_URL ? 'costs-file' : null
}

/** The held cube; `emptyCube()` until the snapshot or the engine answers (never a loading state). */
export function useCostCube(): CostCube {
  const q = useQuery({ queryKey: CUBE_KEY, queryFn: () => emptyCube(), enabled: false, staleTime: Infinity, gcTime: Infinity })
  return q.data ?? EMPTY
}
const EMPTY = emptyCube()

const indexes = new WeakMap<CostCube, Map<string, Index>>()

/** The typed index of the held cube for `tz`, built once per cube. */
export function useCostIndex(tz: string): Index {
  const cube = useCostCube()
  let byTz = indexes.get(cube)
  if (!byTz) indexes.set(cube, (byTz = new Map()))
  let ix = byTz.get(tz)
  if (!ix) byTz.set(tz, (ix = index(cube, tz)))
  return ix
}

const idle = (fn: () => void): void => void (typeof requestIdleCallback === 'function' ? requestIdleCallback(fn, { timeout: 5_000 }) : setTimeout(fn, 0))

/** The snapshot key's engine part: the scope id, or the dev socket URL for the local engine (several local data dirs). */
const engineKey = (scope: string): string => (scope === 'local' ? (import.meta.env.VITE_THEUTH_URL ?? 'local') : scope)

/**
 * The shell's costs bridge (like `RoutineDraftBridge`): renders nothing. Hydrates from IndexedDB, then `cost_cube(since)`;
 * on `CostCubeChanged` one delta request at a time (a newer rev seen mid-flight fetches again once); persists 2 s after
 * the last change, at idle. Preloads `/costs` and the limits snapshot at idle after start.
 */
export function CostsPrefetch() {
  const api = useApi()
  const qc = useQueryClient()
  const router = useRouter()
  const engine = engineKey(useScope().id)
  useEffect(() => {
    let live = true
    const sim = costsSim()
    idle(() => void router.preloadRoute({ to: '/costs' }).catch(() => undefined))
    if (sim === 'costs') {
      const now = Date.now()
      qc.setQueryData(CUBE_KEY, costsFixture(now))
      qc.setQueryData(LIMITS_KEY, limitsFixture(now))
      return
    }
    if (sim === 'costs-file') {
      void fetch(import.meta.env.VITE_COSTS_FIXTURE_URL as string)
        .then((r) => r.json() as Promise<CostCube>)
        .then((c) => live && qc.setQueryData(CUBE_KEY, c))
      idle(() => void qc.prefetchQuery({ queryKey: LIMITS_KEY, queryFn: () => api.usage_limits() }))
      return () => void (live = false)
    }
    let inFlight = false
    let again = false
    let save: ReturnType<typeof setTimeout> | undefined
    const held = () => qc.getQueryData<CostCube>(CUBE_KEY)
    const persist = () => {
      clearTimeout(save)
      save = setTimeout(() => idle(() => void (held()?.epoch && saveCube(engine, held()!).catch(() => undefined))), 2_000)
    }
    const fetchCube = async (): Promise<void> => {
      if (inFlight) return void (again = true)
      inFlight = true
      try {
        const h = held()
        const answer = await api.cost_cube(h?.epoch ? { epoch: h.epoch, rev: h.rev } : null)
        const next = merge(held(), answer) ?? merge(undefined, await api.cost_cube(null))
        // An engine still building answers an empty full cube: keep what we hold (§3.3).
        if (!live || !next || next === held() || (answer.full && !answer.cells.hour.length && held()?.cells.hour.length)) return
        qc.setQueryData(CUBE_KEY, next)
        void qc.invalidateQueries({ queryKey: CALLS_KEY })
        persist()
      } catch {
        // An engine without the method (pre-160) or a dropped socket: keep the held cube; the next change retries.
      } finally {
        inFlight = false
        if (again && live) {
          again = false
          void fetchCube()
        }
      }
    }
    void loadCube(engine).then((disk) => {
      if (live && disk && !held()) qc.setQueryData(CUBE_KEY, disk)
      if (live) void fetchCube()
    })
    idle(() => void qc.prefetchQuery({ queryKey: LIMITS_KEY, queryFn: () => api.usage_limits() }))
    const offs = [
      on('CostCubeChanged', (e) => {
        const h = held()
        if (!h || h.epoch !== e.epoch || e.rev > h.rev) void fetchCube()
      }),
      on('Lagged', () => void fetchCube()),
    ]
    return () => {
      live = false
      clearTimeout(save)
      for (const off of offs) off()
    }
  }, [api, qc, router, engine])
  return null
}

/**
 * `usage_limits()` every 15 s while the page is visible (a memory read on the engine), and `usage_limits_refresh(null)`
 * on mount and every 60 s visible to kick the pollers. `undefined` data = the engine cannot answer.
 */
export function useUsageLimits() {
  const api = useApi()
  const sim = costsSim()
  useEffect(() => {
    if (sim === 'costs') return
    const kick = () => void (document.visibilityState === 'visible' && api.usage_limits_refresh(null).catch(() => undefined))
    kick()
    const t = setInterval(kick, 60_000)
    return () => clearInterval(t)
  }, [api, sim])
  return useQuery({
    queryKey: LIMITS_KEY,
    queryFn: () => (sim === 'costs' ? limitsFixture(Date.now()) : api.usage_limits()),
    refetchInterval: 15_000,
    staleTime: 10_000,
    retry: false,
  })
}

/** The ledger: newest-first keyset pages of `query` (its `before` is the page cursor); previous rows stay while the next arrive. */
export function useCostCalls(query: Omit<CostCallsQuery, 'before'>) {
  const api = useApi()
  const qc = useQueryClient()
  const sim = costsSim()
  return useInfiniteQuery({
    queryKey: [...CALLS_KEY, query],
    queryFn: ({ pageParam }): Promise<CostCallsPage> => {
      const q = { ...query, before: pageParam }
      return sim === 'costs' ? Promise.resolve(costCallsFixture(qc.getQueryData<CostCube>(CUBE_KEY) ?? EMPTY, q)) : api.cost_calls(q)
    },
    initialPageParam: null as CostCallsQuery['before'],
    getNextPageParam: (p: CostCallsPage) => p.next,
    placeholderData: keepPreviousData,
    staleTime: 15_000,
    retry: false,
  })
}
