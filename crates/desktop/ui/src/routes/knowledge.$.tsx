import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/knowledge/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { Document } from '@/components/knowledge/Document'

export type DocumentSearch = { draft?: string; history?: boolean; change?: string; snapshot?: string }

/** Full object ids only: the history reader never guesses from an abbreviation. */
const FULL_COMMIT = /^[0-9a-f]{40}$|^[0-9a-f]{64}$/

/**
 * `_splat` is the concept id (it contains `/`). Three modes (49 §7):
 * `?draft=<title>` opens the editor on a not-yet-saved concept (wins over anything historical);
 * `?change=<commit>` opens History at that commit, `&snapshot=<commit>` reads that snapshot instead of the current text;
 * `?history=true` opens the concept's History sheet.
 */
export const Route = createFileRoute('/knowledge/$')({
  validateSearch: (s: Record<string, unknown>): DocumentSearch => {
    if (typeof s.draft === 'string') return { draft: s.draft }
    const out: DocumentSearch = {}
    if (s.history === true) out.history = true
    if (typeof s.change === 'string' && FULL_COMMIT.test(s.change)) {
      out.change = s.change
      if (typeof s.snapshot === 'string' && FULL_COMMIT.test(s.snapshot)) out.snapshot = s.snapshot
    }
    return out
  },
  component: withCapability(Document, requirements),
})
