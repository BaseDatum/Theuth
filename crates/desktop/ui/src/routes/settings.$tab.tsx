import { createFileRoute, redirect, useNavigate, useRouter } from '@tanstack/react-router'
import { SettingsDialog, type Tab, isTab } from '@/components/settings/SettingsDialog'
import { isMacDesktop } from '@/lib/hotkeys'

/** `?session=<id>`: the chat the dialog was opened from, for per-session rows. */
type Search = { session?: string }

export const Route = createFileRoute('/settings/$tab')({
  validateSearch: (s: Record<string, unknown>): Search =>
    typeof s.session === 'string' ? { session: s.session } : {},
  beforeLoad: ({ params, search }) => {
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
  const { session } = Route.useSearch()
  const router = useRouter()
  const navigate = useNavigate()
  // The inset behind the modal is otherwise empty; keep the header band draggable.
  return (
    <>
      <div data-tauri-drag-region={isMacDesktop() || undefined} className="h-[52px]" />
      <SettingsDialog
        tab={tab}
        session={session ?? null}
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
