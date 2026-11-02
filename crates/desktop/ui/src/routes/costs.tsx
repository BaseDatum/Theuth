import { createFileRoute } from '@tanstack/react-router'
import { CostsPage } from '@/components/costs/CostsPage'
import { validateCostsSearch } from '@/lib/costs/search'

/** 160: the full-history costs dashboard; range, granularity, grouping, metric, compare and filters are search params (D9). */
export const Route = createFileRoute('/costs')({ validateSearch: validateCostsSearch, component: CostsPage })
