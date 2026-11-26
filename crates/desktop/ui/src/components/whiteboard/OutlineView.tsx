// 218 §8.8 / design §4.7: the whiteboard as a list — frames → rows → cards, then links and open marks. The accessible
// representation of the canvas (Tab order by frame and row; every cluster action reachable from here) and the phone's
// default view. The same order and aliases the agent reads in its `<whiteboard>` block.

import type { ReactNode } from 'react'
import type { Board, Card, Id, Mark } from './canvas/model'
import { rowsOf } from './canvas/rows'

export const KIND_LABEL: Record<Card['kind'], string> = {
  page: 'page',
  diagram: 'diagram',
  note: 'note',
  excerpt: 'excerpt',
  diff: 'diff',
  terminal: 'terminal',
  web: 'web page',
  image: 'image',
  sticky: 'sticky',
  session: 'session',
  view: 'app view',
}

const VERSIONED = new Set<Card['kind']>(['page', 'excerpt', 'diff', 'terminal', 'web', 'image', 'session', 'view'])

/** One card's spoken label: alias, title, kind, version, state, tag. */
export function cardLabel(card: Card, alias: string | undefined): string {
  const parts = [alias, card.title || 'Untitled', KIND_LABEL[card.kind]].filter(Boolean) as string[]
  if (VERSIONED.has(card.kind) && card.versions.length > 1) parts.push(`version ${card.versions.length} of ${card.versions.length}`)
  if (card.placeholder !== undefined) parts.push(`being made: ${card.placeholder}`)
  else if (card.state === 'dimmed') parts.push(card.tag ? `dimmed — ${card.tag}` : 'dimmed')
  return parts.join(' · ')
}

function markLabel(mark: Mark, board: Board): string {
  const on = mark.card ? board.aliases[mark.card] ?? board.cards[mark.card]?.title ?? 'a card' : 'the canvas'
  const parts = [board.aliases[mark.id], `on ${on}`, mark.state]
  if (mark.region?.label) parts.push(`page says: ${mark.region.label}`)
  if (mark.hw) parts.push(`handwriting: ${mark.hw}`)
  return parts.filter(Boolean).join(' · ')
}

export type OutlineActions = {
  onOpen(card: Id): void
  /** Show it on the canvas (switches to the canvas, focused on the item). */
  onShow?(id: Id): void
  onDim?(card: Id): void
  onUndim?(card: Id): void
  onArchive?(card: Id): void
  onRestore?(card: Id): void
  onPin?(mark: Id, pinned: boolean): void
  /** The cluster's actions, rendered as a toolbar at the top. */
  toolbar?: ReactNode
}

const btn = 'rounded-md px-2 py-1 text-xs text-fg-secondary hover:bg-layer-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-fg-accent'

function CardRow({ card, board, a }: { card: Card; board: Board; a: OutlineActions }) {
  const alias = board.aliases[card.id]
  return (
    <li className="flex items-center gap-1">
      <button type="button" className="min-w-0 flex-1 truncate rounded-md px-2 py-1.5 text-left text-sm hover:bg-layer-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-fg-accent" data-card={card.id} onClick={() => a.onOpen(card.id)}>
        {alias && <span className="mr-2 font-mono text-xs text-fg-tertiary">{alias}</span>}
        <span className={card.state === 'dimmed' ? 'text-fg-tertiary' : 'text-fg-primary'}>{card.title || 'Untitled'}</span>
        <span className="ml-2 text-xs text-fg-tertiary">
          {KIND_LABEL[card.kind]}
          {VERSIONED.has(card.kind) && card.versions.length > 1 ? ` · v${card.versions.length}` : ''}
          {card.placeholder !== undefined ? ' · being made' : card.state === 'dimmed' ? ` · dimmed${card.tag ? ` — ${card.tag}` : ''}` : ''}
        </span>
        <span className="sr-only">{cardLabel(card, alias)}</span>
      </button>
      {a.onShow && (
        <button type="button" className={btn} aria-label={`Show ${card.title || 'card'} on the whiteboard`} onClick={() => a.onShow?.(card.id)}>
          Show
        </button>
      )}
      {card.state === 'dimmed' && a.onUndim && (
        <button type="button" className={btn} onClick={() => a.onUndim?.(card.id)}>
          Undim
        </button>
      )}
      {card.state === 'live' && a.onDim && (
        <button type="button" className={btn} onClick={() => a.onDim?.(card.id)}>
          Dim
        </button>
      )}
      {a.onArchive && (
        <button type="button" className={btn} onClick={() => a.onArchive?.(card.id)}>
          Archive
        </button>
      )}
    </li>
  )
}

function Rows({ rows, board, a }: { rows: Id[][]; board: Board; a: OutlineActions }) {
  return (
    <ol className="flex flex-col gap-2">
      {rows.map((row, i) => (
        <li key={row.join(',')} aria-label={`Row ${i + 1}`}>
          <div className="px-2 pb-0.5 text-[11px] uppercase tracking-wide text-fg-tertiary" aria-hidden>
            Row {i + 1}
          </div>
          <ul className="flex flex-col">
            {row.map((id) => {
              const card = board.cards[id]
              return card ? <CardRow key={id} card={card} board={board} a={a} /> : null
            })}
          </ul>
        </li>
      ))}
    </ol>
  )
}

export function OutlineView({ board, title, a }: { board: Board; title: string; a: OutlineActions }) {
  const { frames, loose } = rowsOf(board)
  const listed = (id: Id) => {
    const c = board.cards[id]
    return c !== undefined && c.state !== 'archived'
  }
  const links = Object.values(board.links)
    .filter((l) => listed(l.from) && listed(l.to))
    .sort((x, y) => (board.aliases[x.id] ?? x.id).localeCompare(board.aliases[y.id] ?? y.id, undefined, { numeric: true }))
  const marks = Object.values(board.marks)
    .filter((m) => m.state === 'sent' || m.state === 'pinned')
    .sort((x, y) => x.sent_at - y.sent_at)
  const archived = Object.values(board.cards).filter((c) => c.state === 'archived')
  const empty = frames.length === 0 && loose.length === 0
  const arrow = { end: '→', both: '↔', none: '—' } as const
  return (
    <div className="mx-auto flex w-full max-w-2xl flex-col gap-5 px-4 pt-[calc(var(--safe-top,0px)+16px)] pb-32" aria-label={`Whiteboard outline: ${title}`}>
      <header className="flex flex-col gap-2">
        <h1 className="text-lg font-semibold text-fg-primary">{title || 'Untitled whiteboard'}</h1>
        {a.toolbar && (
          <div role="toolbar" aria-label="Whiteboard actions" className="flex flex-wrap gap-1">
            {a.toolbar}
          </div>
        )}
      </header>
      {empty && <p className="text-sm text-fg-secondary">This whiteboard has no cards yet. Add a sticky, or ask the agent to place something.</p>}
      {frames.map(({ frame, rows }) => (
        <section key={frame.id} aria-label={`Frame ${frame.title}`} className="flex flex-col gap-1">
          <h2 className="px-2 text-sm font-semibold text-fg-primary">
            {board.aliases[frame.id] && <span className="mr-2 font-mono text-xs text-fg-tertiary">{board.aliases[frame.id]}</span>}
            {frame.title || 'Frame'}
            {frame.preset && <span className="ml-2 text-xs font-normal text-fg-tertiary">{frame.preset}</span>}
          </h2>
          <Rows rows={rows} board={board} a={a} />
        </section>
      ))}
      {loose.length > 0 && (
        <section aria-label="Cards outside frames" className="flex flex-col gap-1">
          {frames.length > 0 && <h2 className="px-2 text-sm font-semibold text-fg-primary">Outside frames</h2>}
          <Rows rows={loose} board={board} a={a} />
        </section>
      )}
      {links.length > 0 && (
        <section aria-label="Links" className="flex flex-col gap-1">
          <h2 className="px-2 text-sm font-semibold text-fg-primary">Links</h2>
          <ul className="flex flex-col">
            {links.map((l) => {
              const from = board.cards[l.from]
              const to = board.cards[l.to]
              return (
                <li key={l.id} className="px-2 py-1 text-sm text-fg-secondary">
                  {board.aliases[l.id] && <span className="mr-2 font-mono text-xs text-fg-tertiary">{board.aliases[l.id]}</span>}
                  {from?.title || 'Untitled'} {arrow[l.arrow]} {to?.title || 'Untitled'}
                  {l.label ? <span className="text-fg-tertiary"> “{l.label}”</span> : null}
                </li>
              )
            })}
          </ul>
        </section>
      )}
      {marks.length > 0 && (
        <section aria-label="Marks" className="flex flex-col gap-1">
          <h2 className="px-2 text-sm font-semibold text-fg-primary">Marks</h2>
          <ul className="flex flex-col">
            {marks.map((m) => (
              <li key={m.id} className="flex items-center gap-1 px-2 py-1 text-sm text-fg-secondary">
                <span className="min-w-0 flex-1 truncate">{markLabel(m, board)}</span>
                {a.onPin && (
                  <button type="button" className={btn} onClick={() => a.onPin?.(m.id, m.state !== 'pinned')}>
                    {m.state === 'pinned' ? 'Unpin' : 'Pin'}
                  </button>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
      {archived.length > 0 && a.onRestore && (
        <section aria-label="Archived cards" className="flex flex-col gap-1">
          <h2 className="px-2 text-sm font-semibold text-fg-primary">Archived</h2>
          <ul className="flex flex-col">
            {archived.map((c) => (
              <li key={c.id} className="flex items-center gap-1 px-2 py-1 text-sm text-fg-tertiary">
                <span className="min-w-0 flex-1 truncate">{cardLabel(c, board.aliases[c.id])}</span>
                <button type="button" className={btn} onClick={() => a.onRestore?.(c.id)}>
                  Restore
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  )
}

/** The outline as plain text (⋯ → Copy outline): the same order, one line per item. */
export function outlineText(board: Board, title: string): string {
  const { frames, loose } = rowsOf(board)
  const line = (id: Id) => {
    const c = board.cards[id]
    return c ? `  - ${cardLabel(c, board.aliases[id])}` : null
  }
  const out = [`# ${title || 'Untitled whiteboard'}`]
  for (const { frame, rows } of frames) {
    out.push(`## ${frame.title || 'Frame'}`)
    for (const r of rows) for (const id of r) out.push(line(id) ?? '')
  }
  if (loose.length) {
    if (frames.length) out.push('## Outside frames')
    for (const r of loose) for (const id of r) out.push(line(id) ?? '')
  }
  return out.filter(Boolean).join('\n')
}
