import { createFileRoute, redirect, useNavigate, useRouter } from '@tanstack/react-router'
import { movedTab } from '@/components/library/moved'
import { movedSettingsTab } from '@/components/routines/helpers'
import { SettingsDialog } from '@/components/settings/SettingsDialog'
import { MOVED_TABS, isTab } from '@/components/settings/registry'
import { isMacDesktop } from '@/lib/hotkeys'

/** `?focus=model|spend`: the control the memory tab focuses. `?session` / `?peer` are what old links to the per-chat rows carried (188: those rows move to the chat). */
type Search = { session?: string; peer?: string; focus?: string }

export const Route = createFileRoute('/settings/$tab')({
  validateSearch: (s: Record<string, unknown>): Search => ({
    ...(typeof s.session === 'string' && { session: s.session }),
    ...(typeof s.peer === 'string' && { peer: s.peer }),
    ...(typeof s.focus === 'string' && { focus: s.focus }),
  }),
  beforeLoad: ({ params, search }) => {
    // Skills and Tools became Agents pages, Secrets a Library page, and Schedules became Routines; old links and `/settings/<tab>` bookmarks follow.
    const moved = movedTab(params.tab) ?? movedSettingsTab(params.tab)
    if (moved) throw redirect({ to: moved, replace: true, search: search.session ? { session: search.session } : {} })
    // 188: tabs of the old modal that were renamed or split (and 183's `mesh`).
    const renamed = MOVED_TABS[params.tab]
    if (renamed) throw redirect({ to: '/settings/$tab', params: { tab: renamed }, search, replace: true })
    if (!isTab(params.tab)) throw redirect({ to: '/settings/$tab', params: { tab: 'appearance' }, search })
  },
  component: Settings,
})

function Settings() {
  const { tab } = Route.useParams()
  const { focus } = Route.useSearch()
  const router = useRouter()
  const navigate = useNavigate()
  // The inset behind the modal is otherwise empty; keep the header band draggable.
  return (
    <>
      <div data-tauri-drag-region={isMacDesktop() || undefined} className="h-[52px]" />
      <SettingsDialog
        tab={tab}
        focus={focus ?? null}
        onTab={(t) => void navigate({ to: '/settings/$tab', params: { tab: t }, search: {}, replace: true })}
        onClose={() => {
          if (router.history.canGoBack()) router.history.back()
          else void navigate({ to: '/' })
        }}
      />
    </>
  )
}
