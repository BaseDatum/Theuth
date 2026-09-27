// Design 71 §2.2 — the board's own read mode. The document's blocks (read headlessly, `blocks.ts`) are mapped by
// the renderer: headings, prose, lists, quotes, tables, rules; `json-render` blocks through the catalog (an
// `Excerpt` becomes an anchored code window in flow, a `Graph`/`Sequence` takes the wide column); `code:` links
// become anchor spans. One anchor store per board (§2.3); pinning opens the side pane (§2.6). Every anchor is
// verified against the working tree on open and on Verify (§2.5 live, `live.ts`). `##` blocks make sections with
// collapse + TOC (`Sections.tsx`, `Toc.tsx`, M9); new blocks rise in and the badge says who is drawing (`Reveal.tsx`,
// M7). Diagram units and the tour come from `Tour.tsx` / `units.ts` (M14, M6). Select-to-ask (M10/M13) is `Ask.tsx`:
// a place on the board becomes a `[board …]` question in the bound session's composer (`lib/board-ask`).
import { openUrl } from '@tauri-apps/plugin-opener'
import { MessageSquareQuote, RefreshCw } from 'lucide-react'
import { type CSSProperties, Fragment, type ReactNode, createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { MermaidFence } from '@/components/chat/Mermaid'
import { Excerpt, type ExcerptProps } from '@/components/chat/surface/Excerpt'
import { FenceFallback, JsonRenderFence } from '@/components/chat/surface/JsonRenderFence'
import { Button } from '@/components/ui/button'
import type { AttachRef } from '@/lib/attach'
import { composeAsk } from '@/lib/board-ask'
import { useBoards } from '@/lib/boards'
import type { EngineProvider } from '@/lib/doc/provider'
import { usePointerCoarse } from '@/lib/mobile'
import { useApi } from '@/lib/rpc'
import { type Anchor, AnchorContext, HOVER_MS, createAnchorStore, formatAnchor, intersect, useActiveAnchor, useAnchorStore, useAnchored } from './anchors'
import { AskFloat, type Place, askBoard, placeOfPin, placeOfStale } from './Ask'
import { type DocBlock, type Inline, classifyCode, codeAnchor, excerptAnchor, excerptsOf, plainText, rootOf, useBlocks } from './blocks'
import { type LiveRequest, LiveContext, chipText, collectRequests, createLiveStore, excerptKey, staleOf, useLocated } from './live'
import { DrawingBadge, useReveal } from './Reveal'
import { type TocEntry, SectionView, sectionsOf, tocOf, useCollapsed } from './Sections'
import { LiveChip, SidePane } from './SidePane'
import { Toc, type TocProps, useActiveEntry, useRail } from './Toc'
import { BoardDiagram, Tour, useTourBoard, useTourHotkeys, useTourPersist } from './Tour'
import { focusBy } from './units'
import './board.css'

const PANE_KEY = 'board-pane-width'
export const PANE_MIN = 360
export const PANE_MAX = 920
export const PANE_DEFAULT = 560

/** Block ids that arrived after the first render (M7): they carry `data-rise` for the rest of the mount. */
const FreshContext = createContext<ReadonlySet<string>>(new Set())
function useRise(id: string): { 'data-rise'?: true } {
  return useContext(FreshContext).has(id) ? { 'data-rise': true } : {}
}

export function BoardReader({ provider }: { provider: EngineProvider }) {
  const store = useMemo(createAnchorStore, [provider])
  const blocks = useBlocks(provider)
  const excerpts = useMemo(() => excerptsOf(blocks), [blocks])
  const root = useMemo(() => rootOf(blocks), [blocks])
  const api = useApi()
  const rootRef = useRef(root)
  rootRef.current = root
  // §2.5 live: one batch per open, cached by anchor; a Yjs insert locates only what it added.
  const live = useMemo(
    () => createLiveStore((anchors) => (rootRef.current ? api.code_locate(rootRef.current, anchors) : Promise.reject(new Error('this board has no root')))),
    [provider, api],
  )
  const [reqs, setReqs] = useState<LiveRequest[]>([])
  useEffect(() => {
    let on = true
    void collectRequests(blocks).then((r) => {
      if (!on) return
      setReqs(r)
      live.getState().ensure(r)
    })
    return () => {
      on = false
    }
  }, [blocks, live])
  const results = useStore(live, (s) => s.results)
  const liveError = useStore(live, (s) => s.error)
  const verifying = useStore(live, (s) => s.pending > 0)
  const stale = staleOf(results, reqs)
  // M10: a place → header + quote → the bound session's composer (§0 Q5), the board pinned as `[attached note …]`.
  const boardRef = useMemo<AttachRef>(() => ({ kind: 'note', id: provider.id, title: provider.meta?.title || 'Untitled', meta: 'board' }), [provider, provider.meta?.title])
  const ask = useCallback((place: Place) => askBoard(provider.id, composeAsk({ doc: provider.id, block: place.block, at: place.at }, place.quote), boardRef), [provider.id, boardRef])
  const askPinned = () => {
    const s = store.getState()
    if (s.pinned) ask(placeOfPin(s.pinned, s.registry, blocks, excerpts))
  }
  const askRefresh = () =>
    ask(
      placeOfStale(
        reqs.flatMap((r) => {
          const loc = results.get(r.key)
          return loc && loc.status !== 'same' ? [{ key: r.key, loc }] : []
        }),
      ),
    )
  const pinned = useStore(store, (s) => s.pinned)
  const tour = useStore(store, (s) => s.tour)
  const serif = useBoards((s) => s.serif)
  const sections = useMemo(() => sectionsOf(blocks), [blocks])
  const toc = useMemo(() => tocOf(blocks), [blocks])
  const { collapsed, toggle, expand } = useCollapsed(
    provider.id,
    sections.flatMap((s) => (s.heading ? [s.heading.id] : [])),
  )
  const reveal = useReveal(blocks)
  const drawing = useMemo(() => {
    const last = reveal.last
    const s = last ? sections.find((x) => x.heading?.id === last || x.blocks.some((b) => b.id === last)) : undefined
    return s?.heading ? plainText(s.heading.content) : null
  }, [reveal.last, sections])
  const scroll = useRef<HTMLDivElement>(null)
  const activeEntry = useActiveEntry(scroll, toc)
  const rail = useRail(!!pinned)
  // M9: the TOC expands a collapsed section before scrolling to the heading (the body remounts, so wait a frame).
  const jump = (e: TocEntry) => {
    if (e.section) expand(e.section)
    requestAnimationFrame(() => {
      const behavior: ScrollBehavior = matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth'
      document.getElementById(e.id)?.scrollIntoView({ block: 'start', behavior })
    })
  }
  const [width, setWidth] = useState(() => {
    const n = Number(localStorage.getItem(PANE_KEY))
    return n >= PANE_MIN && n <= PANE_MAX ? n : PANE_DEFAULT
  })
  const [dragging, setDragging] = useState(false)
  const page = useRef<HTMLDivElement>(null)

  // `Escape` pops the pin and returns focus to the element that pinned (M5); while a tour is open, `Tour` owns `Escape`.
  useEffect(() => {
    if (!pinned) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented || store.getState().tour) return
      e.preventDefault()
      const by = pinned.by
      store.getState().unpin()
      focusBy(page.current, by)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [pinned, store])

  const drag = (e: React.PointerEvent<HTMLDivElement>) => {
    const grid = page.current
    if (!grid) return
    e.currentTarget.setPointerCapture(e.pointerId)
    setDragging(true)
    const right = grid.getBoundingClientRect().right
    const move = (ev: PointerEvent) => setWidth(Math.min(PANE_MAX, Math.max(PANE_MIN, Math.round(right - ev.clientX))))
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      setDragging(false)
      setWidth((w) => {
        localStorage.setItem(PANE_KEY, String(w))
        return w
      })
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  return (
    <AnchorContext value={store}>
      <TourWiring provider={provider} blocks={blocks} />
      <LiveContext value={live}>
        <FreshContext value={reveal.risen}>
          <div className="board-shell flex min-h-0 flex-1 flex-col" data-serif={serif ? undefined : 'false'}>
            <div ref={page} className="board-page min-h-0 flex-1" data-pinned={pinned ? true : undefined} data-rail={rail || undefined} style={{ '--board-pane-width': `${width}px` } as CSSProperties}>
              <BoardToc blocks={blocks} entries={toc} active={activeEntry} onSelect={jump} />
              <div ref={scroll} className="board-scroll">
                <DrawingBadge doc={provider.id} section={drawing} />
                <AskFloat scroll={scroll} excerpts={excerpts} blocks={blocks} onAsk={ask} />
                <article className="board-column">
                  <div
                    className="board-chrome flex flex-wrap items-center gap-x-3 gap-y-1 py-2 text-fg-tertiary"
                    data-anchors={reqs.length}
                    data-stale={stale}
                  >
                    <span>
                      {reqs.length} {reqs.length === 1 ? 'anchor' : 'anchors'} ·{' '}
                      {liveError ? <span className="text-destructive">verify failed: {liveError}</span> : verifying && results.size === 0 ? 'verifying…' : `${stale} stale`}
                    </span>
                    <Button variant="ghost" size="xs" className="h-6 px-1.5 font-mono text-[11px]" disabled={verifying || reqs.length === 0} onClick={() => live.getState().verify(reqs)}>
                      <RefreshCw className={verifying ? 'animate-spin' : undefined} /> Verify
                    </Button>
                    {stale > 0 && (
                      <Button variant="ghost" size="xs" className="h-6 px-1.5 font-mono text-[11px]" title="Sends the stale anchors to the board's session (M12)" onClick={askRefresh}>
                        <MessageSquareQuote /> Ask to refresh
                      </Button>
                    )}
                  </div>
                  {sections.map((s) =>
                    s.heading ? (
                      <SectionView key={s.heading.id} section={s} collapsed={collapsed.has(s.heading.id)} onToggle={() => toggle(s.heading!.id)} heading={<Block block={s.heading} />}>
                        <Blocks blocks={s.blocks} />
                      </SectionView>
                    ) : (
                      <Blocks key="preamble" blocks={s.blocks} />
                    ),
                  )}
                </article>
              </div>
              {pinned && (
                <div
                  role="separator"
                  aria-orientation="vertical"
                  aria-label="Resize side pane"
                  className="board-resizer"
                  data-dragging={dragging || undefined}
                  onPointerDown={drag}
                />
              )}
              {pinned && <SidePane pinned={pinned} excerpts={excerpts} root={root} onClose={() => store.getState().unpin()} onAsk={askPinned} />}
            </div>
            {tour && <Tour blocks={blocks} excerpts={excerpts} page={page.current} />}
          </div>
        </FreshContext>
      </LiveContext>
    </AnchorContext>
  )
}

/** M6 wiring that needs the anchor store: `T` for "Tour this board" and the persisted `{blockId, index}`. */
function TourWiring({ provider, blocks }: { provider: EngineProvider; blocks: DocBlock[] }) {
  useTourHotkeys(blocks)
  useTourPersist(provider.id, blocks)
  return null
}

/** The TOC with **Tour this board** (§2.6): `useTourBoard` needs the anchor store, so it lives under the context. */
function BoardToc({ blocks, ...props }: { blocks: DocBlock[] } & Omit<TocProps, 'onTour'>) {
  const tourBoard = useTourBoard(blocks)
  return <Toc {...props} onTour={tourBoard} />
}

/** Top-level blocks; consecutive list items fold into one `ul`/`ol` (BlockNote stores them as siblings). */
function Blocks({ blocks }: { blocks: DocBlock[] }) {
  const out: ReactNode[] = []
  for (let i = 0; i < blocks.length; i++) {
    const b = blocks[i]!
    if (b.type === 'bulletListItem' || b.type === 'numberedListItem' || b.type === 'checkListItem' || b.type === 'toggleListItem') {
      const type = b.type
      let j = i
      while (j < blocks.length && blocks[j]!.type === type) j++
      const items = blocks.slice(i, j)
      const List = type === 'numberedListItem' ? 'ol' : 'ul'
      out.push(
        <div key={b.id} className="board-prose">
          <List>
            {items.map((it) => (
              <ListItem key={it.id} block={it} />
            ))}
          </List>
        </div>,
      )
      i = j - 1
      continue
    }
    out.push(<Block key={b.id} block={b} />)
  }
  return <>{out}</>
}

function ListItem({ block }: { block: DocBlock }) {
  const claim = plainText(block.content)
  const rise = useRise(block.id)
  return (
    <li id={block.id} data-block={block.id} {...rise}>
      {block.type === 'checkListItem' && <input type="checkbox" checked={block.props.checked} readOnly className="mr-2 align-middle" aria-label="Done" />}
      <InlineContent content={block.content} blockId={block.id} claim={claim} />
      {block.children.length > 0 && <Blocks blocks={block.children as DocBlock[]} />}
    </li>
  )
}

function Block({ block }: { block: DocBlock }) {
  const rise = useRise(block.id)
  switch (block.type) {
    case 'frontmatter':
      return null
    case 'heading': {
      const claim = plainText(block.content)
      const H = block.props.level === 1 ? 'h1' : block.props.level === 2 ? 'h2' : 'h3'
      return (
        <H id={block.id} data-block={block.id} className="board-prose" {...rise}>
          <InlineContent content={block.content} blockId={block.id} claim={claim} />
        </H>
      )
    }
    case 'paragraph': {
      if (block.content.length === 0 && block.children.length === 0) return null
      const claim = plainText(block.content)
      return (
        <div className="board-prose" data-block={block.id} {...rise}>
          <p id={block.id}>
            <InlineContent content={block.content} blockId={block.id} claim={claim} />
          </p>
          {block.children.length > 0 && <Blocks blocks={block.children as DocBlock[]} />}
        </div>
      )
    }
    case 'quote': {
      const claim = plainText(block.content)
      return (
        <div className="board-prose" data-block={block.id} {...rise}>
          <blockquote id={block.id}>
            <InlineContent content={block.content} blockId={block.id} claim={claim} />
          </blockquote>
        </div>
      )
    }
    case 'divider':
      return <hr id={block.id} data-block={block.id} {...rise} />
    case 'table':
      return <Table block={block} />
    case 'codeBlock':
      return <CodeBlock block={block} />
    default:
      // A block type the reader has no mapping for (none in the schema today): the quiet one-liner (R §D.3).
      return (
        <div className="board-block" data-block={block.id} {...rise}>
          <FenceFallback error={`unsupported block ${block.type}`} source={JSON.stringify(block, null, 2)} title="Block omitted" />
        </div>
      )
  }
}

function Table({ block }: { block: Extract<DocBlock, { type: 'table' }> }) {
  const rows = block.content.rows
  const headerRows = block.content.headerRows ?? 0
  const rise = useRise(block.id)
  return (
    <div className="board-block overflow-x-auto" data-block={block.id} {...rise}>
      <table id={block.id} className="board-table">
        <tbody>
          {rows.map((row, r) => (
            <tr key={r}>
              {row.cells.map((cell, c) => {
                const content = Array.isArray(cell) ? cell : cell.content
                const Cell = r < headerRows ? 'th' : 'td'
                const props = Array.isArray(cell) ? {} : { colSpan: cell.props.colspan, rowSpan: cell.props.rowspan }
                return (
                  <Cell key={c} {...props}>
                    <InlineContent content={content} blockId={block.id} claim={plainText(content)} />
                  </Cell>
                )
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function CodeBlock({ block }: { block: Extract<DocBlock, { type: 'codeBlock' }> }) {
  const c = useMemo(() => classifyCode(block), [block])
  const fresh = useContext(FreshContext).has(block.id)
  const rise = fresh ? { 'data-rise': true as const } : {}
  switch (c.kind) {
    case 'excerpt':
      return <BoardExcerpt blockId={block.id} props={c.props} />
    case 'diagram':
      return <BoardDiagram blockId={block.id} diagram={c.diagram} fresh={fresh} />
    case 'surface':
      return (
        <div className="board-block" data-block={block.id} {...rise}>
          <JsonRenderFence code={c.code} isIncomplete={false} language="json-render" />
        </div>
      )
    case 'invalid':
      return (
        <div className="board-block" data-block={block.id} {...rise}>
          <FenceFallback error={c.error} source={c.code} title="Interactive view unavailable" />
        </div>
      )
    case 'mermaid':
      return (
        <div className="board-block board-wide" data-block={block.id} {...rise}>
          <MermaidFence code={c.code} isIncomplete={false} language="mermaid" />
        </div>
      )
    case 'code':
      return (
        <pre className="board-code" data-block={block.id} data-lang={c.lang || undefined} {...rise}>
          {c.text}
        </pre>
      )
  }
}

/** Styled runs and links; a `code:` href is an anchor span, any other link opens outside. */
function InlineContent({ content, blockId, claim }: { content: DocBlock['content']; blockId: string; claim: string }) {
  if (!Array.isArray(content)) return null
  return (
    <>
      {(content as Inline[]).map((c, i) => {
        if (c.type === 'text') return <Styled key={i} run={c} />
        if (c.type !== 'link') return null
        const anchor = codeAnchor(c.href)
        const inner = c.content.map((t, j) => <Styled key={j} run={t} />)
        if (anchor) {
          return (
            <AnchorSpan key={i} anchor={anchor} blockId={blockId} claim={claim}>
              {inner}
            </AnchorSpan>
          )
        }
        return (
          <a
            key={i}
            href={c.href}
            className="underline decoration-dotted underline-offset-2 hover:text-fg-accent"
            onClick={(e) => {
              e.preventDefault()
              openUrl(c.href).catch(() => window.open(c.href, '_blank', 'noopener'))
            }}
          >
            {inner}
          </a>
        )
      })}
    </>
  )
}

function Styled({ run }: { run: Extract<Inline, { type: 'text' }> }) {
  let node: ReactNode = run.text
  const s = run.styles
  if (s.code) node = <code>{node}</code>
  if (s.bold) node = <strong>{node}</strong>
  if (s.italic) node = <em>{node}</em>
  if (s.underline) node = <u>{node}</u>
  if (s.strike) node = <s>{node}</s>
  return <Fragment>{node}</Fragment>
}

/**
 * M3/M4: a prose anchor. Focusable; hover (fine pointer, 120 ms) or focus lights everything sharing the anchor; click
 * pins. M11: `data-live` and the title carry the anchor's live state (`changed` / `moved → …` / `missing`).
 */
function AnchorSpan({ anchor, blockId, claim, children }: { anchor: Anchor; blockId: string; claim: string; children: ReactNode }) {
  const coarse = usePointerCoarse()
  const { props } = useAnchored({ anchor, kind: 'span', blockId }, { claim, coarse })
  const key = formatAnchor(anchor)
  const loc = useLocated(key)
  const chip = chipText(loc)
  return (
    <a
      href={`code:${key}`}
      className="board-anchor data-[live]:decoration-dotted data-[live=missing]:line-through"
      tabIndex={0}
      title={chip ? `${key} · ${chip}` : key}
      data-live={loc && loc.status !== 'same' ? loc.status : undefined}
      {...props}
    >
      {children}
    </a>
  )
}

/**
 * §2.5 in flow: the `Excerpt` as an anchored element (its range) whose gutter rows are row anchors (`path#L<n>`).
 * Pointer events are delegated from the wrapper — rows are pointer targets, the frame is the keyboard target — so
 * `Excerpt.tsx` itself stays a plain renderer.
 */
function BoardExcerpt({ blockId, props }: { blockId: string; props: ExcerptProps }) {
  const store = useAnchorStore()
  const coarse = usePointerCoarse()
  const anchor = useMemo(() => excerptAnchor(props), [props])
  const claim = props.caption ?? `${props.path}:${anchor.lines[0]}-${anchor.lines[1]}`
  const { id, active, props: frame } = useAnchored({ anchor, kind: 'excerpt', blockId }, { claim, coarse })
  const rise = useRise(blockId)
  const activeAnchor = useActiveAnchor()
  const rows = active && activeAnchor ? intersect(activeAnchor, anchor.lines) : null
  // M11 "show current": the live range replaces the quoted text; marks follow a `moved` range, vanish on `changed`.
  const loc = useLocated(excerptKey(blockId))
  const [current, setCurrent] = useState(false)
  const shown = useMemo(() => {
    if (!current || loc?.text == null) return props
    const delta = loc.start - anchor.lines[0]
    const mark = loc.status === 'moved' ? props.mark?.map(([a, b]) => [a! + delta, b! + delta]) : undefined
    return { ...props, text: loc.text, start: loc.start, mark }
  }, [current, loc, props, anchor])
  const timer = useRef<number | undefined>(undefined)
  const rowAnchor = (e: React.SyntheticEvent): Anchor => {
    const n = (e.target as HTMLElement).closest<HTMLElement>('[data-line]')?.dataset.line
    return n ? { kind: 'code', path: anchor.path, lines: [Number(n), Number(n)] } : anchor
  }
  const hover = coarse
    ? {}
    : {
        onMouseOver: (e: React.MouseEvent) => {
          const a = rowAnchor(e)
          clearTimeout(timer.current)
          timer.current = window.setTimeout(() => store.getState().setHover(a), HOVER_MS)
        },
        onMouseLeave: () => {
          clearTimeout(timer.current)
          const h = store.getState().hover
          if (h?.kind === 'code' && h.path === anchor.path) store.getState().setHover(null)
        },
      }
  return (
    <div
      className="board-block board-excerpt"
      data-block={blockId}
      {...rise}
      role="group"
      tabIndex={0}
      aria-label={`Excerpt ${props.path}`}
      {...frame}
      {...hover}
      onMouseEnter={undefined}
      onClick={(e) => {
        if ((e.target as HTMLElement).closest('button')) return
        e.preventDefault()
        store.getState().pin(rowAnchor(e), id, claim)
      }}
    >
      <Excerpt {...shown} active={rows} chip={<LiveChip loc={loc} current={current && loc?.text != null} onToggle={() => setCurrent((c) => !c)} />} />
    </div>
  )
}
