import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from '@tanstack/react-router'
import { Megaphone } from 'lucide-react'
import { useMemo } from 'react'
import { toast } from 'sonner'
import { openBroadcastDialog } from '@/components/chat/BroadcastDialog'
import { errorText } from '@/components/settings/shared'
import { THEUTH } from '@/components/tasks/helpers'
import { keys, useAgents } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { SidebarGroupLabel, SidebarMenuButton, SidebarMenuItem } from '@/components/ui/sidebar'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { queryKeys, useApi } from '@/lib/rpc'
import { subtrees } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { useSessionField, useSessions } from '@/store/sessions'
import { AgentTree } from './AgentTree'
import { SessionSubagents, runningSubagents } from './SessionItem'

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
 * trees, so only the active desk's subagents (any depth, from `list_sessions`) show, as one `AgentTree` below the grid —
 * the whole tree, under the one list rule (live first, most recent next, `TREE_CAP` per parent behind `Show more`). An
 * inactive desk's live subagents show as the glyph in its tile.
 */
export function Desks({ active }: { active: string | undefined }) {
  const api = useApi()
  const agents = useAgents().data ?? []
  const { data: sessions = [] } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const subs = useMemo(() => subtrees(sessions), [sessions])
  const desks = agents
    .filter((a) => a.status !== 'proposed' && a.status !== 'invalid')
    .map((a) => ({ id: a.desk, name: a.file.name, working: a.status === 'working' }))
    .sort((x, y) => Number(y.name === THEUTH) - Number(x.name === THEUTH))
  if (desks.length === 0) return null
  const labels = deskLabels(desks.map((d) => d.name))
  // The desk the open session belongs to (itself or a subagent of it); at most one.
  const open = desks.find((d) => d.id !== null && (d.id === active || (subs.get(d.id)?.some((s) => s.id === active) ?? false)))?.id
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
              active={d.id === open}
            />
          ))}
        </ul>
      </SidebarMenuItem>
      {open && (
        <SidebarMenuItem>
          <AgentTree session={open} active={active} />
        </SidebarMenuItem>
      )}
    </>
  )
}

/**
 * One desk: its prefix centred, the working dot (roster says working, or a turn in flight) top-left, the running-subagent
 * glyph top-right; hover or focus names it and its state. Fingers have no hover, so a coarse pointer shows the name
 * (truncated) instead of the prefix.
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
  const subRunning = useSessions((s) => runningSubagents(subagents, s.sessions))
  const open = useMutation({
    mutationFn: () => api.desk_open(name),
    onSuccess: (session) => {
      void queryClient.invalidateQueries({ queryKey: keys.agents })
      void queryClient.invalidateQueries({ queryKey: queryKeys.sessions })
      void navigate({ to: '/c/$session', params: { session } })
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
      <span className={cn('absolute top-1.5 left-1.5 size-1.5 rounded-full', busy ? 'bg-success' : 'bg-fg-tertiary/40')} data-dot={busy ? 'working' : 'idle'} aria-hidden />
      <span className="absolute top-1 right-1 flex [&>svg]:size-2.5" aria-hidden>
        <SessionSubagents subagents={subagents} />
      </span>
      <span className="pointer-coarse:hidden">{label}</span>
      <span className="hidden max-w-full truncate px-1.5 text-xs pointer-coarse:block">{name}</span>
    </>
  )
  return (
    <li>
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
              <button type="button" disabled={open.isPending} aria-busy={open.isPending} onClick={() => open.mutate()}>
                {body}
              </button>
            ) : (
              <Link to="/c/$session" params={{ session: id }}>
                {body}
              </Link>
            )}
          </SidebarMenuButton>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          {name} · {state}
        </TooltipContent>
      </Tooltip>
    </li>
  )
}
