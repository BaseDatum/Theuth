import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Globe, PanelLeft, ShieldOff } from 'lucide-react'
import { useState } from 'react'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { useSidebar } from '@/components/ui/sidebar'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useApprovals } from '@/store/approvals'
import { useConnection } from '@/store/connection'
import { useSessionField, useSessions } from '@/store/sessions'
import { ApprovalsSheet } from './ApprovalCard'
import { metaLine } from './ArtifactCard'
import { ArtifactPanel } from './ArtifactPanel'
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

/** `▣ N`: the session's artifacts, newest first; picking one opens the panel (35 §7.2). */
function Artifacts({ session }: { session: string }) {
  const artifacts = useSessionField(session, 'artifacts')
  if (!artifacts?.size) return null
  const list = [...artifacts.values()].sort((x, y) => y.created_at - x.created_at)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className="h-9 gap-1 rounded-lg px-2 text-fg-secondary" aria-label="Artifacts">
          ▣<span className="text-xs">{list.length}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72">
        {list.map((a) => (
          <DropdownMenuItem key={a.call_id} className="flex-col items-start gap-0" onSelect={() => useSessions.getState().setArtifactPanel(session, a.call_id)}>
            <span className="w-full truncate">{a.title}</span>
            <span className="w-full truncate text-xs text-fg-tertiary">
              {metaLine(a)}
              {a.version > 1 ? ` · v${a.version}` : ''}
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
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
  return (
    <header
      data-tauri-drag-region={mac || undefined}
      className={cn('flex h-[52px] shrink-0 items-center gap-2 px-3', !open && mac && 'pl-20')}
    >
      {(!open || isMobile) && (
        <div className="flex shrink-0 items-center">
          <SearchTrigger />
          <SidebarToggle />
        </div>
      )}
      <div className="min-w-0">
        <ModelMenu session={session} draftEffort={draftEffort} onDraftEffort={onDraftEffort} />
      </div>
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
      {session && <Artifacts session={session} />}
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
      {session && <ArtifactPanel session={session} />}
    </header>
  )
}

export function SidebarToggle() {
  const { toggleSidebar } = useSidebar()
  return (
    <Button
      variant="ghost"
      size="icon"
      className="rounded-lg text-fg-secondary"
      aria-label="Open sidebar"
      onClick={toggleSidebar}
    >
      <PanelLeft className="size-5" />
    </Button>
  )
}
