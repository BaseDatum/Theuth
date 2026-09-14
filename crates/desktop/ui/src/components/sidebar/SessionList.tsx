import { useQuery } from '@tanstack/react-query'
import { useVirtualizer } from '@tanstack/react-virtual'
import { type RefObject, useMemo } from 'react'
import { queryKeys, useApi } from '@/lib/rpc'
import type { SessionSummary } from '@/rpc/bindings/SessionSummary'
import { AgentTree } from './AgentTree'
import { SessionItem } from './SessionItem'

type Row = { key: string; label: string } | { key: string; session: SessionSummary } | { key: string; tree: string }

const DAY = 86_400_000

/** Today / Yesterday / Previous 7 days / Older, by local calendar day of `updated_at`. */
function group(iso: string, now: number): string {
  const start = new Date(now).setHours(0, 0, 0, 0)
  const t = Date.parse(iso)
  if (t >= start) return 'Today'
  if (t >= start - DAY) return 'Yesterday'
  if (t >= start - 7 * DAY) return 'Previous 7 days'
  return 'Older'
}

/** The top-level ancestor of `id` (itself when top-level or unknown). */
function rootOf(sessions: SessionSummary[], id: string | undefined): string | undefined {
  let cur = id
  for (;;) {
    const parent = sessions.find((s) => s.id === cur)?.parent
    if (!parent) return cur
    cur = parent
  }
}

/** Top-level sessions newest first with group headers; the active one's root is followed by its agent tree. */
function buildRows(sessions: SessionSummary[], active: string | undefined): Row[] {
  const now = Date.now()
  const root = rootOf(sessions, active)
  const rows: Row[] = []
  let label = ''
  for (const s of sessions.filter((s) => s.parent === null).sort((a, b) => b.updated_at.localeCompare(a.updated_at))) {
    const g = group(s.updated_at, now)
    if (g !== label) rows.push({ key: `h:${g}`, label: (label = g) })
    rows.push({ key: s.id, session: s })
    if (s.id === root) rows.push({ key: `t:${s.id}`, tree: s.id })
  }
  return rows
}

/** The chat list (`list_sessions`), virtualized on the sidebar's scroll container. */
export function SessionList({ scrollRef, active }: { scrollRef: RefObject<HTMLDivElement | null>; active: string | undefined }) {
  const api = useApi()
  const { data: sessions = [] } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const rows = useMemo(() => buildRows(sessions, active), [sessions, active])
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => ('label' in (rows[i] ?? {}) ? 36 : 40),
    overscan: 8,
    getItemKey: (i) => rows[i]?.key ?? i,
  })
  return (
    <ul className="relative block px-1.5" style={{ height: v.getTotalSize() }}>
      {v.getVirtualItems().map((item) => {
        const row = rows[item.index]
        if (!row) return null
        return (
          <li
            key={item.key}
            data-index={item.index}
            ref={v.measureElement}
            className="absolute top-0 left-0 w-full"
            style={{ transform: `translateY(${item.start}px)` }}
          >
            {'label' in row ? (
              <div className="px-2.5 pt-4 pb-1.5 text-xs font-medium text-fg-tertiary">{row.label}</div>
            ) : 'session' in row ? (
              <SessionItem id={row.session.id} title={row.session.title} active={row.session.id === active} />
            ) : (
              <AgentTree session={row.tree} active={active} />
            )}
          </li>
        )
      })}
    </ul>
  )
}
