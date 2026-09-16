import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Globe, PanelLeft, ShieldOff } from 'lucide-react'
import { useState } from 'react'
import { HarnessSwitcher, useHarnessCount } from '@/components/link/HarnessSwitcher'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import { touch } from '@/lib/platform'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useApprovals } from '@/store/approvals'
import { useConnection } from '@/store/connection'
import { useSessionField, useSessions } from '@/store/sessions'
import { ApprovalsSheet } from './ApprovalCard'
import { BrowserLive } from './BrowserLive'
import { ContextMeter } from './ContextMeter'
import { ModelMenu } from './ModelMenu'
import { SessionMenu } from './SessionMenu'

/** `↳ parent` breadcrumb of a subagent's transcript. */
function Parent({ id }: { id: string }) {
  const api = useApi()
  const { data: sessions } = useQuery({
    queryKey: queryKeys.sessions,
    queryFn: () => api.list_sessions(),
  })
  const row = sessions?.find((s) => s.id === id)
  return (
    <Link
      to="/c/$session"
      params={{ session: id }}
      className="truncate text-sm text-fg-secondary hover:text-foreground"
    >
      ↳ {row?.name ?? row?.title ?? id.slice(0, 8)}
    </Link>
  )
}

/** 52 px bar over the column: model on the left, context / approvals / session menu on the right. Draggable window region. */
export function Header({
  session,
  draftEffort,
  onDraftEffort,
}: {
  session: string | null
  draftEffort?: string | null
  onDraftEffort?(effort: string | null): void
}) {
  const { open, isMobile } = useSidebar()
  const info = useConnection((s) => s.info)
  const pending = useApprovals((s) => s.pending.length)
  const parent = useSessionField(session ?? '', 'parent')
  const browser = useSessionField(session ?? '', 'browser')
  const live = useSessionField(session ?? '', 'browserLive')
  const [sheet, setSheet] = useState(false)
  const approvalsOff = info !== null && (!info.approvals.global || !info.approvals.session)
  const mac = isMacDesktop()
  // Phones: the harness name in the title area opens the switcher (31 §7.5); desktop keeps the sidebar one.
  const count = useHarnessCount()
  const switcher = touch() && count > 1
  return (
    <header
      data-tauri-drag-region={mac || undefined}
      className={cn('flex h-[52px] shrink-0 items-center gap-2 px-3', !open && mac && 'pl-20')}
    >
      {(!open || isMobile) && <SidebarToggle />}
      {switcher && <HarnessSwitcher className="max-w-[40vw] shrink-0" />}
      <ModelMenu session={session} draftEffort={draftEffort} onDraftEffort={onDraftEffort} />
      {parent && <Parent id={parent} />}
      <div className="flex-1" data-tauri-drag-region={mac || undefined} />
      {session && <ContextMeter session={session} />}
      {session && browser && (
        <Button
          variant="ghost"
          className="size-9 rounded-lg text-fg-secondary"
          aria-label="Live browser"
          onClick={() => useSessions.getState().setBrowserLive(session, true)}
        >
          <Globe className="size-5" />
        </Button>
      )}
      {approvalsOff && (
        <>
          <Badge variant="outline" className="hidden text-fg-secondary md:inline-flex">
            approvals off
          </Badge>
          <ShieldOff className="size-4 shrink-0 text-fg-secondary md:hidden" aria-label="Approvals off" />
        </>
      )}
      {pending > 0 && (
        <Button
          variant="ghost"
          className="h-7 rounded-full px-2 text-warning"
          onClick={() => setSheet(true)}
          aria-label="Pending approvals"
        >
          ●{pending}
        </Button>
      )}
      {session && <SessionMenu session={session} />}
      <ApprovalsSheet open={sheet} onOpenChange={setSheet} />
      {session && (
        <BrowserLive
          session={session}
          open={!!live}
          onOpenChange={(o) => useSessions.getState().setBrowserLive(session, o)}
        />
      )}
    </header>
  )
}

function SidebarToggle() {
  const { toggleSidebar } = useSidebar()
  return (
    <Button
      variant="ghost"
      className="size-9 rounded-lg text-fg-secondary"
      aria-label="Open sidebar"
      onClick={toggleSidebar}
    >
      <PanelLeft className="size-5" />
    </Button>
  )
}
