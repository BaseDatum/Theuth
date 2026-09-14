import { createFileRoute } from '@tanstack/react-router'
import { Document } from '@/components/knowledge/Document'

/** `_splat` is the concept id (it contains `/`); `?draft=<title>` opens the editor on a not-yet-saved concept. */
export const Route = createFileRoute('/knowledge/$')({
  validateSearch: (s: Record<string, unknown>): { draft?: string } => (typeof s.draft === 'string' ? { draft: s.draft } : {}),
  component: Document,
})
