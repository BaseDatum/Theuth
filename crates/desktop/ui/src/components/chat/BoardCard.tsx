// Design 71 §2.11: the chat card for `doc_create(kind="board")` — the title, `drawing · <section>` while the turn
// runs (the section is the last `##` this session inserted into the board), then **Open** / **Open in new window**.
// §0 Q5: the card binds the board to the session that drew it (`note-chat`), so the board's badge and Ask land there;
// a binding the user already changed (New chat) is left alone.
import { Link } from '@tanstack/react-router'
import { AppWindow, LayoutPanelTop, Loader2 } from 'lucide-react'
import { useEffect } from 'react'
import { Button } from '@/components/ui/button'
import { resultLine } from '@/lib/describe'
import { useNoteChat } from '@/lib/note-chat'
import { canPopout, openPopout } from '@/lib/popout'
import { type Entry, useSessionField, useSessions } from '@/store/sessions'

type Tool = Extract<Entry, { kind: 'tool' }>

/** `doc_create` with `kind: "board"` (args land with the call, so a pending call without them is a plain row). */
export function isBoardCreate(tool: Tool): boolean {
  return tool.name === 'doc_create' && typeof tool.args === 'object' && tool.args !== null && (tool.args as Record<string, unknown>).kind === 'board'
}

/** The board id from the tool's result line, `created board <id> (theuth://notes/<id>)`. */
export function createdBoard(output: string | undefined): string | null {
  return /^created board (\S+)/.exec(output ?? '')?.[1] ?? null
}

/** The last `##` heading among the session's inserts/replaces into `doc` — the section being drawn (§2.8). */
export function drawingSection(entries: readonly Entry[], doc: string): string | null {
  let section: string | null = null
  for (const e of entries) {
    if (e.kind !== 'tool' || (e.name !== 'doc_insert' && e.name !== 'doc_replace') || typeof e.args !== 'object' || e.args === null) continue
    const a = e.args as { doc?: unknown; markdown?: unknown }
    if (a.doc !== doc || typeof a.markdown !== 'string') continue
    for (const m of a.markdown.matchAll(/^## (.+)$/gm)) section = m[1]!.trim()
  }
  return section
}

export function BoardCard({ tool, session, readOnly }: { tool: Tool; session: string; readOnly?: boolean }) {
  const title = String((tool.args as Record<string, unknown>).title ?? 'Board')
  const id = createdBoard(tool.result?.output)
  const failed = tool.result?.isError
  const running = useSessionField(session, 'running')
  const section = useSessions((s) => (id ? drawingSection(s.sessions[session]?.entries ?? [], id) : null))
  useEffect(() => {
    if (!id || readOnly) return
    const chat = useNoteChat.getState()
    if (!chat.notes[id]) chat.set(id, { session })
  }, [id, session, readOnly])
  const drawing = !!id && !!running
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm" data-board-card={id ?? undefined} data-drawing={drawing || undefined}>
      <div className="flex items-center gap-2 font-medium">
        <LayoutPanelTop className="size-3.5 shrink-0 text-fg-secondary" />
        <span className="wrap-anywhere">{title}</span>
        {(tool.pending || !tool.result) && <Loader2 className="size-3.5 shrink-0 animate-spin text-fg-tertiary" />}
        <span className="ml-auto shrink-0 font-mono text-xs text-fg-tertiary">{drawing ? `drawing${section ? ` · ${section}` : '…'}` : id ? 'board' : null}</span>
      </div>
      {failed && <div className="mt-1 truncate font-mono text-xs text-danger">✗ {resultLine(tool.result?.output ?? '')}</div>}
      {id && (
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <Button asChild variant="secondary" size="xs">
            <Link to="/notes/$id" params={{ id }}>
              Open
            </Link>
          </Button>
          {canPopout() && (
            <Button variant="ghost" size="xs" onClick={() => void openPopout({ kind: 'note', doc: id })}>
              <AppWindow /> Open in new window
            </Button>
          )}
        </div>
      )}
    </div>
  )
}
