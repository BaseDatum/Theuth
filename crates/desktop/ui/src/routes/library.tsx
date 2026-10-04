import { createFileRoute, redirect } from '@tanstack/react-router'

/** 97 D1: `/library` is a convenience alias; the Library's first tab is Artifacts (Sky, 2026-09-27). */
export const Route = createFileRoute('/library')({
  beforeLoad: () => {
    throw redirect({ to: '/artifacts', replace: true })
  },
})
