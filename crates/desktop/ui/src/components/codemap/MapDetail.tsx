/**
 * 147 §11.4 — the detail pane: honesty chips · name + path · description, then context / constraint / concern / todo /
 * note when present (omm's reading order, D5) · anchors as code links (*Ask about this*) · `Inside · N` (drill) ·
 * edges with engine-rendered evidence counts (`backed` solid · `inferred` dashed + reason) · also in · listing for
 * leaves · provenance line (§8) · actions Explain in chat · Snapshot to board · Regenerate element · Copy Mermaid, each
 * disabled with the reason when the engine lacks the call.
 */
import type { ReactNode } from 'react'
import { toast } from 'sonner'
import { appendDraft, prefillDraft } from '@/components/chat/Composer'
import { anchorOf, kindGlyph } from '@/lib/codemap/labels'
import { detailChips, draftHead, evidenceCount, inferredReason, mapFence, provenanceLine } from '@/lib/codemap/map'
import type { MapApi } from '@/lib/codemap/map-adapter'
import type { MapAnchor, MapEdge, MapElement } from '@/lib/codemap/map-types'
import { cn } from '@/lib/utils'

export type MapDetailProps = {
  element: MapElement | null
  error: string | null
  root: string
  session: string
  /** The map methods the engine offers (`mapMethodsOf`); a missing one disables its action with a tooltip. */
  has: ReadonlySet<keyof MapApi>
  live: boolean
  onDrill(id: string): void
  onJump(id: string): void
  onRegenerate(id: string): void
  onClose?(): void
}

const FIELDS = ['context', 'constraint', 'concern', 'todo', 'note'] as const

function Sec({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="px-3 pt-2">
      <h3 className="mb-1 font-mono text-[9.5px] font-semibold tracking-[0.1em] text-fg-secondary uppercase">{title}</h3>
      {children}
    </section>
  )
}

function Action({ label, title, disabled, primary, onClick }: { label: string; title?: string; disabled?: boolean; primary?: boolean; onClick(): void }) {
  return (
    <button
      type="button"
      className={cn('rounded border border-border-subtle px-2 font-mono text-[11px] leading-5 hover:bg-layer-2 disabled:cursor-not-allowed disabled:opacity-50', primary && 'bg-layer-3')}
      title={title}
      aria-disabled={disabled}
      disabled={disabled}
      onClick={onClick}
    >
      {label}
    </button>
  )
}

/** `rpc → here · 5 Cargo deps · backed`: the element's name stands for itself, the other end by its child name. */
export function EdgeRow({ e, names, self }: { e: MapEdge; names: ReadonlyMap<string, string>; self: string }) {
  const name = (id: string) => (id === self ? 'here' : (names.get(id) ?? id.slice(id.indexOf(':') + 1)))
  const reason = inferredReason(e)
  const count = evidenceCount(e.evidence)
  return (
    <li className="flex flex-wrap items-baseline gap-x-1.5 py-0.5 font-mono text-[11px]" data-edge={e.backed ? 'backed' : 'inferred'}>
      <span>
        {name(e.from)} → {name(e.to)}
      </span>
      {e.label && <span className="text-fg-secondary">· {e.label}</span>}
      {count && <span className="text-fg-secondary">· {count}</span>}
      <span className={cn('shrink-0 rounded border px-1 text-[9px] leading-[13px]', e.backed ? 'border-success text-success' : 'border-dashed border-border text-fg-secondary')} title={reason ?? undefined}>
        {e.backed ? 'backed' : 'inferred'}
      </span>
      {reason && <span className="basis-full text-fg-tertiary">— {reason}</span>}
    </li>
  )
}

export function MapDetail(p: MapDetailProps) {
  const el = p.element
  const ask = (text: string) => {
    if (!el) return
    appendDraft(p.session, text, draftHead(p.root, el.id))
    window.dispatchEvent(new Event('theuth:focus-composer'))
  }
  const copy = (text: string, what: string) => navigator.clipboard.writeText(text).then(() => toast.success(`Copied ${what}`), (e) => toast.error(String(e)))
  const snapshot = () => {
    if (!el) return
    const fence = mapFence(el)
    prefillDraft(p.session, `${draftHead(p.root, el.id)}${fence}\nAdd this to the board's Map section.`)
    void copy(fence, 'a Graph fence for the board (also in the composer)')
    window.dispatchEvent(new Event('theuth:focus-composer'))
  }
  const names = new Map<string, string>()
  if (el) for (const c of el.children) names.set(c.node, c.name)

  return (
    <aside data-map-detail className="flex h-full min-h-0 flex-col overflow-hidden font-sans text-[12px] leading-[1.45]">
      {p.onClose && (
        <button type="button" className="absolute top-1 right-1 rounded px-1.5 font-mono text-[11px] text-fg-secondary hover:bg-layer-2" aria-label="Close detail" onClick={p.onClose}>
          ✕
        </button>
      )}
      {!el ? (
        <p className="p-3 font-mono text-[11px] text-fg-secondary">{p.error ?? 'loading…'}</p>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto pb-2">
          <header className="border-b border-border-subtle px-3 pt-2.5 pb-2">
            {detailChips(el).length > 0 && (
              <div className="mb-1 flex flex-wrap gap-1 font-mono text-[9px] uppercase">
                {detailChips(el).map((c) => (
                  <span key={c} className={cn('rounded border px-1 leading-3', c === 'stale' ? 'codemap-badge stale' : c === 'imported' ? 'codemap-badge imp' : 'border-border-subtle text-fg-secondary')}>
                    {c}
                  </span>
                ))}
              </div>
            )}
            <h2 className="m-0 text-[14px] leading-tight font-semibold">
              {kindGlyph(el.kind)} {el.name}
            </h2>
            {el.path && <div className="font-mono text-[10px] text-fg-secondary">{el.path}</div>}
            {el.anchors.length > 0 && (
              <ul className="mt-1.5 flex flex-wrap gap-1">
                {el.anchors.map((a: MapAnchor) => (
                  <li key={`${a.path}:${a.line_start ?? ''}`}>
                    <button
                      type="button"
                      className={cn('rounded border border-border-subtle bg-layer-2 px-1.5 font-mono text-[10px] leading-4 text-info hover:underline', !a.exists && 'line-through opacity-60')}
                      title={a.exists ? 'Ask about this' : 'path no longer in the index'}
                      onClick={() => ask(`\`${anchorOf(a.path, a.line_start, a.line_end)}\` `)}
                    >
                      {anchorOf(a.path, a.line_start, a.line_end)}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </header>
          {el.fields.description ? (
            <Sec title="Description">
              <p className="m-0">{el.fields.description}</p>
            </Sec>
          ) : (
            <Sec title="Description">
              <p className="m-0 text-fg-secondary">{el.origin === 'structure' ? 'structure view — the model has not described this element yet' : 'no description'}</p>
            </Sec>
          )}
          {FIELDS.map((f) =>
            el.fields[f] ? (
              <Sec key={f} title={f}>
                <p className="m-0">{el.fields[f]}</p>
              </Sec>
            ) : null,
          )}
          {el.children.length > 0 && (
            <Sec title={`Inside · ${el.children.length}`}>
              <p className="m-0 text-fg-secondary">
                {el.children.map((c, i) => (
                  <span key={c.id}>
                    {i > 0 && ' · '}
                    <button type="button" className="hover:underline" onClick={() => (c.leaf ? p.onJump(c.id) : p.onDrill(c.id))}>
                      {c.name}
                    </button>
                  </span>
                ))}{' '}
                <button type="button" className="text-foreground hover:underline" aria-label="Open this element's diagram" onClick={() => p.onDrill(el.id)}>
                  ›
                </button>
              </p>
            </Sec>
          )}
          {el.edges.length > 0 && (
            <Sec title={`Edges · ${el.edges.length}`}>
              <ul className="m-0 list-none p-0">
                {el.edges.map((e) => (
                  <EdgeRow key={`${e.from}>${e.to}`} e={e} names={names} self={el.node} />
                ))}
              </ul>
            </Sec>
          )}
          {el.also_in.length > 0 && (
            <Sec title="Also in">
              <p className="m-0 text-fg-secondary">
                {el.also_in.map((id, i) => (
                  <span key={id}>
                    {i > 0 && ' · '}
                    <button type="button" className="hover:underline" onClick={() => p.onJump(id)}>
                      {id.slice(0, id.indexOf('/'))}
                    </button>
                  </span>
                ))}
              </p>
            </Sec>
          )}
          {el.listing && (el.listing.files.length > 0 || el.listing.symbols.length > 0) && (
            <Sec title={`Files · ${el.listing.files.length}${el.listing.more ? ` (+${el.listing.more})` : ''}`}>
              <ul className="m-0 list-none p-0 font-mono text-[11px]">
                {el.listing.files.map(([path, lines]) => (
                  <li key={path} className="flex gap-2 truncate">
                    <button type="button" className="truncate hover:underline" onClick={() => ask(`\`${path}\` `)}>
                      {path}
                    </button>
                    <span className="text-fg-secondary">{lines}</span>
                  </li>
                ))}
                {el.listing.symbols.map((s) => (
                  <li key={s.moniker} className="flex gap-2 truncate">
                    <button type="button" className="truncate hover:underline" onClick={() => ask(`\`${anchorOf(s.path, s.line, s.line_end)}\` `)}>
                      {kindGlyph(s.kind)} {s.name}
                    </button>
                    <span className="truncate text-fg-secondary">{s.path}</span>
                  </li>
                ))}
              </ul>
            </Sec>
          )}
        </div>
      )}
      {el && (
        <footer className="shrink-0 border-t border-border-subtle">
          <p className="m-0 px-3 py-1.5 font-mono text-[10.5px] text-fg-secondary" data-provenance>
            {provenanceLine(el.origin, el.provenance)}
          </p>
          <div className="flex flex-wrap gap-1 px-3 pb-2">
            <Action label="Explain in chat" primary title={p.live ? 'Append this element to the composer' : 'no live session'} disabled={!p.live} onClick={() => ask(`\`${el.anchors[0]?.path ?? el.path ?? el.id}\` `)} />
            <Action label="Snapshot to board" title="Copy a Graph fence and draft the ask" disabled={!el.diagram && el.children.length === 0} onClick={snapshot} />
            <Action
              label="Regenerate element"
              title={el.origin === 'imported' ? 'imported elements are read-only' : p.has.has('codemap_generate') ? 'Regenerate this subtree in this session' : 'codemap_generate is not available on this engine'}
              disabled={el.origin === 'imported' || !p.has.has('codemap_generate') || !p.live}
              onClick={() => p.onRegenerate(el.id)}
            />
            <Action label="Copy Mermaid" title={el.mermaid ? 'Copy the diagram as Mermaid' : 'this element has no diagram'} disabled={!el.mermaid} onClick={() => el.mermaid && void copy(el.mermaid, 'Mermaid')} />
          </div>
        </footer>
      )}
    </aside>
  )
}
