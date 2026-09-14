import { createFileRoute } from '@tanstack/react-router'
import { Explorer } from '@/components/knowledge/Explorer'

/** `?q=` prefills the search box (the composer's `/knowledge <query>`). */
export const Route = createFileRoute('/knowledge')({
  validateSearch: (s: Record<string, unknown>): { q?: string } => (typeof s.q === 'string' && s.q !== '' ? { q: s.q } : {}),
  component: Explorer,
})
