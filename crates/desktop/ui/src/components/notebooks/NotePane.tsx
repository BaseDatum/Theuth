import { AppWindow, ChevronLeft } from 'lucide-react'
import { useEffect, useMemo, useRef } from 'react'
import { NoteKernelPill } from '@/components/cells/NoteKernelPill'
import { prefillDraft } from '@/components/chat/Composer'
import { Live } from '@/components/editor/DocumentPage'
import { useDocument } from '@/components/editor/useDocument'
import { useNotebookSession } from '@/components/notebooks/NotebookContext'
import { TitleInput, useNoteBody, useNoteTitle } from '@/components/notes/NotePage'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { canPopout, openPopout } from '@/lib/popout'

/**
 * 157 D5/D6: one note in the notebook's right-hand pane — the editor only, no chat below it. The header is the way
 * back to the notes list (clears `?note`), the note's title (the page's `TitleInput`: rename inline; a just-created
 * note, `fresh`, gets it selected for typing once the document is live, as `?new` does on the note page) and, when
 * the platform can, **Open in new window** (`openPopout({ kind: 'note' })`, then the pane returns to the list). Live
 * collaboration and saving are the document's own (`useDocument` + BlockNote), exactly as on `/notes/$id`.
 * "Ask theuth…" from a block prefills the notebook chat's composer (the thread's draft, or the new chat's).
 */
export function NotePane({ notebook, id, fresh, onBack }: { notebook: string; id: string; fresh: boolean; onBack(): void }) {
  const target = useMemo(() => ({ doc: id }), [id])
  const live = useDocument(target)
  const [title, rename] = useNoteTitle(id, live.provider)
  // ponytail: a board shows its reader here (no Edit toggle in the pane); the note page has the toggle.
  const body = useNoteBody(live, false)
  const session = useNotebookSession(notebook)
  const titleInput = useRef<HTMLInputElement>(null)
  const ready = live.status.kind === 'live'
  useEffect(() => {
    if (fresh && ready) titleInput.current?.select()
  }, [fresh, ready])
  const ask = (block: string) => {
    prefillDraft(session, `about block ${block}: `, [], true)
    window.dispatchEvent(new Event('theuth:focus-composer'))
  }
  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col" aria-label="Note">
      <header className="flex h-10 shrink-0 items-center gap-1 border-b border-border-subtle px-2">
        <Button variant="ghost" size="sm" className="shrink-0 px-1.5 text-xs" aria-label="Back to notes" onClick={onBack}>
          <ChevronLeft /> Notes
        </Button>
        <TitleInput ref={titleInput} value={title} disabled={!live.provider || body.deleted} onCommit={rename} />
        {!body.deleted && <NoteKernelPill doc={id} />}
        {canPopout() && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Open in new window"
                onClick={() => {
                  void openPopout({ kind: 'note', doc: id })
                  onBack()
                }}
              >
                <AppWindow />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom">Open in new window</TooltipContent>
          </Tooltip>
        )}
      </header>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">{body.fallback ?? <Live {...body.live} onAsk={ask} />}</div>
    </section>
  )
}
