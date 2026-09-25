import { createFileRoute, redirect } from '@tanstack/react-router'

/** `/agents/<name>` (the Queue's link) lands on the overview tab. */
export const Route = createFileRoute('/agents/$name/')({
  beforeLoad: ({ params }) => {
    throw redirect({ to: '/agents/$name/$tab', params: { name: params.name, tab: 'overview' }, replace: true })
  },
})
