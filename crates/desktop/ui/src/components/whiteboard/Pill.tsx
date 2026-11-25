// 218 §8.8 "Composer pill" / design §4.1, R12: always visible, bottom-right, glass. Grabber (→ cluster), the session
// chip (dot + short session name + link glyph; "no session" when unbound; tap → picker), the placeholder or the
// agent's one-line status while it works (dot pulses), mic (→ the sheet, focused, for system dictation).

import { useQuery } from '@tanstack/react-query'
import { Check, Link2, Mic, Plus, Unlink } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { useSessions } from '@/store/sessions'
import type { WhiteboardSession } from './wire'

/** A session's name for the chip: its title, cut to fit. */
export function shortName(title: string | null | undefined, max = 22): string {
  const t = (title ?? '').trim() || 'Untitled session'
  return t.length > max ? `${t.slice(0, max - 1)}…` : t
}

export type SessionPick = {
  session: string | null
  title: string | null
  bound: WhiteboardSession[]
  busy: boolean
  onPick(session: string): void
  onNew(): void
  onDetach(session: string): void
}

/** The chip and its picker: bound sessions, recent others, new, detach. */
export function SessionChip({ p, onOpen }: { p: SessionPick; onOpen?(open: boolean): void }) {
  const [open, setOpen] = useState(false)
  const api = useApi()
  const can = useQuerySupport(['list_sessions'])
  const recent = useQuery({ enabled: can && open, queryKey: ['whiteboard', 'recent-sessions'], queryFn: () => api.list_sessions(), staleTime: 15_000 })
  const boundIds = new Set(p.bound.map((b) => b.session))
  const others = (recent.data ?? [])
    .filter((s) => !s.archived && !s.parent && !boundIds.has(s.id) && s.kind !== 'observed')
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
    .slice(0, 8)
  const set = (o: boolean) => {
    setOpen(o)
    onOpen?.(o)
  }
  const row = (key: string, label: ReactNode, on: boolean, onClick: () => void, icon?: ReactNode) => (
    <li key={key}>
      <button type="button" onClick={() => (onClick(), set(false))} className="wb-press flex min-h-11 w-full items-center gap-2 rounded-xl px-3 text-left text-sm">
        <span className="grid size-4 place-items-center">{on ? <Check className="size-4" /> : icon}</span>
        <span className="min-w-0 flex-1 truncate">{label}</span>
      </button>
    </li>
  )
  return (
    <Popover open={open} onOpenChange={set}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={p.session ? `Session: ${p.title ?? 'untitled'}. Change session` : 'No session. Choose a session'}
          className="wb-chip flex h-8 max-w-[11rem] shrink-0 items-center gap-1.5 rounded-full px-2.5 text-[12px] font-semibold"
        >
          <span className="wb-dot" data-busy={p.busy} data-idle={p.session ? undefined : 'none'} aria-hidden />
          <span className="min-w-0 truncate">{p.session ? shortName(p.title) : 'no session'}</span>
          {p.session && <Link2 className="size-3.5 shrink-0 opacity-60" aria-hidden />}
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className="wb-glass w-72 rounded-2xl border-0 p-1.5">
        <ul className="flex flex-col" aria-label="Sessions">
          {p.bound.length > 0 && <li className="wb-muted px-3 pt-1 pb-0.5 text-[11px] font-semibold uppercase tracking-wide">On this whiteboard</li>}
          {p.bound.map((b) => row(b.session, shortName(b.title, 34), b.session === p.session, () => p.onPick(b.session)))}
          {others.length > 0 && <li className="wb-muted px-3 pt-2 pb-0.5 text-[11px] font-semibold uppercase tracking-wide">Other sessions</li>}
          {others.map((s) => row(s.id, shortName(s.title || s.name, 34), false, () => p.onPick(s.id)))}
          <li className="my-1 h-px bg-current opacity-10" aria-hidden />
          {row('new', 'New session with this whiteboard', p.session === null, p.onNew, <Plus className="size-4" />)}
          {p.session && row('detach', 'Detach this session', false, () => p.onDetach(p.session as string), <Unlink className="size-4" />)}
        </ul>
      </PopoverContent>
    </Popover>
  )
}

export function useSessionTitle(session: string | null, bound: WhiteboardSession[]): string | null {
  const live = useSessions((s) => (session ? s.sessions[session]?.title : undefined))
  return session ? live || bound.find((b) => b.session === session)?.title || null : null
}

export function Pill({
  pick,
  status,
  onGrabber,
  onOpen,
  onMic,
}: {
  pick: SessionPick
  /** The one-line status (agent working, or a notice while full-window); null → the placeholder. */
  status: string | null
  onGrabber(): void
  onOpen(): void
  onMic(): void
}) {
  return (
    <div role="group" aria-label="Composer" className="wb-glass wb-corner absolute z-20 flex h-11 w-[min(330px,calc(100vw-48px))] items-center gap-2 rounded-full pr-1.5 pl-1">
      <button type="button" aria-label="Show controls" onClick={onGrabber} className="grid h-11 w-6 shrink-0 place-items-center">
        <span className="h-5 w-[5px] rounded-full bg-current opacity-35" />
      </button>
      <SessionChip p={pick} />
      <button type="button" onClick={onOpen} aria-label={status ? `${status}. Write a message` : 'Write a message'} className="h-full min-w-0 flex-1 truncate text-left text-[14px]">
        <span className={status ? '' : 'wb-muted'} aria-live="polite">
          {status ?? 'Ask…'}
        </span>
      </button>
      <button type="button" aria-label="Dictate" onClick={onMic} className="wb-chip grid size-8 shrink-0 place-items-center rounded-full">
        <Mic className="size-4" />
      </button>
    </div>
  )
}
