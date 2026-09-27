// Design 71 §2.2 / M2 / M9 — sections derive from `##` blocks. Everything before the first `##` is the preamble
// (title, lead). Collapse is UI state keyed `board:<doc>:<blockId>` in `localStorage` — never in the document
// (R §A.6 "layout is renderer-owned"). A collapsed heading dims and shows "N blocks · M anchors".
import { ChevronDown } from 'lucide-react'
import { type ReactNode, useCallback, useState } from 'react'
import { type DocBlock, classifyCode, codeAnchor, plainText } from './blocks'

export type Section = {
  /** The `##` block, or `null` for the preamble. */
  heading: DocBlock | null
  blocks: DocBlock[]
  anchors: number
}

export type TocEntry = { id: string; level: 1 | 2 | 3; text: string; section: string | null }

/** Split the top-level blocks on `##` headings; the preamble (if any) comes first with `heading: null`. */
export function sectionsOf(blocks: DocBlock[]): Section[] {
  const out: Section[] = []
  let cur: Section = { heading: null, blocks: [], anchors: 0 }
  for (const b of blocks) {
    if (b.type === 'heading' && b.props.level === 2) {
      if (cur.heading || cur.blocks.some((x) => x.type !== 'frontmatter')) out.push(cur)
      cur = { heading: b, blocks: [], anchors: 0 }
      continue
    }
    cur.blocks.push(b)
  }
  if (cur.heading || cur.blocks.some((x) => x.type !== 'frontmatter')) out.push(cur)
  for (const s of out) s.anchors = countAnchors(s.blocks)
  return out
}

/** `#`/`##`/`###` blocks in order; `section` is the enclosing `##` (a `##` is its own section). */
export function tocOf(blocks: DocBlock[]): TocEntry[] {
  const out: TocEntry[] = []
  let section: string | null = null
  for (const b of blocks) {
    if (b.type !== 'heading') continue
    const level = b.props.level as 1 | 2 | 3
    if (level === 2) section = b.id
    out.push({ id: b.id, level, text: plainText(b.content), section: level === 1 ? null : section })
  }
  return out
}

/** Anchored things the reader can touch: `code:` links, `Excerpt`s, diagram units with a `ref`. */
export function countAnchors(blocks: DocBlock[]): number {
  let n = 0
  const inline = (content: unknown) => {
    if (!Array.isArray(content)) return
    for (const c of content as { type: string; href?: string }[]) if (c.type === 'link' && c.href && codeAnchor(c.href)) n++
  }
  const walk = (b: DocBlock) => {
    if (b.type === 'codeBlock') {
      const c = classifyCode(b)
      if (c.kind === 'excerpt') n++
      else if (c.kind === 'diagram') n += countRefs(c.diagram.props)
    } else if (b.type === 'table') {
      for (const row of b.content.rows) for (const cell of row.cells) inline(Array.isArray(cell) ? cell : cell.content)
    } else inline(b.content)
    for (const child of b.children) walk(child as DocBlock)
  }
  for (const b of blocks) walk(b)
  return n
}

function countRefs(v: unknown): number {
  if (Array.isArray(v)) return v.reduce<number>((n, x) => n + countRefs(x), 0)
  if (v && typeof v === 'object') {
    let n = 0
    for (const [k, x] of Object.entries(v)) n += k === 'ref' && typeof x === 'string' ? 1 : countRefs(x)
    return n
  }
  return 0
}

export const collapseKey = (doc: string, blockId: string) => `board:${doc}:${blockId}`

/** Collapsed section ids of one board, read from and written to `localStorage` (M9: "persisted per session"). */
export function useCollapsed(doc: string, sectionIds: string[]) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set(sectionIds.filter((id) => localStorage.getItem(collapseKey(doc, id)) === '1')))
  const set = useCallback(
    (id: string, on: boolean) =>
      setCollapsed((prev) => {
        if (prev.has(id) === on) return prev
        const next = new Set(prev)
        if (on) {
          next.add(id)
          localStorage.setItem(collapseKey(doc, id), '1')
        } else {
          next.delete(id)
          localStorage.removeItem(collapseKey(doc, id))
        }
        return next
      }),
    [doc],
  )
  return {
    collapsed,
    toggle: useCallback((id: string) => set(id, !collapsed.has(id)), [collapsed, set]),
    expand: useCallback((id: string) => set(id, false), [set]),
  }
}

/**
 * One `##` section: chevron at `left: -24px` (rest opacity .45), the heading, the collapsed summary in the mono
 * register, the body unmounted while collapsed (so diagrams lay out at real width when it opens; block ids are
 * already "seen", so nothing rises).
 */
export function SectionView({ section, collapsed, onToggle, heading, children }: { section: Section; collapsed: boolean; onToggle: () => void; heading: ReactNode; children: ReactNode }) {
  const id = section.heading!.id
  const blocks = section.blocks.filter((b) => b.type !== 'frontmatter').length
  return (
    <section className="board-section" data-section={id} data-collapsed={collapsed || undefined}>
      <div className="board-section-head">
        <button type="button" className="board-section-toggle" aria-expanded={!collapsed} aria-controls={`${id}-body`} aria-label={collapsed ? 'Expand section' : 'Collapse section'} onClick={onToggle}>
          <ChevronDown size={18} strokeWidth={1.75} aria-hidden />
        </button>
        {heading}
        {collapsed && (
          <span className="board-section-meta board-chrome">
            {blocks} {blocks === 1 ? 'block' : 'blocks'} · {section.anchors} {section.anchors === 1 ? 'anchor' : 'anchors'}
          </span>
        )}
      </div>
      {!collapsed && (
        <div className="board-section-body" id={`${id}-body`}>
          {children}
        </div>
      )}
    </section>
  )
}
