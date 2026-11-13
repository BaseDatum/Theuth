/**
 * 147 §11.9 — the list form (< 600 px panel, phones): the element's children as an indented list under a crumb line and
 * a title (`Overall architecture · 11 elements · 15 of 16 edges · written 3 min ago`); tap on a group = drill the
 * list, tap on a leaf = the detail sheet (60 vh) with the fields, anchors and the diagram as a text adjacency list
 * (65 §3.3's fallback). No SVG, no thumbnails, no hover-only affordance.
 */
import type { ReactNode } from 'react'
import { adjacencyText } from '@/components/chat/surface/Graph'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { kindGlyph } from '@/lib/codemap/labels'
import { ago, elementGraph, elementStats, type Mark } from '@/lib/codemap/map'
import type { GraphData } from '@/lib/graph-layout'
import { cn } from '@/lib/utils'
import type { MapElement } from "@/rpc/bindings/method.codeindex.output"
import './codemap.css'

export type MapListProps = {
  element: MapElement
  marks: ReadonlyMap<string, readonly Mark[]>
  /** The child whose detail sheet is open. */
  open: string | null
  /** The sheet body (`MapDetail`), rendered when `open` names a child. */
  detail: ReactNode
  onDrill(id: string): void
  onUp(): void
  onCrumb(id: string): void
  onOpen(id: string | null): void
}

/** Title line: name · stats · `written 3 min ago` when generated. */
export function listTitle(el: MapElement): string {
  const parts = [elementStats(el)]
  if (el.provenance) parts.push(`written ${ago(el.provenance.generated_ms)}`)
  else if (el.origin === 'structure') parts.push('structure view')
  else if (el.origin === 'imported') parts.push('imported')
  return parts.join(' · ')
}

export function MapList(p: MapListProps) {
  const el = p.element
  const canUp = el.crumbs.length > 1
  return (
    <div data-map-list className="flex h-full min-h-0 flex-col font-sans text-[12px]">
      <nav aria-label="Breadcrumbs" className="flex h-7 shrink-0 items-center gap-1.5 overflow-hidden border-b border-border-subtle px-3 font-mono text-[11px] whitespace-nowrap text-fg-secondary">
        {canUp && (
          <button type="button" className="mr-1 rounded border border-border-subtle px-1.5 text-foreground" onClick={p.onUp} aria-label="Up one level">
            ‹
          </button>
        )}
        {el.crumbs.map(([id, name], i) => (
          <span key={id} className="flex items-center gap-1.5 truncate">
            {i > 0 && <span className="opacity-60">›</span>}
            {i === el.crumbs.length - 1 ? (
              <b className="text-foreground">{name}</b>
            ) : (
              <button type="button" className="hover:underline" onClick={() => p.onCrumb(id)}>
                {name}
              </button>
            )}
          </span>
        ))}
      </nav>
      <div className="min-h-0 flex-1 overflow-auto">
        <header className="border-b border-border-subtle px-3.5 pt-2.5 pb-1.5">
          <h2 className="m-0 text-[15px] leading-tight font-semibold">{el.name}</h2>
          <p className="m-0 truncate text-fg-secondary">{listTitle(el)}</p>
          {el.fields.description && <p className="mt-1 mb-0 text-fg-secondary">{el.fields.description}</p>}
        </header>
        {el.children.length === 0 ? (
          <button type="button" className="w-full px-3.5 py-3 text-left text-fg-secondary" onClick={() => p.onOpen(el.id)}>
            leaf — tap for files and symbols
          </button>
        ) : (
          <ul className="m-0 list-none p-0">
            {el.children.map((c) => {
              const marks = p.marks.get(c.id) ?? []
              return (
                <li key={c.id} className="border-b border-border-subtle">
                  <button
                    type="button"
                    data-row={c.id}
                    className="flex min-h-[46px] w-full items-center gap-2.5 px-3.5 py-1.5 text-left hover:bg-layer-2"
                    onClick={() => (c.leaf ? p.onOpen(c.id) : p.onDrill(c.id))}
                  >
                    <span className={cn('h-7 w-[3px] shrink-0 rounded-sm', c.leaf ? 'bg-border-strong' : 'bg-info')} aria-hidden />
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center gap-1.5 text-[13px] leading-tight font-semibold">
                        <span className="truncate">
                          {kindGlyph(c.kind)} {c.name}
                        </span>
                        {marks.includes('edited') && <span className="codemap-mark edited" aria-label="edited" />}
                        {marks.includes('read') && <span className="codemap-mark read" aria-label="read" />}
                        {c.stale && <span className="codemap-badge stale">stale</span>}
                      </span>
                      <span className="block truncate font-mono text-[10px] text-fg-secondary">
                        {c.path ?? c.node}
                        {c.files ? ` · ${c.files} files` : ''}
                        {c.lines ? ` · ${c.lines.toLocaleString()} lines` : ''}
                      </span>
                    </span>
                    {!c.leaf && <span className="shrink-0 font-mono text-[10px] text-fg-secondary">{c.thumbnail?.nodes.length ?? ''} inside</span>}
                    <span className="shrink-0 text-[14px] text-fg-secondary">›</span>
                  </button>
                </li>
              )
            })}
          </ul>
        )}
        {el.edges.length > 0 && (
          <pre className="m-0 px-3.5 py-2 font-mono text-[11px] leading-5 whitespace-pre-wrap text-fg-secondary" data-adjacency>
            {adjacencyText(elementGraph(el, 'TB').graph as GraphData)}
          </pre>
        )}
      </div>
      <Sheet open={p.open !== null} onOpenChange={(o) => !o && p.onOpen(null)}>
        <SheetContent side="bottom" showCloseButton className="h-[60vh] gap-0 overflow-hidden p-0">
          <SheetTitle className="sr-only">Element</SheetTitle>
          <div className="relative h-full min-h-0">{p.detail}</div>
        </SheetContent>
      </Sheet>
    </div>
  )
}
