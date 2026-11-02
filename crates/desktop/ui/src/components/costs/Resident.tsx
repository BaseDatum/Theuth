// 160 R2: the costs page is resident. It is pre-rendered at idle once the cube is held and stays mounted when you
// leave, laid out off-screen at the real width (hidden, inert, frozen), so opening /costs only flips its visibility:
// Recharts builds a chart over several commits, and paying that on every navigation was the ~300 ms first-paint hitch.
// The `/costs` route renders nothing under this host; a popped-out window (no host) renders the page itself.

import { useRouterState } from '@tanstack/react-router'
import { createContext, lazy, startTransition, Suspense, useEffect, useMemo, useState } from 'react'
import { useCostCube } from '@/lib/costs/queries'
import { type CostsSearch, validateCostsSearch } from '@/lib/costs/search'
import { cn } from '@/lib/utils'

const CostsPage = lazy(() => import('./CostsPage').then((m) => ({ default: m.CostsPage })))

/** `true` under the shell's resident host: the `/costs` route component then renders nothing. */
export const ResidentCtx = createContext(false)

/** How long after the cube lands the hidden page is pre-rendered (idle permitting). */
const PREWARM_MS = 1_500

export function CostsResident() {
  const location = useRouterState({ select: (s) => s.location })
  const here = location.pathname === '/costs'
  const key = here ? JSON.stringify(location.search) : null
  // The page shows the last /costs view while away (it is hidden and frozen then).
  const [last, setLast] = useState<{ key: string; search: CostsSearch } | null>(null)
  const search = useMemo(() => (key !== null ? validateCostsSearch(JSON.parse(key)) : (last?.search ?? {})), [key, last])
  useEffect(() => {
    if (key !== null && key !== last?.key) setLast({ key, search })
  }, [key, last, search])
  const held = useCostCube().cells.hour.length > 0
  const [warm, setWarm] = useState(false)
  useEffect(() => {
    if (warm || !held) return
    const t = setTimeout(() => (typeof requestIdleCallback === 'function' ? requestIdleCallback(() => startTransition(() => setWarm(true)), { timeout: 3_000 }) : startTransition(() => setWarm(true))), PREWARM_MS)
    return () => clearTimeout(t)
  }, [warm, held])
  if (!warm && !here) return null
  return (
    <div
      // Always laid out at the column's size; showing it changes only non-inherited properties (opacity, z-order) and
      // `inert`, so the browser does not restyle the page's thousands of nodes. Hidden: inert (no focus, no pointer, out
      // of the accessibility tree) and transparent.
      className={cn('absolute inset-0 flex flex-col overflow-hidden [contain:strict]', !here && '-z-10 opacity-0')}
      inert={!here}
    >
      <Suspense fallback={null}>
        <CostsPage search={search} visible={here} />
      </Suspense>
    </div>
  )
}
