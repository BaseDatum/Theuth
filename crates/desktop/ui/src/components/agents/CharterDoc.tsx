import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { MessageSquare } from 'lucide-react'
import { useMemo } from 'react'
import { prefillDraft } from '@/components/chat/Composer'
import { Live } from '@/components/editor/DocumentPage'
import { useDocument } from '@/components/editor/useDocument'
import { NoteChat } from '@/components/notes/NoteChat'
import { THEUTH } from '@/components/tasks/helpers'
import { Button } from '@/components/ui/button'
import { useNoteChat } from '@/lib/note-chat'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { AgentFile } from "@/rpc/bindings/method.roster.output"
import { charterBinding, charterRef } from './charter'
import { NAME_RE } from './wizard'

type Who = Pick<AgentFile, 'name' | 'title' | 'reports_to' | 'capabilities'>

/**
 * The id of `agent-charter:<name>`: resolved from the roster file's body on first open, or — with `seed`, in the
 * Hire wizard before the file exists — created from the seed. `null` until known; disabled for a bad name and theuth.
 */
export function useCharterDoc(name: string, seed?: string) {
  const canQuery1 = useQuerySupport(["doc_ensure","doc_resolve"])
  const api = useApi()
  return useQuery({
    queryKey: ['charter-doc', name, seed !== undefined],
    queryFn: () => (seed !== undefined ? api.doc_ensure(charterBinding(name), seed) : api.doc_resolve(charterBinding(name))),
    staleTime: Number.POSITIVE_INFINITY,
    enabled: canQuery1 && (NAME_RE.test(name) && name !== THEUTH),
  })
}

/**
 * Sky's ruling: the charter is a document — the BlockNote editor over `agent-charter:<name>` (agents' edits show
 * member-labelled as in notes) with the note chat beneath it, its every message pinned to the document. The panel's
 * height, open state and session are remembered per document (`lib/note-chat`); it starts open.
 */
export function CharterDoc({ who, seed, hint, className }: { who: Who; seed?: string; hint: string; className?: string }) {
  const doc = useCharterDoc(who.name, seed)
  if (doc.error) return <p className="m-auto px-6 text-center text-sm text-danger">{doc.error.message}</p>
  if (!doc.data) return <Live provider={null} status={{ kind: 'connecting' }} error={null} />
  return <Bound id={doc.data} who={who} hint={hint} className={className} />
}

function Bound({ id, who, hint, className }: { id: string; who: Who; hint: string; className?: string }) {
  const target = useMemo(() => ({ doc: id }), [id])
  const live = useDocument(target)
  const panel = useNoteChat((s) => s.notes[id])
  const setPanel = useNoteChat((s) => s.set)
  const open = panel?.open ?? true
  // 59 §9.3: the slash item "Ask theuth…" opens the panel with the block named in its composer.
  const ask = (block?: string) => {
    if (block) {
      prefillDraft(panel?.session ?? null, `about block ${block}: `, [], true)
      if (open) window.dispatchEvent(new CustomEvent('theuth:note-ask', { detail: { note: id } }))
    }
    setPanel(id, { open: true })
  }
  return (
    <div className={cn('flex min-h-0 min-w-0 flex-1 flex-col', className)}>
      <div className="flex shrink-0 items-center gap-2 border-b border-border-subtle px-4 py-1 text-[13px] text-fg-tertiary">
        <span className="min-w-0 flex-1 truncate">{hint}</span>
        <Button variant="ghost" size="sm" aria-pressed={open} onClick={() => setPanel(id, { open: !open })}>
          <MessageSquare /> Chat
        </Button>
      </div>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
        <Live {...live} dense onAsk={ask} />
      </div>
      {open && <NoteChat note={id} doc={charterRef(id, who)} about="this charter" />}
    </div>
  )
}
