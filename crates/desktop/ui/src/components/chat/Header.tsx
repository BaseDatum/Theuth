import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Globe, PanelLeft } from 'lucide-react'
import { useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
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
  const { data: sessions } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const row = sessions?.find((s) => s.id === id)
  return (
    <Link to="/c/$session" params={{ session: id }} className="truncate text-sm text-fg-secondary hover:text-foreground">
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
  const { open } = useSidebar()
  const info = useConnection((s) => s.info)
  const pending = useApprovals((s) => s.pending.length)
  const parent = useSessionField(session ?? '', 'parent')
  const browser = useSessionField(session ?? '', 'browser')
  const live = useSessionField(session ?? '', 'browserLive')
  const [sheet, setSheet] = useState(false)
  const approvalsOff = info !== null && (!info.approvals.global || !info.approvals.session)
  return (
    <header
      data-tauri-drag-region
      className={cn('flex h-[52px] shrink-0 items-center gap-2 px-3', !open && 'pl-20')}
    >
      {!open && <SidebarToggle />}
      <ModelMenu session={session} draftEffort={draftEffort} onDraftEffort={onDraftEffort} />
      {parent && <Parent id={parent} />}
      <div className="flex-1" data-tauri-drag-region />
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
        <Badge variant="outline" className="text-fg-secondary">
          approvals off
        </Badge>
      )}
      {pending > 0 && (
        <Button variant="ghost" className="h-7 rounded-full px-2 text-warning" onClick={() => setSheet(true)} aria-label="Pending approvals">
          ●{pending}
        </Button>
      )}
      {session && <SessionMenu session={session} />}
      <ApprovalsSheet open={sheet} onOpenChange={setSheet} />
      {session && <BrowserLive session={session} open={!!live} onOpenChange={(o) => useSessions.getState().setBrowserLive(session, o)} />}
    </header>
  )
}

function SidebarToggle() {
  const { toggleSidebar } = useSidebar()
  return (
    <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label="Open sidebar" onClick={toggleSidebar}>
      <PanelLeft className="size-5" />
    </Button>
  )
}
