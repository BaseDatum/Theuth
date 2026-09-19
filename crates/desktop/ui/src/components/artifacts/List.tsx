import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'
import { memo, useEffect, useMemo, useState } from 'react'
import { GLYPH, metaLine } from '@/components/chat/ArtifactCard'
import { Img } from '@/components/chat/Attachments'
import { ago } from '@/components/sidebar/AgentTree'
import { UNGROUPED } from '@/components/sidebar/SessionList'
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

/** The rail's text filter: a case-insensitive substring of the name or the title. */
export function nameMatch(a: Artifact, q: string): boolean {
  const needle = q.trim().toLowerCase()
  return !needle || a.name.toLowerCase().includes(needle) || a.title.toLowerCase().includes(needle)
}

/** One `name` in one session: the newest version and the older ones, newest first. */
type Stack = { head: Artifact; older: Artifact[] }
export type Group = { session: string; stacks: Stack[] }

/** Newest first in, grouped by session (ordered by their newest artifact) then collapsed by `name`. */
function group(all: Artifact[], f: Filter): Group[] {
  const q = f.q.trim()
  const groups = new Map<string, Map<string, Artifact[]>>()
  for (const a of all) {
    if (f.kind && a.kind !== f.kind) continue
    if (f.session && a.session !== f.session) continue
    if (q && !nameMatch(a, q)) continue
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

/** Section header of artifacts whose durable owner is gone from `list_sessions` (51 §8.3). */
export const UNAVAILABLE = 'Unavailable session'
type Section = { title: string; groups: Group[] }

/**
 * Outer grouping by the owner's effective project (51 §3), sections in first-seen order of `groups`, `Ungrouped`
 * then `Unavailable session` last; no sections at all while every owner is a known ungrouped session (or the
 * session list has not loaded, when nothing can be told apart).
 */
export function sections(groups: Group[], sessions: SessionSummary[] | undefined): Section[] | null {
  if (!sessions) return null
  const by = new Map<string, Group[]>()
  for (const g of groups) {
    const owner = sessions.find((s) => s.id === g.session)
    const title = owner ? (owner.project ?? UNGROUPED) : UNAVAILABLE
    const list = by.get(title)
    if (list) list.push(g)
    else by.set(title, [g])
  }
  if (by.size === 1 && by.has(UNGROUPED)) return null
  const out: Section[] = []
  for (const [title, list] of by) if (title !== UNGROUPED && title !== UNAVAILABLE) out.push({ title, groups: list })
  for (const title of [UNGROUPED, UNAVAILABLE]) {
    const list = by.get(title)
    if (list) out.push({ title, groups: list })
  }
  return out
}

/** The rail body: sessions, each its artifacts newest first. ponytail: not virtualized; virtualize past ~2000 rows. */
export function List({ filter, active }: { filter: Filter; active: string | undefined }) {
  const api = useApi()
  const list = useArtifactList()
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const groups = useMemo(() => group(list.data ?? [], filter), [list.data, filter])
  const split = useMemo(() => sections(groups, sessions.data), [groups, sessions.data])

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
  const render = (gs: Group[]) =>
    gs.map((g) => (
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
    ))
  return (
    <nav className="min-h-0 flex-1 overflow-y-auto px-1.5 py-1" aria-label="Artifacts">
      {split
        ? split.map((sec) => (
            <Collapsible key={sec.title} defaultOpen className="group/project mb-2">
              <CollapsibleTrigger className="flex h-8 w-full items-center gap-1 rounded-[10px] px-2.5 text-xs font-semibold text-fg-secondary hover:bg-layer-2">
                <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[state=open]/project:rotate-90" />
                <span className="truncate">{sec.title}</span>
              </CollapsibleTrigger>
              <CollapsibleContent className="pl-2">{render(sec.groups)}</CollapsibleContent>
            </Collapsible>
          ))
        : render(groups)}
    </nav>
  )
}

/** Older versions stay collapsed until one is active; opening is controlled so a hit on an old version expands an already-mounted stack (49 §7). */
const StackRows = memo(function StackRows({ stack: s, active }: { stack: Stack; active: string | undefined }) {
  const activeOlder = s.older.some((o) => o.call_id === active)
  const [open, setOpen] = useState(activeOlder)
  useEffect(() => {
    if (activeOlder) setOpen(true)
  }, [activeOlder])
  if (s.older.length === 0) return <Row a={s.head} active={s.head.call_id === active} />
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="group/stack">
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
      ref={active ? reveal : undefined}
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

/** Callback ref for the active row: scroll it into the rail when it (re)mounts or becomes active. */
function reveal(el: HTMLAnchorElement | null) {
  el?.scrollIntoView({ block: 'nearest' })
}

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
