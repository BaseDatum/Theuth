import { useQuerySupport } from '@/rpc/capabilities'
// 104 A6/A7: what every attention screen of this wave opens and closes with — where the item came from, what the agent
// last said there, and the fixed action bar at the bottom.
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { type ReactNode, useState } from 'react'
import { title } from '@/components/chat/broadcast'
import { snoozeUntil } from '@/components/hud/model'
import { useAttentionActions } from '@/components/hud/queries'
import { age } from '@/components/tasks/helpers'
import { itemHome } from '@/components/tasks/home'
import { useMesh, useTask, useTasks } from '@/components/tasks/queries'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'
import type { AttentionItem } from "@/rpc/bindings/method.roster.output"
import { clampText, lastAssistantBefore, pathText } from './model'
import { Chip } from './shared'

/** `list_sessions` — the names behind every row's and screen's `from <session>`; one query, shared with the sidebar. */
export function useSessionList() {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const api = useApi()
  return useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
}

/**
 * The header line: kind chip · **From ‹project · Root › child› · ‹agent› · ‹age›** · Open session (→ `/c/$session`) ·
 * the task's title (→ `/tasks/$id`). 207 R5: the engine's lineage path when it sent one, else the session's title.
 */
export function Source({ item }: { item: AttentionItem }) {
  const sessions = useSessionList()
  const task = useTask(item.task ?? '', item.task !== null)
  const on = itemHome(item.task, useTasks().data, useMesh().data)
  const from = pathText(item) || (item.session ? title(item.session, sessions.data, item.session.slice(0, 8)) : '')
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-fg-tertiary">
        <Chip kind={item.kind} id={item.id} />
        <span className="min-w-0 truncate">
          {(from || item.agent) && 'From '}
          {from && <span className="text-fg-secondary">{from}</span>}
          {from && item.agent && ' · '}
          {item.agent && <span className="text-fg-secondary">{item.agent}</span>}
          {(from || item.agent) && ' · '}
          <span className="tabular-nums">{age(item.at_ms)}</span>
          {on && ` · ${on}`}
        </span>
        {item.session && (
          <Button asChild variant="link" size="xs" className="h-auto px-0 text-xs">
            <Link to="/c/$session" params={{ session: item.session }}>
              Open session
            </Link>
          </Button>
        )}
        {item.task && (
          <Link to="/tasks/$id" params={{ id: item.task }} className="ml-auto min-w-0 truncate text-fg-secondary hover:underline">
            {task.data?.title ?? 'Open task'}
          </Link>
        )}
      </div>
    </div>
  )
}

/**
 * 104 A7: the agent's last assistant text in `session` before `beforeMs` (≤ 600 chars), from the chat's own
 * `load_session` page. `null` while loading, when the RPC fails, or when the session is unknown — the caller shows nothing.
 */
export function useContextLine(session: string | null, beforeMs: number): string | null {
  const canQuery2 = useQuerySupport(["load_session"])
  const api = useApi()
  const q = useQuery({
    queryKey: ['attention-context', session, beforeMs],
    queryFn: async () => {
      const page = await api.load_session(session as string, null, 60)
      const text = lastAssistantBefore(page.history, beforeMs)
      return text === null ? null : clampText(text)
    },
    enabled: canQuery2 && (session !== null),
    retry: false,
    staleTime: 5 * 60_000,
  })
  return q.data ?? null
}

/** The context line under the header: three clamped lines with "more"; renders nothing when there is none. */
export function ContextLine({ session, beforeMs }: { session: string | null; beforeMs: number }) {
  const text = useContextLine(session, beforeMs)
  const [open, setOpen] = useState(false)
  if (text === null) return null
  return (
    <div className="flex flex-col gap-1 border-l-2 border-border-subtle pl-3 text-sm text-fg-secondary">
      <p className={open ? 'whitespace-pre-wrap' : 'line-clamp-3 whitespace-pre-wrap'}>{text}</p>
      {text.length > 160 && (
        <button type="button" className="w-fit text-xs text-fg-tertiary hover:underline" onClick={() => setOpen((o) => !o)}>
          {open ? 'less' : 'more'}
        </button>
      )}
    </div>
  )
}

/** The bar fixed under the scroll area (the review screen's pattern): phones get Send / Allow where the thumb is. */
export function ActionBar({ children }: { children: ReactNode }) {
  return (
    <div className="shrink-0 border-t border-border-subtle bg-background px-4 pt-3 pb-[calc(0.75rem+var(--safe-bottom))] md:px-6">
      <div className="mx-auto flex w-full max-w-[720px] flex-wrap items-center gap-1.5">{children}</div>
    </div>
  )
}

/** Snooze until 09:00 tomorrow; `onResolved` moves the pane on (101 D11). */
export function SnoozeButton({ item, onResolved, size = 'sm' }: { item: AttentionItem; onResolved: () => void; size?: 'sm' | 'default' }) {
  const act = useAttentionActions()
  return (
    <Button variant="ghost" size={size} disabled={act.snooze.isPending} title="Snooze until 09:00 tomorrow" onClick={() => void act.snooze.mutateAsync({ id: item.id, until: snoozeUntil('tomorrow') }).then(onResolved, () => undefined)}>
      Snooze
    </Button>
  )
}
