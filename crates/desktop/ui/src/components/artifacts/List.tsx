import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'
import { memo, useMemo } from 'react'
import { GLYPH, metaLine } from '@/components/chat/ArtifactCard'
import { Img } from '@/components/chat/Attachments'
import { ago } from '@/components/sidebar/AgentTree'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Skeleton } from '@/components/ui/skeleton'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import type { Artifact, Kind } from '@/rpc/extra'

/** Every session's artifacts, newest first, once; `ArtifactShown` and the refresh button invalidate it. */
export function useArtifactList() {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.artifacts, queryFn: () => api.list_artifacts(null, null) })
}

/** The session's title, or the id's tail when the list does not know it (a deleted or foreign session). */
export function sessionLabel(id: string, sessions: SessionSummary[] | undefined): string {
  return sessions?.find((s) => s.id === id)?.title || `…${id.slice(-8)}`
}

export type Filter = { q: string; kind: Kind | null; session: string | null }

/** One `name` in one session: the newest version and the older ones, newest first. */
type Stack = { head: Artifact; older: Artifact[] }
type Group = { session: string; stacks: Stack[] }

/** Newest first in, grouped by session (ordered by their newest artifact) then collapsed by `name`. */
function group(all: Artifact[], f: Filter): Group[] {
  const q = f.q.trim().toLowerCase()
  const groups = new Map<string, Map<string, Artifact[]>>()
  for (const a of all) {
    if (f.kind && a.kind !== f.kind) continue
    if (f.session && a.session !== f.session) continue
    if (q && !a.name.toLowerCase().includes(q) && !a.title.toLowerCase().includes(q)) continue
    let byName = groups.get(a.session)
    if (!byName) groups.set(a.session, (byName = new Map()))
    const versions = byName.get(a.name)
    if (versions) versions.push(a)
    else byName.set(a.name, [a])
  }
  const out: Group[] = []
  for (const [session, byName] of groups) {
    const stacks: Stack[] = []
    for (const versions of byName.values()) {
      versions.sort((x, y) => y.version - x.version)
      const head = versions[0]
      if (head) stacks.push({ head, older: versions.slice(1) })
    }
    out.push({ session, stacks })
  }
  return out
}

/** The rail body: sessions, each its artifacts newest first. ponytail: not virtualized; virtualize past ~2000 rows. */
export function List({ filter, active }: { filter: Filter; active: string | undefined }) {
  const api = useApi()
  const list = useArtifactList()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const groups = useMemo(() => group(list.data ?? [], filter), [list.data, filter])

  if (list.isPending) {
    return (
      <div className="space-y-2 p-3">
        <Skeleton className="h-[52px] w-full" />
        <Skeleton className="h-[52px] w-full" />
        <Skeleton className="h-[52px] w-2/3" />
      </div>
    )
  }
  if (list.isError) return <p className="p-3 text-sm text-danger">{list.error.message}</p>
  if (list.data.length === 0) {
    return <p className="p-3 text-sm text-fg-tertiary">Nothing shown yet — ask for a report, a table or a PDF and it lands here.</p>
  }
  if (groups.length === 0) return <p className="p-3 text-sm text-fg-tertiary">No matches</p>
  return (
    <nav className="min-h-0 flex-1 overflow-y-auto px-1.5 py-1" aria-label="Artifacts">
      {groups.map((g) => (
        <section key={g.session} className="mb-2">
          <Link
            to="/c/$session"
            params={{ session: g.session }}
            className="flex h-8 items-center px-2.5 text-xs font-medium text-fg-secondary hover:text-foreground"
            title="Open chat"
          >
            <span className="truncate">{sessionLabel(g.session, sessions.data)}</span>
          </Link>
          {g.stacks.map((s) => (
            <StackRows key={s.head.name} stack={s} active={active} />
          ))}
        </section>
      ))}
    </nav>
  )
}

const StackRows = memo(function StackRows({ stack: s, active }: { stack: Stack; active: string | undefined }) {
  if (s.older.length === 0) return <Row a={s.head} active={s.head.call_id === active} />
  return (
    <Collapsible defaultOpen={s.older.some((o) => o.call_id === active)} className="group/stack">
      <Row a={s.head} active={s.head.call_id === active} />
      <CollapsibleTrigger className="flex h-7 w-full items-center gap-1 rounded-[10px] pl-[62px] text-xs text-fg-tertiary hover:bg-layer-2">
        <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[state=open]/stack:rotate-90" />
        {s.older.length} older
      </CollapsibleTrigger>
      <CollapsibleContent>
        {s.older.map((o) => (
          <Row key={o.call_id} a={o} active={o.call_id === active} />
        ))}
      </CollapsibleContent>
    </Collapsible>
  )
})

const Row = memo(function Row({ a, active }: { a: Artifact; active: boolean }) {
  return (
    <Link
      to="/artifacts/$callId"
      params={{ callId: a.call_id }}
      className={cn('flex items-center gap-2.5 rounded-[10px] px-2.5 py-1.5 text-sm hover:bg-layer-2', active && 'bg-layer-3 hover:bg-layer-3')}
    >
      <Thumb a={a} />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="flex-1 truncate">{a.title}</span>
          {a.version > 1 && <span className="shrink-0 rounded-md bg-layer-2 px-1.5 text-[11px] text-fg-secondary">v{a.version}</span>}
        </div>
        <p className="truncate text-xs text-fg-tertiary">
          {metaLine(a)} · {ago(new Date(a.created_at * 1000).toISOString())}
        </p>
      </div>
    </Link>
  )
})

/** The 40×52 slot: preview PNG, the image itself, or a kind glyph. */
function Thumb({ a }: { a: Artifact }) {
  const box = 'h-[52px] w-10 shrink-0 overflow-hidden rounded-md bg-layer-2'
  if (a.preview) return <Img path={a.preview} className={cn(box, 'object-cover object-top')} />
  if (a.kind === 'image' || a.kind === 'svg') return <Img path={a.path} className={cn(box, 'object-cover')} />
  const Glyph = GLYPH[a.kind]
  return (
    <div className={cn(box, 'flex items-center justify-center text-fg-tertiary')}>
      <Glyph className="size-5" />
    </div>
  )
}
