import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/briefs/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { BriefSearch } from '@/components/briefs/BriefSearch'

export type BriefsSearch = { q?: string; id?: string }

/** 05 §7: the brief cache, a tab of the meetings view — search, list by tag, edit, delete, new. `?q=` prefills the box; `?id=` outlines one row. */
export const Route = createFileRoute('/meetings/briefs')({
  validateSearch: (s: Record<string, unknown>): BriefsSearch => {
    const out: BriefsSearch = {}
    if (typeof s.q === 'string' && s.q !== '') out.q = s.q
    if (typeof s.id === 'string' && s.id !== '') out.id = s.id
    return out
  },
  component: withCapability(BriefsRoute, requirements),
})

function BriefsRoute() {
  const { q, id } = Route.useSearch()
  return <BriefSearch initial={q ?? ''} focus={id ?? null} />
}
