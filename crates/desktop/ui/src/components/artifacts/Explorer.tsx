import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Outlet, getRouteApi, useParams } from '@tanstack/react-router'
import { PanelLeft, RefreshCw, Search, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { type Filter, List, nameMatch, sessionLabel, useArtifactList } from '@/components/artifacts/List'
import { GLYPH } from '@/components/chat/ArtifactCard'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSidebar } from '@/components/ui/sidebar'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { isSearchDestination, useSearchOrigin } from '@/lib/search'
import { cn } from '@/lib/utils'
import type { Kind } from '@/rpc/extra'
import { MeetingPill } from '@/components/meeting/MeetingPill'

const route = getRouteApi('/artifacts')

/** `/artifacts`: 300 px rail (search, kind chips, the list) and the viewer outlet. Under `md` one pane at a time: the list at `/artifacts`, the viewer at `/artifacts/$callId`. */
export function Explorer() {
  const api = useApi()
  const qc = useQueryClient()
  const initial = route.useSearch()
  const { callId: active } = useParams({ strict: false })
  const sidebar = useSidebar()
  const list = useArtifactList()
  const [q, setQ] = useState(initial.q ?? '')
  const [kind, setKind] = useState<Kind | null>(initial.kind ?? null)
  const [session, setSession] = useState<string | null>(initial.session ?? null)
  const filter = useMemo<Filter>(() => ({ q, kind, session }), [q, kind, session])
  const kinds = useMemo(() => {
    const seen: Partial<Record<Kind, true>> = {}
    for (const a of list.data ?? []) seen[a.kind] = true
    return (Object.keys(GLYPH) as Kind[]).filter((k) => seen[k])
  }, [list.data])
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })

  // A search hit opened `active` (49 §7): the record is exact, so any local filter hiding it is cleared,
  // once per activation (`origin` is set only then), never while the user is typing a new one.
  const origin = useSearchOrigin()
  const fromSearch = active !== undefined && isSearchDestination(origin, { category: 'artifacts', callId: active })
  const target = useQuery({
    queryKey: queryKeys.artifact(active ?? ''),
    queryFn: () => api.get_artifact(active ?? ''),
    enabled: fromSearch,
  })
  const hit = fromSearch ? target.data : undefined
  useEffect(() => {
    if (!hit) return
    setKind((k) => (k && hit.kind !== k ? null : k))
    setSession((s) => (s && hit.session !== s ? null : s))
    setQ((v) => (v && !nameMatch(hit, v) ? '' : v))
  }, [origin, hit?.call_id])

  const mac = isMacDesktop()
  return (
    <div className="flex min-h-0 flex-1 overflow-hidden">
      <aside
        className={cn(
          'w-full shrink-0 flex-col border-r border-border-subtle md:flex md:w-[300px]',
          active ? 'hidden' : 'flex',
        )}
      >
        <header
          data-tauri-drag-region={mac || undefined}
          className={cn('flex h-[52px] shrink-0 items-center gap-1 px-3', !sidebar.open && mac && 'pl-20')}
        >
          {(!sidebar.open || sidebar.isMobile) && (
            <div className="flex shrink-0 items-center">
              <SearchTrigger />
              <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
                <PanelLeft />
              </Button>
            </div>
          )}
          <h2 className="flex-1 truncate text-sm font-semibold">Artifacts</h2>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Refresh"
                disabled={list.isFetching}
                onClick={() => void qc.invalidateQueries({ queryKey: queryKeys.artifacts })}
              >
                <RefreshCw className={list.isFetching ? 'animate-spin' : undefined} />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Refresh</TooltipContent>
          </Tooltip>
          {!active && <MeetingPill />}
        </header>
        <div className="relative shrink-0 px-3 pb-2">
          <Search className="pointer-events-none absolute top-1/2 left-5 size-4 -translate-y-1/2 text-fg-tertiary" />
          <Input
            value={q}
            placeholder="Filter by name or title"
            className="h-9 rounded-[10px] bg-layer-2 pl-8"
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') setQ('')
            }}
          />
        </div>
        {(kinds.length > 1 || session) && (
          <div className="flex shrink-0 flex-wrap gap-1 px-3 pb-2">
            {kinds.map((k) => {
              const Glyph = GLYPH[k]
              const on = kind === k
              return (
                <button
                  key={k}
                  type="button"
                  aria-pressed={on}
                  onClick={() => setKind(on ? null : k)}
                  className={cn(
                    'flex h-7 items-center gap-1 rounded-full border px-2 text-xs transition-colors',
                    on ? 'border-fg-accent bg-accent' : 'border-border-subtle text-fg-secondary hover:bg-accent/60',
                  )}
                >
                  <Glyph className="size-3.5" />
                  {k}
                </button>
              )
            })}
            {session && (
              <button
                type="button"
                onClick={() => setSession(null)}
                className="flex h-7 max-w-full items-center gap-1 rounded-full border border-fg-accent bg-accent px-2 text-xs"
                aria-label="Show every session"
              >
                <span className="truncate">{sessionLabel(session, sessions.data)}</span>
                <X className="size-3.5 shrink-0" />
              </button>
            )}
          </div>
        )}
        <List filter={filter} active={active} />
      </aside>
      <main className={cn('min-w-0 flex-1 md:flex', active ? 'flex' : 'hidden')}>
        {active ? <Outlet /> : <p className="m-auto text-sm text-fg-tertiary">Select an artifact</p>}
      </main>
    </div>
  )
}
