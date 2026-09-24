import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ExternalLink, SquarePen, X } from 'lucide-react'
import { type PointerEvent, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Composer } from '@/components/chat/Composer'
import { Transcript } from '@/components/chat/Transcript'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { EMPTY, MIN_HEIGHT, useNoteChat } from '@/lib/note-chat'
import type { AttachRef } from '@/lib/attach'
import { queryKeys, useApi } from '@/lib/rpc'
import { useSessionField, useSessions } from '@/store/sessions'

const PAGE = 60

/**
 * 59 §7.3: the chat under a note. A resizable bottom panel with the remembered session's transcript (`compact`)
 * and a composer whose every message leads with the note's `[attached note …]` marker (66 §3.3); the first message
 * creates a session titled like the note and the panel keeps it (`lib/note-chat`). "New chat" forgets the session; the
 * session itself stays in the normal chat list.
 */
export function NoteChat({ note, doc }: { note: string; doc: AttachRef }) {
  const state = useNoteChat((s) => s.notes[note] ?? EMPTY)
  const set = useNoteChat((s) => s.set)
  const navigate = useNavigate()
  const api = useApi()
  const session = state.session
  useLoadSession(session)
  // The store slice learns the title from `TitleChanged`; after a reload the session list has it.
  const live = useSessionField(session ?? '', 'title')
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: !!session && !live })
  const title = live || sessions.data?.find((s) => s.id === session)?.title
  // 59 §9.3: the page seeded the panel draft for "Ask theuth…" while this panel was open; remount the composer on it.
  const [seed, setSeed] = useState(0)
  useEffect(() => {
    const onAsk = (ev: Event) => {
      if ((ev as CustomEvent<{ note: string }>).detail.note === note) setSeed((n) => n + 1)
    }
    window.addEventListener('theuth:note-ask', onAsk)
    return () => window.removeEventListener('theuth:note-ask', onAsk)
  }, [note])

  // Drag the top edge: the panel grows upward, clamped to the pane.
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    const startY = e.clientY
    const startH = state.height
    const el = e.currentTarget
    const max = (el.parentElement?.parentElement?.clientHeight ?? 800) - 120
    el.setPointerCapture(e.pointerId)
    const move = (ev: globalThis.PointerEvent) => {
      set(note, { height: Math.min(max, Math.max(MIN_HEIGHT, startH + (startY - ev.clientY))) })
    }
    const up = () => {
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
  }

  return (
    <section
      className="flex shrink-0 flex-col border-t border-border-subtle bg-background"
      style={{ height: state.height }}
      aria-label="Chat about this note"
    >
      <div
        role="separator"
        aria-orientation="horizontal"
        onPointerDown={onPointerDown}
        className="h-1.5 shrink-0 cursor-row-resize hover:bg-layer-2"
      />
      <header className="flex h-8 shrink-0 items-center gap-1 px-2 text-xs text-fg-secondary">
        <span className="min-w-0 flex-1 truncate">{session ? title || 'Chat' : 'New chat about this note'}</span>
        {session && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Open chat"
                onClick={() => void navigate({ to: '/c/$session', params: { session } })}
              >
                <ExternalLink />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Open in the chat list</TooltipContent>
          </Tooltip>
        )}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label="New chat" disabled={!session} onClick={() => set(note, { session: null })}>
              <SquarePen />
            </Button>
          </TooltipTrigger>
          <TooltipContent>New chat</TooltipContent>
        </Tooltip>
        <Button variant="ghost" size="icon-sm" aria-label="Close chat" onClick={() => set(note, { open: false })}>
          <X />
        </Button>
      </header>
      {session ? (
        <Transcript key={session} session={session} compact />
      ) : (
        <p className="m-auto px-6 text-center text-sm text-fg-tertiary">
          Ask about this note; the first message starts a chat with it attached.
        </p>
      )}
      <Composer
        key={`${session ?? 'new'}:${seed}`}
        session={session}
        pinned={doc}
        sessionTitle={doc.title}
        onCreated={(id) => set(note, { session: id })}
        compact
      />
    </section>
  )
}

/** The panel's session page, once: the same first load `c.$session.tsx` does, without its todo/agents extras. */
function useLoadSession(session: string | null) {
  const api = useApi()
  useEffect(() => {
    if (!session) return
    const st = useSessions.getState()
    st.ensure(session)
    if (st.sessions[session]?.loaded) return
    api.load_session(session, null, PAGE).then(
      (page) => useSessions.getState().loadHistory(session, page, 'replace'),
      (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
    )
  }, [api, session])
}
