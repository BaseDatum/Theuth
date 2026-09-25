import { createFileRoute, redirect } from '@tanstack/react-router'

/** `/routines/<name>` → its Overview. */
export const Route = createFileRoute('/routines/$name/')({
  beforeLoad: ({ params }) => {
    throw redirect({ to: '/routines/$name/$section', params: { name: params.name, section: 'overview' } })
  },
})
