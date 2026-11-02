import { createFileRoute } from '@tanstack/react-router'
import { useContext } from 'react'
import { CostsPage } from '@/components/costs/CostsPage'
import { ResidentCtx } from '@/components/costs/Resident'
import { validateCostsSearch } from '@/lib/costs/search'

/**
 * 160: the full-history costs dashboard; range, granularity, grouping, metric, compare and filters are search params (D9).
 * R2: in the shell the page is resident (`CostsResident` in `__root`), so the route itself renders nothing there; a
 * popped-out window has no host and renders the page here.
 */
export const Route = createFileRoute('/costs')({ validateSearch: validateCostsSearch, component: Page })

function Page() {
  const search = Route.useSearch()
  return useContext(ResidentCtx) ? null : <CostsPage search={search} visible />
}
