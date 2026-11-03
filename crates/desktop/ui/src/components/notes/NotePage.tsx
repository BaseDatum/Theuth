import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { getRouteApi, useNavigate } from '@tanstack/react-router'
import { AppWindow, BookOpen, History as HistoryIcon, MessageSquare, MessageSquarePlus, Pencil, PenLine, Trash2 } from 'lucide-react'
import { type KeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { BoardReader } from '@/components/board/BoardReader'
import { NoteKernelPill } from '@/components/cells/NoteKernelPill'
import { prefillDraft } from '@/components/chat/Composer'
import { Action, DocumentPage, ownedByMeeting, useNotebookBack } from '@/components/editor/DocumentPage'
import { type LiveDocument, useDocument } from '@/components/editor/useDocument'
import { UndoAgentAction } from '@/components/knowledge/Document'
import { ChatPicker } from '@/components/notes/ChatPicker'
import { ExportMenu } from '@/components/notes/ExportMenu'
import { NoteChat } from '@/components/notes/NoteChat'
import { titleAfter } from '@/components/notes/title'
import { useNotes } from '@/components/notes/useNotes'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { DictationButton } from '@/components/ui/dictation'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { type AttachRef, subtitle } from '@/lib/attach'
import type { EngineProvider } from '@/lib/doc/provider'
import { EMPTY, useNoteChat } from '@/lib/note-chat'
import { canPopout, closePopout, isPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { on } from '@/store/sessions'
import { cn } from '@/lib/utils'

const route = getRouteApi('/notes_/$id')

/** `/notes/$id` (59 §7.3): the note in BlockNote, its title inline in the bar, the chat panel below. `?nb` (144 §4): the notebook to go back to. */
export function NotePage() {
  const { id } = route.useParams()
  const { new: isNew, nb } = route.useSearch()
  return <NoteView key={id} id={id} isNew={isNew === true} nb={nb} />
}

/**
 * The note's title as the provider knows it (59 §7.3): `meta` lands with `doc_open`; a rename — this page's, the
 * list row's, another device's — arrives as `DocumentChanged` on the bus, not on the document stream, so the page
 * follows that too. The local rename result sets it first so the bar never flickers back.
 */
function useTitle(provider: EngineProvider | null): [string, (t: string) => void] {
  const [title, setTitle] = useState(provider?.meta?.title ?? '')
  useEffect(() => {
    if (!provider) return
    setTitle(provider.meta?.title ?? '')
    const offStatus = provider.onStatus(() => setTitle(provider.meta?.title ?? ''))
    const offChanged = on('DocumentChanged', (e) => {
      const t = titleAfter(provider, e)
      if (t !== null) setTitle(t)
    })
    return () => {
      offStatus()
      offChanged()
    }
  }, [provider])
  return [
    title,
    (t) => {
      if (provider?.meta) provider.meta.title = t
      setTitle(t)
    },
  ]
}

/**
 * 157 D5: the title as the provider knows it, with its rename (`doc_rename`; the local result sets it first so the
 * bar never flickers back). The note page's bar and the notebook pane's header share it.
 */
export function useNoteTitle(id: string, provider: EngineProvider | null): [string, (t: string) => void] {
  const api = useApi()
  const [title, setTitle] = useTitle(provider)
  const rename = useMutation({
    mutationFn: (t: string) => api.doc_rename(id, t),
    onSuccess: (_, t) => setTitle(t),
    onError: (e) => toast.error(e.message),
  })
  return [title, (t) => t !== title && rename.mutate(t)]
}

/**
 * 157 D5: what fills a note's body — the one rule for the note page and the notebook pane. `deleted`: the provider
 * reports `closed · deleted` (the live document gets a plain error). 71 §2.2: a board opens in its own read mode and
 * `editing` toggles to BlockNote (a board is still a note); until `meta` lands with `doc_open` the kind is unknown,
 * so `fallback` is an empty box rather than an editor that flips. Render `fallback ?? <Live {...live} />`.
 */
export function useNoteBody(live: LiveDocument, editing: boolean): { deleted: boolean; board: boolean; live: LiveDocument; fallback: ReactNode | undefined } {
  const deleted = live.status.kind === 'closed' && live.status.reason === 'deleted'
  const kind = live.provider?.meta?.kind
  const board = kind === 'board' && !deleted
  const fallback =
    board && !editing && live.provider ? <BoardReader provider={live.provider} /> : live.provider && !kind && live.status.kind === 'connecting' ? <div /> : undefined
  return { deleted, board, live: deleted ? { ...live, error: new Error('This note was deleted.') } : live, fallback }
}

/**
 * One note, whole: title, action bar, editor, chat. The route's page, and (156 D1) the meeting page's right pane
 * (`embedded`: no band, no back link, no window title) and the popped meeting note. With `meeting` the note is that
 * meeting's (`manifest.note`): Delete is hidden — the meeting's own Delete owns it — "Open in new window" opens the
 * meeting-note pop-out, and (D2) its chat is the meeting's own session: embedded, "Ask theuth…" prefills the
 * `LiveSession` composer beside the note and focuses it; popped, the chat panel is pinned to that session.
 */
export function NoteView({ id, isNew = false, nb, embedded = false, meeting }: { id: string; isNew?: boolean; nb?: string; embedded?: boolean; meeting?: string }) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const target = useMemo(() => ({ doc: id }), [id])
  const live = useDocument(target)
  const [title, rename] = useNoteTitle(id, live.provider)
  const chat = useNoteChat((s) => s.notes[id] ?? EMPTY)
  const setChat = useNoteChat((s) => s.set)
  const [attaching, setAttaching] = useState(false)
  const [snapshots, setSnapshots] = useState(false)
  const titleInput = useRef<HTMLInputElement>(null)
  // 66 §3.1: the note as a reference — the live title, the list row's facts for `meta` when the list has it.
  const summary = useNotes().data?.find((n) => n.id === id)
  const doc: AttachRef = { kind: 'note', id, title: title || 'Untitled', meta: summary ? subtitle({ kind: 'note', doc: summary }) : '' }
  // 156 D2: the meeting's session, from the cache the meeting page already holds (no fetch of its own while embedded).
  const manifest = useQuery({ queryKey: queryKeys.meeting(meeting ?? ''), queryFn: () => api.meeting_get(meeting ?? ''), enabled: meeting !== undefined })
  const meetingSession = meeting !== undefined ? (manifest.data?.session ?? null) : null
  useEffect(() => {
    if (!embedded) document.title = title
  }, [title, embedded])
  // Popped meeting note: the panel talks in the meeting's session, never a note-chat session of its own.
  useEffect(() => {
    if (meetingSession && !embedded && chat.session !== meetingSession) setChat(id, { session: meetingSession })
  }, [meetingSession, embedded, chat.session, id, setChat])
  // A just-created note (`?new`, from New note): the title is selected for typing over once the document is live
  // (`meta`, and so the title, lands with `doc_open` just before); the flag is dropped from the URL, `?nb` stays.
  const ready = live.status.kind === 'live'
  useEffect(() => {
    if (!isNew || !ready) return
    titleInput.current?.select()
    void navigate({ to: '/notes/$id', params: { id }, search: nb ? { nb } : {}, replace: true })
  }, [isNew, ready, id, nb, navigate])
  // 144 §4: back to the notebook this note was opened from (or belongs to) at every width.
  const wayBack = useNotebookBack('note', id, nb)

  const [editing, setEditing] = useState(false)
  const body = useNoteBody(live, editing)
  const { deleted, board } = body
  const del = useMutation({
    mutationFn: () => api.doc_delete(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.notes })
      toast(`Deleted ${title || 'the note'}`)
      if (isPopout()) closePopout()
      else void navigate({ to: '/notes' })
    },
    onError: (e) => toast.error(e.message),
  })
  // 59 §9.3: the slash item "Ask theuth…" opens the panel with the block named in its composer.
  const ask = (block?: string) => {
    if (embedded) {
      // 156 D2: the meeting chat is beside the note — its composer renders the draft store, so the text lands at once.
      if (meetingSession && block) prefillDraft(meetingSession, `about block ${block}: `)
      window.dispatchEvent(new Event('theuth:focus-composer'))
      return
    }
    if (block) {
      // The draft is read on the composer's mount (panel closed) or on the re-seed the event asks for (panel open).
      prefillDraft(chat.session, `about block ${block}: `, [], true)
      if (chat.open) window.dispatchEvent(new CustomEvent('theuth:note-ask', { detail: { note: id } }))
    }
    setChat(id, { open: true })
  }
  // Popped (`lib/popout.ts`): no second "new window"; the document is the one the main window edits, so both editors
  // follow each other through the document actor like two devices. The chat panel is there too (143 D8).
  const popped = isPopout()
  const kind = live.provider?.meta?.kind

  return (
    <DocumentPage
      backTo="/notes"
      backLabel="Back to notes"
      back={wayBack.back}
      embedded={embedded}
      title={
        // Embedded in a narrow pane the title keeps at least 180 px and the actions wrap under it (`Frame`).
        <span className={cn('flex min-w-0 flex-1 items-center gap-1', embedded && 'min-w-[180px]')}>
          <TitleInput
            ref={titleInput}
            value={title}
            disabled={!live.provider || deleted}
            onCommit={rename}
          />
          {!deleted && <DictationButton field={titleInput} />}
          {wayBack.chip}
          {!deleted && <NoteKernelPill doc={id} />}
        </span>
      }
      actions={
        <>
          {board && <span className="mr-1 rounded border px-1.5 py-0.5 font-mono text-[10px] tracking-[0.08em] text-fg-tertiary uppercase">board</span>}
          {board && <Action label={editing ? 'Read' : 'Edit'} icon={editing ? BookOpen : PenLine} onClick={() => setEditing((e) => !e)} pressed={editing} />}
          <Action label="Rename" icon={Pencil} onClick={() => titleInput.current?.select()} disabled={deleted} />
          <Action label="Attach to chat…" icon={MessageSquarePlus} onClick={() => setAttaching(true)} disabled={deleted} />
          <Action label="Snapshots…" icon={HistoryIcon} onClick={() => setSnapshots(true)} disabled={deleted} />
          <ExportMenu id={id} title={title} disabled={deleted || !live.provider} />
          <UndoAgentAction provider={live.provider} />
          {!popped && canPopout() && (
            <Action
              label="Open in new window"
              icon={AppWindow}
              onClick={() => void openPopout(meeting !== undefined ? { kind: 'meeting-note', meeting } : { kind: 'note', doc: id })}
              disabled={deleted}
            />
          )}
          {!ownedByMeeting(kind, meeting) && <Action label="Delete" icon={Trash2} onClick={() => del.mutate()} disabled={del.isPending || deleted} />}
          {!embedded && <Action label="Chat" icon={MessageSquare} onClick={() => setChat(id, { open: !chat.open })} pressed={chat.open} />}
        </>
      }
      live={body.live}
      fallback={body.fallback}
      onAsk={ask}
      below={chat.open && !deleted && !embedded ? <NoteChat note={id} doc={doc} /> : null}
    >
      <ChatPicker open={attaching} onOpenChange={setAttaching} item={doc} />
      <Snapshots id={id} open={snapshots} onOpenChange={setSnapshots} />
    </DocumentPage>
  )
}

/** Inline-editable title: Enter or blur commits, Escape reverts (59 §7.3). Exported for the notebook pane's header (157 D6). */
export function TitleInput({
  ref,
  value,
  disabled,
  onCommit,
}: {
  ref: React.Ref<HTMLInputElement>
  value: string
  disabled: boolean
  onCommit: (title: string) => void
}) {
  const [draft, setDraft] = useState<string | null>(null)
  const commit = () => {
    const t = draft?.trim()
    setDraft(null)
    if (t) onCommit(t)
  }
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') e.currentTarget.blur()
    else if (e.key === 'Escape') {
      setDraft(null)
      e.currentTarget.blur()
    }
  }
  return (
    <input
      ref={ref}
      aria-label="Title"
      className="min-w-0 flex-1 truncate bg-transparent text-sm font-medium outline-none placeholder:text-fg-tertiary focus:underline focus:decoration-border-subtle"
      value={draft ?? value}
      placeholder="Untitled"
      disabled={disabled}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={onKeyDown}
    />
  )
}

/** `doc_snapshots` with a Restore per row (59 §4.6: a restore imports the snapshot's text as one diff, history kept). Notes and markdown artifacts share it. */
export function Snapshots({ id, open, onOpenChange }: { id: string; open: boolean; onOpenChange: (o: boolean) => void }) {
  const api = useApi()
  const list = useQuery({ queryKey: ['doc', 'snapshots', id], queryFn: () => api.doc_snapshots(id), enabled: open })
  const restore = useMutation({
    mutationFn: (name: string) => api.doc_restore(id, name),
    onSuccess: () => {
      toast('Restored')
      onOpenChange(false)
    },
    onError: (e) => toast.error(e.message),
  })
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Snapshots</DialogTitle>
          <DialogDescription>Restoring imports the snapshot's text over the current one; nothing is rewound.</DialogDescription>
        </DialogHeader>
        {list.data?.length === 0 && <p className="text-sm text-fg-tertiary">No snapshots yet.</p>}
        <ul className="max-h-80 overflow-y-auto text-sm">
          {list.data?.map((s) => (
            <li key={s.name} className="flex items-center gap-2 py-1">
              <span className="flex-1 truncate">{s.label}</span>
              <span className="text-xs text-fg-tertiary">{ago(new Date(Number(s.at_ms)).toISOString())}</span>
              <Button variant="outline" size="sm" disabled={restore.isPending} onClick={() => restore.mutate(s.name)}>
                Restore
              </Button>
            </li>
          ))}
        </ul>
      </DialogContent>
    </Dialog>
  )
}
