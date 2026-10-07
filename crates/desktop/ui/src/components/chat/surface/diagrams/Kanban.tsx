// 123 §4.1 / §5.2 (W3): the `Kanban` renderer stays HTML. Adds `limit` (`n/limit`, `⚠` over), card `state`
// (`⊘` blocked, `✓` done), `rows`/`row`/`cut` (story-map swimlanes on one CSS grid, a release line after `cut`),
// `title`, and the §5.3 unit contract on cards. `registry.tsx` mounts the exported `Kanban` by name.
import { type CSSProperties, Fragment, type ReactNode, useMemo } from 'react'
import type { z } from 'zod'
import { unitLabel } from '@/lib/diagram/a11y'
import { type Units, useUnits } from '@/lib/diagram/units'
import type { catalog } from '../catalog'
import { Tag } from '../Tag'

export type KanbanData = z.infer<(typeof catalog)['data']['components']['Kanban']['props']>
type KanbanCard = KanbanData['columns'][number]['cards'][number]

/** Unit ids in authored order: `card.id`, else `${column}:${card}` indices. */
export function kanbanUnits(props: KanbanData): string[] {
  return props.columns.flatMap((col, i) => col.cards.map((c, j) => c.id ?? `${i}:${j}`))
}

const DANGER = 'var(--status-danger-fg)'

function Card({ card, id, u }: { card: KanbanCard; id: string; u: Units }) {
  const paint = u.paint(id)
  const blocked = card.state === 'blocked'
  const done = card.state === 'done'
  const unit = u.unit(id, unitLabel(card.title, card.detail ?? card.tag, card.state))
  const style: CSSProperties = { ...unit.style }
  if (blocked) style.borderColor = DANGER
  if (paint === 'active') style.outline = '2px solid var(--fg-accent)'
  return (
    <div
      {...unit}
      style={style}
      title={card.detail ?? card.tag}
      data-ref={card.ref}
      aria-description={card.ref}
      className={`flex flex-col items-start gap-1 rounded-lg border border-border-subtle bg-layer-1 p-2${paint === 'muted' ? ' opacity-50' : ''}`}
    >
      <div className={done ? 'text-fg-tertiary line-through' : undefined}>
        {blocked && (
          <span aria-hidden className="mr-1" style={{ color: DANGER }}>
            ⊘
          </span>
        )}
        {done && (
          <span aria-hidden className="mr-1">
            ✓
          </span>
        )}
        {card.title}
      </div>
      {card.detail && <div className="text-xs text-fg-secondary">{card.detail}</div>}
      {card.tag && <Tag text={card.tag} variant="secondary" />}
    </div>
  )
}

function Header({ col }: { col: KanbanData['columns'][number] }) {
  const n = col.cards.length
  const over = col.limit !== undefined && n > col.limit
  return (
    <div className="flex items-baseline justify-between px-1">
      <span className="font-medium">{col.title}</span>
      <span className="text-xs text-fg-tertiary tabular-nums" style={over ? { color: DANGER } : undefined}>
        {col.limit === undefined ? n : `${n}/${col.limit}`}
        {over && (
          <span aria-label="over limit" className="ml-1">
            ⚠
          </span>
        )}
      </span>
    </div>
  )
}

export function Kanban({ props, highlight, onSelect }: { props: KanbanData; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const ids = useMemo(() => kanbanUnits(props), [props])
  // ponytail: `focusUnit` looks for the nearest <svg>, so arrow keys are a no-op in this HTML board; Tab still works.
  const u = useUnits({ ids, highlight, onSelect })
  const cols = props.columns
  const idOf = (i: number, j: number) => cols[i]?.cards[j]?.id ?? `${i}:${j}`

  let board: ReactNode
  if (props.rows) {
    const rows = props.rows
    // a card's band is its row; a missing or out-of-range row (streaming) lands in the trailing unlabelled band
    const bands: { label: string | null; cells: { id: string; card: KanbanCard }[][] }[] = [...rows, null].map((label) => ({ label, cells: cols.map(() => []) }))
    cols.forEach((col, i) => col.cards.forEach((c, j) => bands[c.row !== undefined && c.row < rows.length ? c.row : rows.length]?.cells[i]?.push({ id: idOf(i, j), card: c })))
    const trailing = bands[rows.length]
    if (trailing && trailing.cells.every((cell) => cell.length === 0)) bands.pop()
    const span = { gridColumn: `1 / span ${cols.length}` }
    board = (
      <div className="grid gap-x-3 gap-y-2" style={{ gridTemplateColumns: `repeat(${cols.length}, minmax(12rem, 1fr))` }}>
        {cols.map((col, i) => (
          <div key={i} className="rounded-xl bg-layer-2 p-2">
            <Header col={col} />
          </div>
        ))}
        {bands.map((band, r) => (
          <Fragment key={r}>
            <div className="text-xs text-fg-secondary" style={span} data-band={band.label ?? ''}>
              {band.label}
            </div>
            {band.cells.map((cell, i) => (
              <div key={i} className="flex flex-col gap-2 rounded-xl bg-layer-2 p-2">
                {cell.map(({ id, card }) => (
                  <Card key={id} card={card} id={id} u={u} />
                ))}
              </div>
            ))}
            {props.cut === r && <div role="separator" aria-label="release cut" data-cut className="border-t-2 border-dashed border-border-strong" style={span} />}
          </Fragment>
        ))}
      </div>
    )
  } else {
    board = (
      <div className="flex gap-3 overflow-x-auto">
        {cols.map((col, i) => (
          <div key={i} className="flex min-w-48 flex-1 flex-col gap-2 rounded-xl bg-layer-2 p-2">
            <Header col={col} />
            {col.cards.map((c, j) => (
              <Card key={j} card={c} id={idOf(i, j)} u={u} />
            ))}
          </div>
        ))}
      </div>
    )
  }

  return (
    <div {...u.root} className="flex flex-col gap-2 overflow-x-auto text-sm">
      {props.title && <div className="font-medium">{props.title}</div>}
      {board}
    </div>
  )
}
