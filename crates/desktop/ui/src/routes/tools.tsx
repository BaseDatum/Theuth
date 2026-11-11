import { createFileRoute, redirect } from '@tanstack/react-router'

/** Agents › Tools became Settings › Tools › MCP (Sky, 2026-11-11): old links follow, keeping the chat scope (`?session=`). */
export const Route = createFileRoute('/tools')({
  validateSearch: (s: Record<string, unknown>): { session?: string } => ({
    ...(typeof s.session === 'string' && { session: s.session }),
  }),
  beforeLoad: ({ search }) => {
    throw redirect({ to: '/settings/$tab', params: { tab: 'tools' }, search, replace: true })
  },
})
