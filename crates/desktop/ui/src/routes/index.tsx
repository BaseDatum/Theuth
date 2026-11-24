import { createFileRoute, redirect } from '@tanstack/react-router'

/**
 * `/` is the new-chat page. The THEUTH mark's overview (70 HUD) that lived here is retired (Sky, 2026-11-24); every
 * `Link to="/"` and `navigate({ to: '/' })` (a deleted or forgotten chat, the pairing flow's way home) lands on `/new`,
 * a `?project=` carried along.
 */
export const Route = createFileRoute('/')({
  validateSearch: (s: Record<string, unknown>): { project?: string } =>
    typeof s.project === 'string' && s.project !== '' ? { project: s.project } : {},
  beforeLoad: ({ search }) => {
    throw redirect({ to: '/new', search, replace: true })
  },
})
