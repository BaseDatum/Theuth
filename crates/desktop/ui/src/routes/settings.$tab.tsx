import { createFileRoute, redirect } from '@tanstack/react-router'
import { SettingsPage } from '@/components/settings/SettingsPage'
import { type SettingsSearch, routeTarget } from '@/components/settings/route-target'

export const Route = createFileRoute('/settings/$tab')({
  validateSearch: (s: Record<string, unknown>): SettingsSearch => ({
    ...(typeof s.session === 'string' && { session: s.session }),
    ...(typeof s.peer === 'string' && { peer: s.peer }),
    ...(typeof s.focus === 'string' && { focus: s.focus }),
    ...(typeof s.progress === 'string' && s.progress.length > 0 && { progress: s.progress }),
  }),
  beforeLoad: ({ params, search }) => {
    const target = routeTarget(params.tab, search)
    switch (target.kind) {
      case 'render':
        return
      case 'page':
        throw redirect({ to: target.to, search: target.search, replace: true })
      case 'chat':
        throw redirect({ to: '/c/$session', params: { session: target.session }, search: target.search, replace: true })
      case 'tab':
        throw redirect({ to: '/settings/$tab', params: { tab: target.tab }, search: target.search, replace: true })
    }
  },
  component: Settings,
})

function Settings() {
  const { tab } = Route.useParams()
  const { focus, progress } = Route.useSearch()
  return <SettingsPage tab={tab} focus={focus ?? null} progress={progress ?? null} />
}
