import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/knowledge/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { Explorer } from '@/components/knowledge/Explorer'

export type KnowledgeSearch = { q?: string; history?: boolean }

/** `?q=` prefills the search box (the composer's `/knowledge <query>`); `?history=true` opens the whole bundle's history at bare `/knowledge`. */
export const Route = createFileRoute('/knowledge')({
  validateSearch: (s: Record<string, unknown>): KnowledgeSearch => {
    const out: KnowledgeSearch = {}
    if (typeof s.q === 'string' && s.q !== '') out.q = s.q
    if (s.history === true) out.history = true
    return out
  },
  component: withCapability(Explorer, requirements),
})
