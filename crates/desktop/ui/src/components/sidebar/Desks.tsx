import { useQuerySupport } from '@/rpc/capabilities'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Megaphone } from 'lucide-react'
import { useMemo } from 'react'
import { toast } from 'sonner'
import { openBroadcastDialog } from '@/components/chat/BroadcastDialog'
import { useSessionDrag } from '@/components/chat/Panes'
import { errorText } from '@/components/settings/shared'
import { THEUTH } from '@/components/tasks/helpers'
import { keys, useAgents } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { SidebarGroupLabel, SidebarMenuButton, SidebarMenuItem } from '@/components/ui/sidebar'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { queryKeys, useApi } from '@/lib/rpc'
import { canPopout, openPopout } from '@/lib/popout'
import { live, subtrees } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import { useSessionField, useSessions } from '@/store/sessions'
import { AgentTree } from './AgentTree'
import { runningSubagents } from './SessionItem'
import { SidebarPopoutButton } from './SidebarPopoutButton'

const NONE: SessionSummary[] = []

/** Tile text per name: the first letter, or, for names sharing it, the shortest prefix (≤ 3) no other name shares; first letter upper-cased. */
export function deskLabels(names: readonly string[]): string[] {
  const lower = names.map((n) => n.toLowerCase())
  return names.map((name, i) => {
    let k = 1
    while (k < 3 && lower.some((o, j) => j !== i && o.slice(0, k) === lower[i]!.slice(0, k))) k++
    const p = name.slice(0, k)
    return p.charAt(0).toUpperCase() + p.slice(1).toLowerCase()
  })
}

/**
 * Sky 2026-11-11: one tile per active roster agent (not proposed, not invalid), theuth first, in a grid that wraps with the
 * sidebar's width, whether or not its desk session exists yet: a tile with a desk (`AgentSummary.desk`, roster-owned)
 * opens it; one without asks the roster to open it first. Pinning has nothing to do with this group. Tiles cannot carry
 * trees, so only the active desk's subagents (any depth, from `list_sessions`) show, as one `DeskTree` below the grid —
 * the whole tree, under the one list rule (live first, most recent next, `TREE_CAP` per parent behind `Show more`). An
 * inactive desk's live subagents show as the count in its tile.
 */
export function Desks({ active }: { active: string | undefined }) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const agents = useAgents().data ?? []
  const { data: sessions = [] } = useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const subs = useMemo(() => subtrees(sessions), [sessions])
  const desks = agents
    .filter((a) => a.status !== 'proposed' && a.status !== 'invalid')
    .map((a) => ({ id: a.desk, name: a.file.name, working: a.status === 'working' }))
    .sort((x, y) => Number(y.name === THEUTH) - Number(x.name === THEUTH))
  if (desks.length === 0) return null
  const labels = deskLabels(desks.map((d) => d.name))
  // The desk the open session belongs to (itself or a subagent of it); at most one.
  const open = desks.find((d) => d.id !== null && (d.id === active || (subs.get(d.id)?.some((s) => s.id === active) ?? false)))
  return (
    <>
      <SidebarGroupLabel className="mt-2">
        Desks
        <Button
          variant="ghost"
          size="icon-sm"
          className="ml-auto"
          aria-label="Broadcast to desks"
          title="Broadcast to desks"
          onClick={() => openBroadcastDialog({ preset: 'desks' })}
        >
          <Megaphone />
        </Button>
      </SidebarGroupLabel>
      <SidebarMenuItem>
        {/* Fine pointer 44 px min per column, a finger 64 px (room for the short name; see `DeskTile`). */}
        <ul className="grid grid-cols-[repeat(auto-fill,minmax(44px,1fr))] gap-1 px-1 pb-1 pointer-coarse:grid-cols-[repeat(auto-fill,minmax(64px,1fr))]">
          {desks.map((d, i) => (
            <DeskTile
              key={d.name}
              {...d}
              label={labels[i]!}
              subagents={d.id === null ? undefined : subs.get(d.id)}
              active={d === open}
            />
          ))}
        </ul>
      </SidebarMenuItem>
      {open?.id && <DeskTree id={open.id} name={open.name} active={active} />}
    </>
  )
}

/**
 * The open desk's subagents below the grid, under a line naming the desk (the tiles show letters only) with its live and
 * finished counts; clicking the name opens the desk. No line while the desk has no subagents.
 */
function DeskTree({ id, name, active }: { id: string; name: string; active: string | undefined }) {
  const drag = useSessionDrag(id, name)
  const rows = Object.values(useSessionField(id, 'children') ?? {})
  const n = rows.filter((r) => live(r.state.state)).length
  return (
    <SidebarMenuItem>
      {rows.length > 0 && (
        <div className="group/popout-row relative">
          <Link
            to="/c/$session"
            params={{ session: id }}
            {...drag}
            data-desk-header
            className="mx-1 flex h-7 items-center gap-1.5 rounded-md px-2 text-xs hover:bg-sidebar-accent pointer-fine:pr-8"
          >
            <span className="truncate font-semibold">{name}</span>
            <span className="shrink-0 text-fg-tertiary">
              {n} live · {rows.length - n} finished
            </span>
          </Link>
          <SidebarPopoutButton target={{ kind: 'chat', session: id }} className="top-1 right-2" />
        </div>
      )}
      <AgentTree session={id} active={active} />
    </SidebarMenuItem>
  )
}

/**
 * One desk: its prefix centred, the working dot (roster says working, or a turn in flight) top-right, the number of live
 * subagents bottom-right; hover or focus names it and its state. Fingers have no hover, so a coarse pointer shows the
 * name (truncated, clear of the corners) instead of the prefix.
 */
function DeskTile({
  id,
  name,
  label,
  working,
  subagents = NONE,
  active,
}: {
  id: string | null
  name: string
  label: string
  working: boolean
  subagents: readonly SessionSummary[] | undefined
  active: boolean
}) {
  const api = useApi()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const running = useSessionField(id ?? '', 'running')
  // Spread only on the session Link: a desk without a session opens via `desk_open` and has nothing to drag.
  const drag = useSessionDrag(id, name)
  const subRunning = useSessions((s) => runningSubagents(subagents, s.sessions))
  // `popout`: the tile's "Open in new window" — the same `desk_open`, then the desk's chat in its own window.
  const open = useMutation({
    mutationFn: (_popout: boolean) => api.desk_open(name),
    onSuccess: (session, popout) => {
      void queryClient.invalidateQueries({ queryKey: keys.agents })
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions })
      if (popout) void openPopout({ kind: 'chat', session })
      else void navigate({ to: '/c/$session', params: { session } })
    },
    onError: (e) => toast.error(errorText(e)),
  })
  const busy = working || running
  const state = [
    open.isPending ? 'opening' : busy ? 'working' : 'idle',
    subRunning > 0 && `${subRunning} subagent${subRunning === 1 ? '' : 's'} running`,
  ]
    .filter(Boolean)
    .join(', ')
  const body = (
    <>
      <span className={cn('absolute top-1.5 right-1.5 size-1.5 rounded-full', busy ? 'bg-success' : 'bg-fg-tertiary/40')} data-dot={busy ? 'working' : 'idle'} aria-hidden />
      {subRunning > 0 && (
        <span className="absolute right-1.5 bottom-1 text-[10px] leading-none font-bold text-info" data-live-count aria-hidden>
          {subRunning}
        </span>
      )}
      <span className={cn('pointer-coarse:hidden', canPopout() && 'group-hover/popout-row:opacity-0 group-focus-within/popout-row:opacity-0')}>{label}</span>
      <span className="hidden max-w-full truncate px-3 text-xs pointer-coarse:block">{name}</span>
    </>
  )
  return (
    <li className="group/popout-row relative">
      <Tooltip>
        <TooltipTrigger asChild>
          <SidebarMenuButton
            asChild
            isActive={active}
            aria-label={`${name} desk, ${state}`}
            className={cn(
              'relative h-9 justify-center rounded-[10px] bg-sidebar-accent/50 p-0 text-[13px] font-semibold shadow-[inset_0_0_0_1px_var(--sidebar-border)] pointer-coarse:h-11',
              'data-[active=true]:font-semibold data-[active=true]:shadow-[inset_0_0_0_1.5px_var(--sidebar-ring)]',
              open.isPending && 'animate-pulse',
            )}
          >
            {id === null ? (
              <button type="button" disabled={open.isPending} aria-busy={open.isPending} onClick={() => open.mutate(false)}>
                {body}
              </button>
            ) : (
              <Link to="/c/$session" params={{ session: id }} {...drag}>
                {body}
              </Link>
            )}
          </SidebarMenuButton>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          {name} · {state}
        </TooltipContent>
      </Tooltip>
      <SidebarPopoutButton target={id !== null ? { kind: 'chat', session: id } : () => { if (!open.isPending) open.mutate(true) }} className="inset-0! m-auto size-6 bg-sidebar-accent" />
    </li>
  )
}
