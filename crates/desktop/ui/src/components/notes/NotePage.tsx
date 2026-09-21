import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { getRouteApi, useNavigate } from '@tanstack/react-router'
import { History as HistoryIcon, MessageSquare, MessageSquarePlus, Pencil, Trash2 } from 'lucide-react'
import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { prefillDraft } from '@/components/chat/Composer'
import { Action, DocumentPage } from '@/components/editor/DocumentPage'
import { useDocument } from '@/components/editor/useDocument'
import { UndoAgentAction } from '@/components/knowledge/Document'
import { ChatPicker } from '@/components/notes/ChatPicker'
import { NoteChat } from '@/components/notes/NoteChat'
import { ago } from '@/components/sidebar/AgentTree'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import type { HarnessProvider } from '@/lib/doc/provider'
import { EMPTY, useNoteChat } from '@/lib/note-chat'
import { queryKeys, useApi } from '@/lib/rpc'

const route = getRouteApi('/notes_/$id')

/** `/notes/$id` (59 §7.3): the note in BlockNote, its title inline in the bar, the chat panel below. */
export function NotePage() {
  const { id } = route.useParams()
  return <Note key={id} id={id} />
}

/** The note's title as the provider knows it, updated locally on rename so the bar never flickers back. */
function useTitle(provider: HarnessProvider | null): [string, (t: string) => void] {
  const [title, setTitle] = useState(provider?.meta?.title ?? '')
  useEffect(() => {
    if (!provider) return
    setTitle(provider.meta?.title ?? '')
    // `meta` lands with `doc_open`, after the first `live`.
    return provider.onStatus(() => setTitle(provider.meta?.title ?? ''))
  }, [provider])
  return [
    title,
    (t) => {
      if (provider?.meta) provider.meta.title = t
      setTitle(t)
    },
  ]
}

function Note({ id }: { id: string }) {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const target = useMemo(() => ({ doc: id }), [id])
  const live = useDocument(target)
  const [title, setTitle] = useTitle(live.provider)
  const chat = useNoteChat((s) => s.notes[id] ?? EMPTY)
  const setChat = useNoteChat((s) => s.set)
  const [attaching, setAttaching] = useState(false)
  const [snapshots, setSnapshots] = useState(false)
  const titleInput = useRef<HTMLInputElement>(null)
  const doc = { ref: id, title: title || 'Untitled' }

  const rename = useMutation({
    mutationFn: (t: string) => api.doc_rename(id, t),
    onSuccess: (_, t) => setTitle(t),
    onError: (e) => toast.error(e.message),
  })
  const del = useMutation({
    mutationFn: () => api.doc_delete(id),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: queryKeys.notes })
      toast(`Deleted ${title || 'the note'}`)
      void navigate({ to: '/notes' })
    },
    onError: (e) => toast.error(e.message),
  })
  // 59 §9.3: the slash item "Ask theuth…" opens the panel with the block named in its composer.
  const ask = (block?: string) => {
    if (block) {
      // The draft is read on the composer's mount (panel closed) or on the re-seed the event asks for (panel open).
      prefillDraft(chat.session, `about block ${block}: `, [], true)
      if (chat.open) window.dispatchEvent(new CustomEvent('theuth:note-ask', { detail: { note: id } }))
    }
    setChat(id, { open: true })
  }
  // A deleted note's page: the provider reports `closed · deleted` in the status line; say it plainly too.
  const deleted = live.status.kind === 'closed' && live.status.reason === 'deleted'

  return (
    <DocumentPage
      backTo="/notes"
      backLabel="Back to notes"
      title={
        <TitleInput
          ref={titleInput}
          value={title}
          disabled={!live.provider || deleted}
          onCommit={(t) => t !== title && rename.mutate(t)}
        />
      }
      actions={
        <>
          <Action label="Rename" icon={Pencil} onClick={() => titleInput.current?.select()} disabled={deleted} />
          <Action label="Attach to chat…" icon={MessageSquarePlus} onClick={() => setAttaching(true)} disabled={deleted} />
          <Action label="Snapshots…" icon={HistoryIcon} onClick={() => setSnapshots(true)} disabled={deleted} />
          <UndoAgentAction provider={live.provider} />
          <Action label="Delete" icon={Trash2} onClick={() => del.mutate()} disabled={del.isPending || deleted} />
          <Action label="Chat" icon={MessageSquare} onClick={() => setChat(id, { open: !chat.open })} pressed={chat.open} />
        </>
      }
      live={deleted ? { ...live, error: new Error('This note was deleted.') } : live}
      onAsk={ask}
      below={chat.open && !deleted ? <NoteChat note={id} doc={doc} /> : null}
    >
      <ChatPicker open={attaching} onOpenChange={setAttaching} doc={doc} />
      <Snapshots id={id} open={snapshots} onOpenChange={setSnapshots} />
    </DocumentPage>
  )
}

/** Inline-editable title: Enter or blur commits, Escape reverts (59 §7.3). */
function TitleInput({
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
