// The `view` card (a live app route inside a card): which paths it takes, its title, where inner navigation leaves
// the card, and the task card a task opened from it becomes (placed beside it, linked). The engine checks the same
// paths on `wb_place`/`wb_update` (`document::wb::engine::view_path`).

import type * as Y from 'yjs'
import { availableViews, validateViewPath } from '@/lib/view-catalog'
import { viewTitle } from '@/lib/view-title'
import { type Board, type Id, versionOf } from './canvas/model'
import { addCard, addLink } from './canvas/ops'

/** `/whiteboards/<id>` is refused: a whiteboard inside one of its own cards would hold itself. */
const NESTED_BOARD = /^\/whiteboards\/[^/?#]+/

/** The path a `view` card shows, normalized, or null when it is not one of the app's view routes. */
export function viewCardPath(raw: unknown): string | null {
  if (typeof raw !== 'string' || raw.length > 512) return null
  const v = validateViewPath(raw)
  return v.ok && !NESTED_BOARD.test(v.normalized) ? v.normalized : null
}

/** The task graph of `project` (`null`: every project). */
export const taskGraphPath = (project: string | null): string => `/tasks/graph?project=${encodeURIComponent(project ?? '')}`

/** The route's title for the card header: the catalog's name, the task graph with its project. */
export function viewCardTitle(path: string): string {
  const q = path.search(/[?#]/)
  const pathname = q >= 0 ? path.slice(0, q) : path
  if (pathname === '/tasks/graph') {
    const project = q >= 0 ? new URLSearchParams(path.slice(q).replace(/#.*$/, '')).get('project') : null
    return project ? `Task graph · ${project}` : 'Task graph'
  }
  return availableViews().find((v) => v.path === pathname)?.title ?? viewTitle(path)
}

/** `/tasks/<id>` (not the graph): a task's detail. */
const TASK = /^\/tasks\/([^/?#]+)\/?(?:[?#]|$)/

/**
 * Where a navigation inside the card goes instead of the card: a chat (`/c/<id>`) or a new chat to the workspace, a
 * whiteboard to a pane (never nested in the card), a task's detail to its own card beside this one (on the canvas).
 * `null`: it stays in the card.
 */
export function leavesCard(
  href: string,
): { kind: 'chat'; session: string; href: string } | { kind: 'outer'; href: string } | { kind: 'pane'; href: string } | { kind: 'task'; id: string; href: string } | null {
  const chat = /^\/c\/([^/?#]+)/.exec(href)
  if (chat?.[1]) return { kind: 'chat', session: decodeURIComponent(chat[1]), href }
  if (href === '/' || /^\/new(?:$|[?#/])/.test(href)) return { kind: 'outer', href }
  if (NESTED_BOARD.test(href)) return { kind: 'pane', href }
  const task = TASK.exec(href)
  if (task?.[1] && task[1] !== 'graph') return { kind: 'task', id: decodeURIComponent(task[1]), href }
  return null
}

export const TASK_CARD: [number, number] = [440, 520]
const GAP = 40

const overlaps = (a: [number, number, number, number], b: [number, number, number, number]) =>
  a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]

/**
 * Open task `task` from view card `from` on the whiteboard: its existing `/tasks/<id>` card (linked from `from` if it
 * is not yet), else a new view card right of `from` — moved down past whatever is there — with a dashed link from
 * `from` labelled with the task's title. Returns the task card and its box (to focus it).
 */
export function openTaskBeside(doc: Y.Doc, board: Board, from: Id, task: string, title: string): { id: Id; placed: boolean; box: [number, number, number, number] } {
  const path = `/tasks/${encodeURIComponent(task)}`
  const linked = (to: Id) => Object.values(board.links).some((l) => (l.from === from && l.to === to) || (l.from === to && l.to === from))
  const link = (to: Id) => addLink(doc, { from, to, label: title, style: 'dashed', arrow: 'end' })
  const cards = Object.values(board.cards).filter((c) => c.state !== 'archived')
  const have = cards.find((c) => {
    const s = versionOf(c)?.source
    return c.kind === 'view' && s?.t === 'view' && viewCardPath(s.path) === path
  })
  if (have) {
    if (have.id !== from && !linked(have.id)) link(have.id)
    return { id: have.id, placed: false, box: [have.x, have.y, have.w, have.h] }
  }
  const src = board.cards[from]
  const [w, h] = TASK_CARD
  const box: [number, number, number, number] = [src ? src.x + src.w + GAP : 0, src ? src.y : 0, w, h]
  for (let i = 0; i < 40 && cards.some((c) => overlaps(box, [c.x, c.y, c.w, c.h])); i++) box[1] += h + GAP / 2
  const id = addCard(doc, { kind: 'view', title: title.slice(0, 120), x: box[0], y: box[1], w, h, source: { t: 'view', path } })
  link(id)
  return { id, placed: true, box }
}
