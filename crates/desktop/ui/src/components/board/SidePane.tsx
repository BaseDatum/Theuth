// Design 71 §2.6 / M5: the code beside the sentence. One pane, one content — pinning replaces. Header (mono): the
// claim that pinned, the anchor, close. Body: the board's `Excerpt` that covers the range with the pinned rows lit
// (R §B.7 `heightMode: content`), else the range fetched live through `code_locate` (§2.5, M3: a link is never a
// dead end) — with its live chip, or the reason there is nothing to show and "Open the file".
import { openPath } from '@tauri-apps/plugin-opener'
import { ExternalLink, MessageSquareQuote, X } from 'lucide-react'
import { type ReactNode, useEffect, useRef } from 'react'
import { notify as toast } from '@/lib/notify'
import { useStore } from 'zustand'
import { Excerpt } from '@/components/chat/surface/Excerpt'
import { Button } from '@/components/ui/button'
import { isDesktop } from '@/lib/platform'
import { cn } from '@/lib/utils'
import type { Located } from "@/rpc/bindings/method.codeindex.output"
import { type Anchor, type AnchorState, contains, formatAnchor, intersect } from './anchors'
import type { BoardExcerpt } from './blocks'
import { chipText, langOf, toLocate, useLiveStore, useLocated } from './live'

/** The board's code window for `a`: the first excerpt containing its range; for a bare path, the file's first excerpt. */
export function coveringExcerpt(excerpts: readonly BoardExcerpt[], a: Anchor): BoardExcerpt | null {
  if (a.kind !== 'code') return null
  return excerpts.find((e) => contains(e.anchor, a)) ?? (a.lines ? null : (excerpts.find((e) => e.anchor.path === a.path) ?? null))
}

/** M11: the live chip — `changed` / `moved → L88–L101` / `missing` — and "show current" when the tree still has text. */
export function LiveChip({ loc, current, onToggle }: { loc: Located | undefined; current?: boolean; onToggle?: () => void }) {
  const text = chipText(loc)
  if (!text || !loc) return null
  return (
    <span className="flex shrink-0 items-center gap-1" data-live={loc.status}>
      <span className={cn('rounded border px-1 text-[10px] tracking-[0.08em] uppercase', loc.status === 'missing' ? 'text-destructive' : 'text-fg-accent')}>{text}</span>
      {onToggle && loc.text != null && (
        <button type="button" className="text-[10px] tracking-[0.04em] hover:text-foreground" onClick={onToggle}>
          {current ? 'show quoted' : 'show current'}
        </button>
      )}
    </span>
  )
}

export function SidePane({
  pinned,
  excerpts,
  root,
  onClose,
  onAsk,
}: {
  pinned: NonNullable<AnchorState['pinned']>
  excerpts: readonly BoardExcerpt[]
  root: string | null
  onClose: () => void
  /** §2.6 **Ask about this** (M10): the pinned place as a `[board …]` question. */
  onAsk: () => void
}) {
  const { anchor, claim } = pinned
  const excerpt = coveringExcerpt(excerpts, anchor)
  const body = useRef<HTMLDivElement>(null)
  // A new pin: the window scrolls to its first lit row (M4 "the excerpt scrolls into view").
  useEffect(() => {
    body.current?.querySelector('[data-anchor-active]')?.scrollIntoView({ block: 'center' })
  }, [anchor])
  const label = anchor.kind === 'code' ? formatAnchor(anchor) : `block ${anchor.id}`
  return (
    <aside className="board-pane" aria-label="Code window">
      <div className="board-pane-head flex items-start gap-2 border-b border-border-subtle px-3 py-2 text-fg-tertiary">
        <div className="min-w-0 flex-1">
          {claim && <div className="line-clamp-3 text-foreground">{claim}</div>}
          <div className="truncate" title={label}>
            {label}
          </div>
        </div>
        <Button variant="ghost" size="icon-sm" aria-label="Ask about this" title="Ask about this (⌘⇧A)" className="board-pane-ask" onClick={onAsk}>
          <MessageSquareQuote />
        </Button>
        <Button variant="ghost" size="icon-sm" aria-label="Close side pane" onClick={onClose}>
          <X />
        </Button>
      </div>
      <div ref={body} className="board-pane-body">
        {excerpt ? (
          <Excerpt {...excerpt.props} full active={intersect(anchor, excerpt.anchor.lines)} />
        ) : anchor.kind === 'code' ? (
          <LiveWindow anchor={anchor} root={root} />
        ) : (
          <Placeholder label={label} text="A block anchor has no code window." />
        )}
      </div>
    </aside>
  )
}

/** §2.5 live window: the anchor's current lines from the working tree; asked for on demand when the open batch missed it. */
export function LiveWindow({ anchor, root }: { anchor: Anchor & { kind: 'code' }; root: string | null }) {
  const live = useLiveStore()
  const key = formatAnchor(anchor)
  const loc = useLocated(key)
  const error = useStore(live, (s) => s.error)
  const pending = useStore(live, (s) => s.pending > 0)
  useEffect(() => {
    live.getState().ensure([{ key, anchor: toLocate(anchor) }])
  }, [live, key, anchor])
  const open = root && isDesktop() && (
    <Button variant="outline" size="sm" className="mt-3" onClick={() => openPath(`${root}/${anchor.path}`).catch((e: unknown) => toast.error(String(e)))}>
      <ExternalLink /> Open the file
    </Button>
  )
  if (loc?.text != null) {
    return (
      <div data-live-window={loc.status}>
        <Excerpt path={anchor.path} text={loc.text} start={loc.start} lang={langOf(anchor.path)} full={!!anchor.lines} chip={<LiveChip loc={loc} />} />
        <div className="mt-2 font-mono text-[11px] text-fg-tertiary">live from the working tree</div>
      </div>
    )
  }
  const text = loc ? `${key} is missing: the file, range or symbol is gone.` : error ? `Could not verify: ${error}` : pending ? 'Fetching the current lines…' : 'No excerpt on this board covers these lines.'
  return (
    <Placeholder label={key} text={text} status={loc?.status}>
      {open}
    </Placeholder>
  )
}

function Placeholder({ label, text, status, children }: { label: string; text: string; status?: Located['status']; children?: ReactNode }) {
  return (
    <div className="rounded-md border p-3 font-mono text-xs text-fg-tertiary" data-live-window={status}>
      <p className="text-foreground">{label}</p>
      <p className="mt-1">{text}</p>
      {children}
    </div>
  )
}
