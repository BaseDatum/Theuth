import { createFileRoute, redirect, useNavigate, useRouter } from '@tanstack/react-router'
import { movedTab } from '@/components/library/moved'
import { movedSettingsTab } from '@/components/routines/helpers'
import { SettingsDialog, type Tab, isTab } from '@/components/settings/SettingsDialog'
import { isMacDesktop } from '@/lib/hotkeys'

/** `?session=<id>`: the chat the dialog was opened from, for per-session rows; `?peer=<message id>` focuses one pending peer request on the communication tab. */
type Search = { session?: string; peer?: string }

export const Route = createFileRoute('/settings/$tab')({
  validateSearch: (s: Record<string, unknown>): Search => ({
    ...(typeof s.session === 'string' && { session: s.session }),
    ...(typeof s.peer === 'string' && { peer: s.peer }),
  }),
  beforeLoad: ({ params, search }) => {
    // Skills, Tools and Secrets became Library pages and Schedules became Routines; old links and `/settings/<tab>` bookmarks follow.
    const moved = movedTab(params.tab) ?? movedSettingsTab(params.tab)
    if (moved) throw redirect({ to: moved, replace: true, search: search.session ? { session: search.session } : {} })
    if (!isTab(params.tab))
      throw redirect({
        to: '/settings/$tab',
        params: { tab: 'general' },
        search,
      })
  },
  component: Settings,
})

function Settings() {
  const tab = Route.useParams().tab as Tab
  const { session, peer } = Route.useSearch()
  const router = useRouter()
  const navigate = useNavigate()
  // The inset behind the modal is otherwise empty; keep the header band draggable.
  return (
    <>
      <div data-tauri-drag-region={isMacDesktop() || undefined} className="h-[52px]" />
      <SettingsDialog
        tab={tab}
        session={session ?? null}
        peer={peer ?? null}
        onTab={(t) =>
          void navigate({
            to: '/settings/$tab',
            params: { tab: t },
            search: { session },
            replace: true,
          })
        }
        onClose={() => {
          if (router.history.canGoBack()) router.history.back()
          else void navigate({ to: '/' })
        }}
      />
    </>
  )
}
