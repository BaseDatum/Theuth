import { Capability, useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { ExternalLink, SquarePen, X } from 'lucide-react'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Composer } from '@/components/chat/Composer'
import { ModelMenu } from '@/components/chat/ModelMenu'
import { ToolsSheet } from '@/components/chat/ToolsSheet'
import { modelRequirements, toolsRequirements } from '@/components/chat/requirements'
import { Transcript } from '@/components/chat/Transcript'
import { Button } from '@/components/ui/button'
import { ResizeHandle } from '@/components/ui/resize-handle'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useIsMobile } from '@/hooks/use-mobile'
import { EMPTY, panelHeight, useNoteChat } from '@/lib/note-chat'
import type { AttachRef } from '@/lib/attach'
import { isSoloPopout, openPopout } from '@/lib/popout'
import { PX_STEP } from '@/lib/resize-drag'
import { queryKeys, useApi } from '@/lib/rpc'
import { useSessionField, useSessions } from '@/store/sessions'

const PAGE = 60

/**
 * 59 §7.3: the chat under a note. Two shapes, chosen by the remembered session (143 D10): with none yet, an input
 * bar — a header line and a composer whose first message creates a session titled like the note, which the panel then
 * keeps (`lib/note-chat`); with one, a resizable bottom panel with that session's transcript (`compact`) over the same
 * composer. Every message leads with the note's `[attached note …]` marker (66 §3.3). "New chat" forgets the session
 * (back to the bar); the session itself stays in the normal chat list. `about` names the document in the copy (an
 * agent's charter is one too). #8: both headers carry the main chat's model picker (`ModelMenu`): the session's model
 * and effort, or — before the first message — the draft's pick, which that message's `create_session` applies. The
 * note page has no chat header, so the panel mounts the chat's Tools… sheet the composer's MCP button opens.
 */
export function NoteChat({ note, doc, about = 'this note' }: { note: string; doc: AttachRef; about?: string }) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const state = useNoteChat((s) => s.notes[note] ?? EMPTY)
  const set = useNoteChat((s) => s.set)
  const navigate = useNavigate()
  const api = useApi()
  const session = state.session
  useLoadSession(session)
  // The store slice learns the title from `TitleChanged`; after a reload the session list has it.
  const live = useSessionField(session ?? '', 'title')
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canQuery1 && (!!session && !live) })
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

  // Drag the top edge: the panel grows upward, clamped to the pane (the hit zone extends up over the editor, never down over the header buttons).
  const mobile = useIsMobile()
  const section = useRef<HTMLElement>(null)
  const start = useRef({ y: 0, height: 0 })
  const measure = () => section.current?.parentElement?.clientHeight ?? 800
  // The pane for the rendered height: read once the panel is in the DOM (the drag reads it live).
  const [pane, setPane] = useState(800)
  useLayoutEffect(() => {
    if (session) setPane(measure())
  }, [session])
  const height = panelHeight(state.height, pane, mobile)
  const clampHeight = (h: number) => panelHeight(h, measure(), mobile)
  // 143 D11: a popped note has no shell to navigate inside — its chat opens in its own window.
  const popped = isSoloPopout()

  const close = (
    <Button variant="ghost" size="icon-sm" aria-label="Close chat" onClick={() => set(note, { open: false })}>
      <X />
    </Button>
  )
  const picker = (
    <Capability required={modelRequirements} hide>
      <div className="min-w-0 shrink">
        <ModelMenu session={session} small />
      </div>
    </Capability>
  )
  const sheet = (
    <Capability required={toolsRequirements} hide>
      <ToolsSheet session={session ?? ''} />
    </Capability>
  )
  const composer = (
    <Composer
      key={`${session ?? 'new'}:${seed}`}
      session={session}
      pinned={doc}
      sessionTitle={doc.title}
      onCreated={(id) => set(note, { session: id })}
      compact
    />
  )

  if (!session)
    return (
      <section ref={section} className="flex shrink-0 flex-col border-t border-border-subtle bg-background" aria-label={`Chat about ${about}`}>
        <header className="flex h-8 shrink-0 items-center gap-1 px-2 text-xs text-fg-secondary">
          <span className="min-w-0 flex-1 truncate">
            Ask theuth about {about} {'\u2014'} the first message starts a chat with it attached
          </span>
          {picker}
          {close}
        </header>
        {composer}
        {sheet}
      </section>
    )

  return (
    <section
      ref={section}
      className="flex shrink-0 flex-col border-t border-border-subtle bg-background"
      style={{ height }}
      aria-label={`Chat about ${about}`}
    >
      <ResizeHandle
        orientation="horizontal"
        placement="start"
        aria-label="Resize chat panel"
        className="h-1.5 bg-transparent hover:bg-layer-2"
        onStart={(e) => (start.current = { y: e.clientY, height })}
        onMove={(_, y) => set(note, { height: clampHeight(start.current.height + start.current.y - y) })}
        onNudge={(d) => set(note, { height: clampHeight(height - d * PX_STEP) })}
      />
      <header className="flex h-8 shrink-0 items-center gap-1 px-2 text-xs text-fg-secondary">
        <span className="min-w-0 flex-1 truncate">{title || 'Chat'}</span>
        {picker}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Open chat"
              onClick={() => (popped ? void openPopout({ kind: 'chat', session }) : void navigate({ to: '/c/$session', params: { session } }))}
            >
              <ExternalLink />
            </Button>
          </TooltipTrigger>
          <TooltipContent>{popped ? 'Open in its own window' : 'Open in the chat list'}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="ghost" size="icon-sm" aria-label="New chat" onClick={() => set(note, { session: null })}>
              <SquarePen />
            </Button>
          </TooltipTrigger>
          <TooltipContent>New chat</TooltipContent>
        </Tooltip>
        {close}
      </header>
      <Transcript key={session} session={session} compact />
      {composer}
      {sheet}
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
