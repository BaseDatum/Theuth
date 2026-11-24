// Design 71 §2.9 / M10, M13: select-to-ask. A place on the board — a prose selection, excerpt rows, a pinned unit
// or span, the stale list — becomes a `[board …]` header + quote in the bound session's composer (`lib/board-ask`).
// The floating **Ask** sits in the document beside the selection (R §C.1 anchors theirs in `.review-document`);
// ⌘⇧A asks about the selection, else the pin, else the board. Where the text lands (`askBoard`): the NoteChat panel
// composer, pinned to the board so `[attached note …]` leads (§0 Q5: the session that drew it, or NoteChat's
// first-message-creates-a-session when none is bound); a popped window sends the bound session's chat window instead.
import { MessageSquareQuote } from 'lucide-react'
import { type CSSProperties, type RefObject, useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { prefillDraft } from '@/components/chat/Composer'
import { excerptLines } from '@/components/chat/surface/parse'
import type { AttachRef } from '@/lib/attach'
import { type BoardAt, formatAt } from '@/lib/board-ask'
import { useNoteChat } from '@/lib/note-chat'
import { canPopout, isPopout, openPopout } from '@/lib/popout'
import { type AnchorState, type Registered, formatAnchor, intersect, useAnchorStore } from './anchors'
import { type BoardExcerpt, type DocBlock, classifyCode } from './blocks'
import type { Located } from "@/rpc/bindings/method.codeindex.output"
import { chipText } from './live'

/** A place the reader points at: the block that owns it, where inside it, and the words quoted. */
export type Place = { block: string | null; at: BoardAt | null; quote: string | null }

export function askBoard(doc: string, text: string, boardRef: AttachRef): void {
  const chat = useNoteChat.getState()
  const session = chat.notes[doc]?.session ?? null
  if (!isPopout()) {
    // 59 §9.3's "Ask theuth…" path: the panel composer reads the draft on mount, or on the re-seed the event asks for.
    prefillDraft(session, text, [], true)
    if (chat.notes[doc]?.open) window.dispatchEvent(new CustomEvent('theuth:note-ask', { detail: { note: doc } }))
    chat.set(doc, { open: true })
    return
  }
  if (session && canPopout()) {
    prefillDraft(session, text, [boardRef])
    void openPopout({ kind: 'chat', session })
    return
  }
  prefillDraft(null, text, [boardRef])
  toast.message('Question drafted', { description: 'This board has no chat yet — send it from a new chat in the main window.' })
}

/** The rows `[a, b]` of an excerpt as quoted text. */
function rowsText(e: BoardExcerpt, lines: [number, number]): string {
  const all = excerptLines(e.props.text)
  const start = e.anchor.lines[0]
  return all.slice(lines[0] - start, lines[1] - start + 1).join('\n')
}

/** The excerpt rows or prose the native selection covers inside `column`, or `null` (collapsed, or outside the board). */
export function placeOfSelection(sel: Selection | null, column: HTMLElement | null, excerpts: readonly BoardExcerpt[]): { place: Place; range: Range } | null {
  if (!sel || sel.isCollapsed || sel.rangeCount === 0 || !column) return null
  const range = sel.getRangeAt(0)
  const el = (n: Node | null) => (n instanceof Element ? n : n?.parentElement) ?? null
  const from = el(sel.anchorNode)
  const to = el(sel.focusNode)
  if (!from || !to || !column.contains(from) || !column.contains(to)) return null
  const rowA = from.closest<HTMLElement>('[data-line]')
  const rowB = to.closest<HTMLElement>('[data-line]')
  const frame = rowA?.closest<HTMLElement>('.board-excerpt[data-block]')
  if (rowA && rowB && frame && frame === rowB.closest('.board-excerpt[data-block]')) {
    const block = frame.dataset.block!
    const e = excerpts.find((x) => x.blockId === block)
    if (e) {
      const a = Number(rowA.dataset.line)
      const b = Number(rowB.dataset.line)
      const lines: [number, number] = [Math.min(a, b), Math.max(a, b)]
      return { place: { block, at: { kind: 'code', path: e.anchor.path, lines }, quote: rowsText(e, lines) }, range }
    }
  }
  const quote = sel.toString().trim()
  if (!quote) return null
  const owner = el(range.commonAncestorContainer)?.closest<HTMLElement>('[data-block]') ?? from.closest<HTMLElement>('[data-block]')
  return { place: { block: owner?.dataset.block ?? null, at: null, quote }, range }
}

/** The pinned anchor as a place: a unit → `node <id>` / `step <n>` (1-based), a span or excerpt rows → `path:a-b`. */
export function placeOfPin(pinned: NonNullable<AnchorState['pinned']>, registry: ReadonlyMap<string, Registered>, blocks: readonly DocBlock[], excerpts: readonly BoardExcerpt[]): Place {
  const reg = registry.get(pinned.by)
  const code: BoardAt | null = pinned.anchor.kind === 'code' ? { kind: 'code', path: pinned.anchor.path, lines: pinned.anchor.lines ?? null } : null
  if (reg?.kind === 'unit') {
    const unitId = pinned.by.slice(pinned.by.lastIndexOf(':') + 1)
    const block = blocks.find((b) => b.id === reg.blockId)
    const diagram = block?.type === 'codeBlock' ? classifyCode(block) : null
    const at: BoardAt | null = diagram?.kind === 'diagram' ? (diagram.diagram.type === 'Graph' ? { kind: 'node', id: unitId } : { kind: 'step', n: Number(unitId) + 1 }) : code
    const ref = formatAnchor(pinned.anchor)
    return { block: reg.blockId, at, quote: pinned.claim ? `${pinned.claim} · ${ref}` : ref }
  }
  if (reg?.kind === 'excerpt' && code?.lines) {
    const e = excerpts.find((x) => x.blockId === reg.blockId)
    const rows = e ? intersect(pinned.anchor, e.anchor.lines) : null
    return { block: reg.blockId, at: code, quote: e && rows ? rowsText(e, rows) : pinned.claim }
  }
  return { block: reg?.blockId ?? null, at: code, quote: pinned.claim }
}

/** M12 "Ask to refresh": the stale anchors, one per line with their state, under a `stale <k>` header. */
export function placeOfStale(stale: readonly { key: string; loc: Located }[]): Place {
  const quote = stale.map((s) => `${s.key.replace(/^excerpt:/, 'Excerpt block ')} · ${chipText(s.loc) ?? s.loc.status}`).join('\n')
  return { block: null, at: { kind: 'stale', count: stale.length }, quote }
}

/**
 * The floating **Ask** and ⌘⇧A. `scroll` is the `.board-scroll` box the button is positioned in (absolute, so it
 * scrolls with the words); the button takes `mousedown` without stealing the selection. Coarse pointers get a 44 px
 * target below the selection, clear of the platform's own callout above it (§2.10).
 */
export function AskFloat({ scroll, excerpts, blocks, onAsk }: { scroll: RefObject<HTMLDivElement | null>; excerpts: readonly BoardExcerpt[]; blocks: readonly DocBlock[]; onAsk: (place: Place) => void }) {
  const store = useAnchorStore()
  const [sel, setSel] = useState<{ place: Place; top: number; left: number } | null>(null)
  useEffect(() => {
    const onChange = () => {
      const box = scroll.current
      const hit = placeOfSelection(document.getSelection(), box?.querySelector('.board-column') ?? null, excerpts)
      if (!hit || !box) return setSel(null)
      const r = hit.range.getBoundingClientRect()
      const b = box.getBoundingClientRect()
      setSel({
        place: hit.place,
        top: r.bottom - b.top + box.scrollTop + 8,
        left: Math.max(8, Math.min(r.right - b.left + box.scrollLeft, b.width - 88)),
      })
    }
    document.addEventListener('selectionchange', onChange)
    return () => document.removeEventListener('selectionchange', onChange)
  }, [scroll, excerpts])
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || !e.shiftKey || e.key.toLowerCase() !== 'a' || e.defaultPrevented) return
      const t = e.target as HTMLElement | null
      if (t && (t.tagName === 'TEXTAREA' || t.tagName === 'INPUT' || t.isContentEditable)) return
      e.preventDefault()
      const s = store.getState()
      onAsk(sel?.place ?? (s.pinned ? placeOfPin(s.pinned, s.registry, blocks, excerpts) : { block: null, at: null, quote: null }))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [store, sel, blocks, excerpts, onAsk])
  if (!sel) return null
  const at = sel.place.at
  return (
    <button
      type="button"
      className="board-ask board-chrome"
      style={{ top: sel.top, left: sel.left } as CSSProperties}
      title={`Ask about ${at ? formatAt(at) : 'this'} (⌘⇧A)`}
      aria-keyshortcuts="Meta+Shift+A"
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => {
        onAsk(sel.place)
        document.getSelection()?.removeAllRanges()
      }}
    >
      <MessageSquareQuote /> Ask
    </button>
  )
}
