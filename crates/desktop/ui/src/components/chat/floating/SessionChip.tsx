// Shared session picker pill for floating chat docks (Whiteboard and Task Graph).

import { useQuery } from '@tanstack/react-query'
import { Check, ExternalLink, Link2, Plus, Unlink } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useApi } from '@/lib/rpc'
import { useDark } from '@/lib/theme'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import { useQuerySupport } from '@/rpc/capabilities'
import { useSessions } from '@/store/sessions'
import { useCloseOnCanvas } from './useCloseOnCanvas'

export type FloatingSessionItem = {
  session: string
  title: string | null | undefined
}

/** A session's name for the picker: its title, cut to fit. */
export function shortName(title: string | null | undefined, max = 22): string {
  const t = (title ?? '').trim() || 'Untitled session'
  return t.length > max ? `${t.slice(0, max - 1)}…` : t
}

/** Live title of `session` from the session store, falling back to a known list item. */
export function useSessionTitle(session: string | null, items: readonly FloatingSessionItem[]): string | null {
  const live = useSessions((s) => (session ? s.sessions[session]?.title : undefined))
  return session ? live || items.find((b) => b.session === session)?.title || null : null
}

/** Filter unarchived, root, non-observed sessions excluding `excludeIds`, newest first. */
export function filterOtherSessions(
  sessions: readonly SessionSummary[],
  excludeIds: ReadonlySet<string>,
  limit = 8,
): FloatingSessionItem[] {
  return sessions
    .filter((s) => !s.archived && !s.parent && !excludeIds.has(s.id) && s.kind !== 'observed')
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
    .slice(0, limit)
    .map((s) => ({ session: s.id, title: s.title || s.name }))
}

export type SessionChipProps = {
  session: string | null
  title?: string | null
  bound: readonly FloatingSessionItem[]
  /** Section heading for `bound` sessions (default: `'On this whiteboard'`). */
  boundLabel?: string
  /** Optional pre-filtered recent other sessions; when omitted, fetched via `list_sessions` on open. */
  recent?: readonly FloatingSessionItem[]
  busy: boolean
  onPick(session: string): void
  onNew(): void
  onDetach?: (session: string) => void
  /** Open the session in chat (leaves the current view). */
  onOpen?: (session: string) => void
  /** Canvas selector used by `useCloseOnCanvas` (default: `'.wb-canvas'`). */
  canvasSelector?: string
}

/** Ink green matching `whiteboard/ink/colour.ts` `INK_COLOURS.green` (≥ 3:1 contrast on light & dark). */
const ACTIVE_DOT = { light: '#2a8e3d', dark: '#5fd27a' } as const

/** The session picker: New session, bound/project sessions, recent others, open in chat, optional detach. */
export function SessionChip({
  session,
  title: explicitTitle,
  bound,
  boundLabel = 'On this whiteboard',
  recent,
  busy,
  onPick,
  onNew,
  onDetach,
  onOpen,
  canvasSelector = '.wb-canvas',
}: SessionChipProps) {
  const dark = useDark()
  const [open, setOpen] = useState(false)
  useCloseOnCanvas(open, () => setOpen(false), canvasSelector)
  const api = useApi()
  const can = useQuerySupport(['list_sessions'])
  const recentQuery = useQuery({
    enabled: recent === undefined && can && open,
    queryKey: ['whiteboard', 'recent-sessions'],
    queryFn: () => api.list_sessions(),
    staleTime: 15_000,
  })
  const boundIds = new Set(bound.map((b) => b.session))
  const others = recent !== undefined ? recent.filter((s) => !boundIds.has(s.session)) : filterOtherSessions(recentQuery.data ?? [], boundIds)
  const fallbackTitle = useSessionTitle(session, [...bound, ...others])
  const title = explicitTitle !== undefined ? explicitTitle : fallbackTitle
  const row = (key: string, label: ReactNode, on: boolean, onClick: () => void, icon?: ReactNode) => (
    <li key={key}>
      <button type="button" onClick={() => (onClick(), setOpen(false))} className="wb-press flex min-h-11 w-full items-center gap-2 rounded-xl px-3 text-left text-sm">
        <span className="grid size-4 place-items-center">{on ? <Check className="size-4" /> : icon}</span>
        <span className="min-w-0 flex-1 truncate">{label}</span>
      </button>
    </li>
  )
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={session ? `Session: ${title ?? 'untitled'}. Change session` : 'New session. Choose a session'}
          className="wb-glass flex h-8 min-w-0 max-w-[14rem] shrink items-center gap-1.5 rounded-full px-2.5 text-[12px] font-semibold"
        >
          <span className="wb-dot" data-busy={busy} style={session ? { background: dark ? ACTIVE_DOT.dark : ACTIVE_DOT.light } : undefined} aria-hidden />
          <span className="min-w-0 truncate">{session ? shortName(title, 28) : 'New session'}</span>
          {session && <Link2 className="size-3.5 shrink-0 opacity-60" aria-hidden />}
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" collisionPadding={12} className="wb-glass w-72 rounded-2xl border-0 p-1.5">
        <ul className="flex flex-col" aria-label="Sessions">
          {row('new', 'New session', session === null, onNew, <Plus className="size-4" />)}
          {bound.length > 0 && <li className="wb-muted px-3 pt-1 pb-0.5 text-[11px] font-semibold uppercase tracking-wide">{boundLabel}</li>}
          {bound.map((b) => row(b.session, shortName(b.title, 34), b.session === session, () => onPick(b.session)))}
          {others.length > 0 && <li className="wb-muted px-3 pt-2 pb-0.5 text-[11px] font-semibold uppercase tracking-wide">Other sessions</li>}
          {others.map((s) => row(s.session, shortName(s.title, 34), s.session === session, () => onPick(s.session)))}
          {session && (onOpen || onDetach) && (
            <>
              <li className="my-1 h-px bg-current opacity-10" aria-hidden />
              {onOpen && row('open', 'Open in chat', false, () => onOpen(session), <ExternalLink className="size-4" />)}
              {onDetach && row('detach', 'Detach this session', false, () => onDetach(session), <Unlink className="size-4" />)}
            </>
          )}
        </ul>
      </PopoverContent>
    </Popover>
  )
}
