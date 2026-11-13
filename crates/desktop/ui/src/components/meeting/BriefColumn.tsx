// 156 D3: the briefs of one meeting as a full-height column — the first child of the meeting page's body row in
// every state, live or ended — oldest first, the island's cursor ringed, a cached instance linking its row on the
// briefs tab. The list is `meeting_briefs` alone (`queryKeys.meetingBriefs`): `lib/rpc.ts` invalidates it on
// `TopicAdded`, `TopicRemoved` and `Brief` (D4), so topic briefs — which publish no `Brief` event — appear as
// they land. Width and collapse live in `lib/meeting-settings.ts`; the page owns the handle and the toggle.

import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { type CSSProperties, useEffect, useRef } from 'react'
import { Empty } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Brief } from "@/rpc/bindings/method.meeting.output"

export const hhmmss = (ms: number): string => {
  const s = Math.floor(ms / 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`
}

/** The meeting's briefs, oldest first (the engine's order); shared by the column and the header's count. */
export function useBriefs(id: string) {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.meetingBriefs(id), queryFn: () => api.meeting_briefs(id) })
}

/**
 * `cursor` is the brief the island shows. `live`: a reader at the bottom stays there as briefs land (one scrolled up
 * is left where they are, like the live log's own rule in reverse).
 */
export function BriefColumn({ briefs, cursor, live, className, style }: { briefs: Brief[]; cursor: string | null; live: boolean; className?: string; style?: CSSProperties }) {
  const scroller = useRef<HTMLDivElement>(null)
  const atBottom = useRef(true)
  useEffect(() => {
    const el = scroller.current
    if (live && el && atBottom.current) el.scrollTop = el.scrollHeight
  }, [briefs, live])
  return (
    <div
      ref={scroller}
      className={cn('flex min-h-0 shrink-0 flex-col gap-2 overflow-y-auto', className)}
      style={style}
      data-testid="brief-column"
      onScroll={(e) => {
        const el = e.currentTarget
        atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 8
      }}
    >
      {briefs.length === 0 && <Empty>No briefs yet.</Empty>}
      {briefs.map((b) => (
        <div key={b.id} className={cn('rounded-xl border border-border-subtle p-3 text-[13px]', b.id === cursor && 'border-fg-accent')}>
          <div className="flex flex-wrap items-baseline justify-between gap-x-2">
            <span className="font-semibold">{b.title}</span>
            <span className="shrink-0 text-[11px] text-fg-tertiary">
              {b.cached !== null ? (
                // 05 §7: a cached instance links its row on the briefs tab (`?id=` outlines it).
                <Link to="/meetings/briefs" search={{ id: b.cached }} className="hover:underline">
                  {b.source === 'Cache' ? 'cached' : `${b.source} · cached`}
                </Link>
              ) : (
                b.source
              )}{' '}
              · {hhmmss(Number(b.at_ms))}
            </span>
          </div>
          <ul className="mt-1 list-disc space-y-0.5 pl-4 text-fg-secondary">
            {b.bullets.map((x) => (
              <li key={x}>{x}</li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  )
}
