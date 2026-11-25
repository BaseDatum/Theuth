// The §3.2 row rule, for the outline view: frames in reading order (y, then x); in each container sort cards by y; a
// card joins the current row when its vertical centre lies within the row's first card's [y, y + h]; rows by top,
// cards in a row by x. Archived cards are not listed.
import type { Board, Card, Frame, Id } from './model'

function rowsIn(cards: Card[]): Id[][] {
  const sorted = [...cards].sort((a, b) => a.y - b.y || a.x - b.x || (a.id < b.id ? -1 : 1))
  const rows: Card[][] = []
  for (const c of sorted) {
    const row = rows[rows.length - 1]
    const head = row?.[0]
    const centre = c.y + c.h / 2
    if (row && head && centre >= head.y && centre <= head.y + head.h) row.push(c)
    else rows.push([c])
  }
  return rows.map((r) => r.sort((a, b) => a.x - b.x || (a.id < b.id ? -1 : 1)).map((c) => c.id))
}

export function rowsOf(board: Board): { frames: { frame: Frame; rows: Id[][] }[]; loose: Id[][] } {
  const frames = Object.values(board.frames).sort((a, b) => a.y - b.y || a.x - b.x || (a.id < b.id ? -1 : 1))
  const members = new Map<Id, Card[]>(frames.map((f) => [f.id, []]))
  const loose: Card[] = []
  for (const c of Object.values(board.cards)) {
    if (c.state === 'archived') continue
    const list = c.frame ? members.get(c.frame) : undefined
    if (list) list.push(c)
    else loose.push(c)
  }
  return { frames: frames.map((frame) => ({ frame, rows: rowsIn(members.get(frame.id)!) })), loose: rowsIn(loose) }
}
