// `task` cards: a task (67 TASKS) on the whiteboard by id, drawn live from the task store. This module is the pure
// part: which task a card shows, the layered layout "Add tasks" places (blockers left of what they block), the board
// links that mirror blockers between task cards (marked `sync: 'task-blocker'`, so a user's own links are never
// touched), placing a set of tasks as one undo step, a task opened from a view card, and the moves a card offers.

import * as Y from 'yjs'
import { openViewFromCard } from '@/components/tiles/open'
import type { Task, TaskState } from '@/rpc/bindings/method.roster.output'
import { actionsFor } from '@/components/tasks/helpers'
import { ulid } from './canvas/ids'
import { LOCAL, OWNER, versionOf, type Board, type Card, type Id } from './canvas/model'
import { addCard, addLink, chunked } from './canvas/ops'

export const TASK_SYNC = 'task-blocker' as const
export const BLOCKS_LABEL = 'blocks'
/** A task card's placement size; it is then sized to its content (fit.ts). */
export const TASK_SIZE: [number, number] = [280, 168]
const COL_GAP = 96
const ROW_GAP = 56
const PAD = 40
/** Room for the frame's title above the first row. */
const HEAD = 24

export type TaskLite = Pick<Task, 'id' | 'title' | 'blockers'>

/** The task a card shows, or null when it is not a task card. */
export function taskOf(card: Card): string | null {
  if (card.kind !== 'task') return null
  const s = versionOf(card)?.source
  return s?.t === 'task' ? s.task : null
}

/** The `/tasks/<id>` route of a task (its detail page). */
export const taskPath = (task: string): string => `/tasks/${encodeURIComponent(task)}`

/** Task `task` in a pane beside the whiteboard (on a phone: its page), through the shared view-tile opener. */
export function openTaskPane(task: string, title: string | undefined, navigate?: (href: string) => void): string | null {
  return openViewFromCard({ path: taskPath(task), title: title || undefined }, { navigate })
}

/** Live (not archived) task cards: card id → task id. */
function taskCards(board: Board): Map<Id, string> {
  const out = new Map<Id, string>()
  for (const c of Object.values(board.cards)) {
    if (c.state === 'archived') continue
    const t = taskOf(c)
    if (t) out.set(c.id, t)
  }
  return out
}

/**
 * Layers by blockers within `tasks`: a task's layer is one past the deepest of its blockers in the set (blockers of
 * other tasks, decisions and people do not count); a blocker cycle is cut where it closes. Each layer keeps the input
 * order.
 */
export function taskLayers(tasks: readonly Pick<Task, 'id' | 'blockers'>[]): string[][] {
  const by = new Map(tasks.map((t) => [t.id, t]))
  const depth = new Map<string, number>()
  const onStack = new Set<string>()
  const visit = (id: string): number => {
    const known = depth.get(id)
    if (known !== undefined) return known
    if (onStack.has(id)) return -1
    onStack.add(id)
    let d = 0
    for (const b of by.get(id)?.blockers ?? []) if (b !== id && by.has(b)) d = Math.max(d, visit(b) + 1)
    onStack.delete(id)
    depth.set(id, d)
    return d
  }
  const layers: string[][] = []
  for (const t of tasks) {
    const d = visit(t.id)
    ;(layers[d] ??= []).push(t.id)
  }
  return layers.filter(Boolean)
}

/** Where each task's card goes, `origin` the frame's top-left: a column per layer, left to right; and the frame. */
export function layoutTasks(
  tasks: readonly Pick<Task, 'id' | 'blockers'>[],
  origin: { x: number; y: number },
  size: [number, number] = TASK_SIZE,
): { frame: { x: number; y: number; w: number; h: number }; at: Map<string, { x: number; y: number }> } {
  const layers = taskLayers(tasks)
  const [w, h] = size
  const at = new Map<string, { x: number; y: number }>()
  layers.forEach((col, i) => col.forEach((id, j) => at.set(id, { x: origin.x + PAD + i * (w + COL_GAP), y: origin.y + PAD + HEAD + j * (h + ROW_GAP) })))
  const rows = Math.max(1, ...layers.map((l) => l.length))
  const cols = Math.max(1, layers.length)
  return {
    frame: { x: origin.x, y: origin.y, w: 2 * PAD + cols * w + (cols - 1) * COL_GAP, h: 2 * PAD + HEAD + rows * h + (rows - 1) * ROW_GAP },
    at,
  }
}

export type SyncPlan = {
  add: { from: Id; to: Id }[]
  remove: Id[]
  /** Task cards whose title is behind their task's. */
  retitle: { id: Id; title: string }[]
}

/**
 * The blocker links the board should have: one `blocks` link from each task card to every task card its task blocks
 * (`to.blockers ∋ from`), dashed, `sync: 'task-blocker'`. Only sync links are removed (a blocker gone, a card archived
 * or gone, a duplicate — the lowest id kept, so two clients agree); a pair the user already linked gets none. `tasks`:
 * every task the store knows (a task card whose task is absent is a deleted task: its links go).
 */
export function planTaskSync(board: Board, tasks: ReadonlyMap<string, TaskLite>): SyncPlan {
  const cards = taskCards(board)
  const byTask = new Map<string, Id[]>()
  for (const [card, task] of cards) byTask.set(task, [...(byTask.get(task) ?? []), card])
  const want = new Set<string>()
  for (const [to, task] of cards) {
    for (const b of tasks.get(task)?.blockers ?? []) for (const from of byTask.get(b) ?? []) if (from !== to) want.add(`${from}>${to}`)
  }
  const user = new Set<string>()
  const sync = new Map<string, Id[]>()
  for (const l of Object.values(board.links)) {
    const k = `${l.from}>${l.to}`
    if (l.sync === TASK_SYNC) sync.set(k, [...(sync.get(k) ?? []), l.id])
    else user.add(k)
  }
  const remove: Id[] = []
  for (const [k, ids] of sync) {
    const sorted = [...ids].sort()
    remove.push(...(want.has(k) && !user.has(k) ? sorted.slice(1) : sorted))
  }
  const add: SyncPlan['add'] = []
  for (const k of want) {
    if (sync.has(k) || user.has(k)) continue
    const [from, to] = k.split('>') as [Id, Id]
    add.push({ from, to })
  }
  const retitle: SyncPlan['retitle'] = []
  for (const [id, task] of cards) {
    const t = tasks.get(task)
    const title = t?.title.slice(0, 120)
    if (title && board.cards[id]?.title !== title) retitle.push({ id, title })
  }
  return { add, remove, retitle }
}

const linkValue = (from: Id, to: Id): Y.Map<unknown> => {
  const m = new Y.Map<unknown>()
  for (const [k, v] of Object.entries({ from, to, label: BLOCKS_LABEL, style: 'dashed', arrow: 'end', by: 'owner', sync: TASK_SYNC })) m.set(k, v)
  return m
}

/** Applies `planTaskSync` (bookkeeping, not undoable: the tasks changed, not the board). Returns whether it wrote. */
export function syncTaskCards(doc: Y.Doc, board: Board, tasks: ReadonlyMap<string, TaskLite>): boolean {
  const p = planTaskSync(board, tasks)
  if (!p.add.length && !p.remove.length && !p.retitle.length) return false
  const links = doc.getMap<unknown>('links')
  const cards = doc.getMap<unknown>('cards')
  const ops: (() => void)[] = [
    ...p.remove.map((id) => () => links.delete(id)),
    ...p.add.map((l) => () => links.set(ulid(), linkValue(l.from, l.to))),
    ...p.retitle.map((r) => () => {
      const m = cards.get(r.id)
      if (m instanceof Y.Map) m.set('title', r.title)
    }),
  ]
  chunked(doc, LOCAL, ops)
  return true
}

/**
 * "Add tasks": the tasks not yet on the board as task cards in a new frame titled `title`, laid out by blockers
 * (`layoutTasks`), with a blocker link between every pair of task cards (new ones and those already on the board) —
 * all one undo step. `spot` finds room for the frame. Null when every task is already on the board.
 */
export function addTasks(
  doc: Y.Doc,
  board: Board,
  spot: (w: number, h: number) => { x: number; y: number },
  tasks: readonly TaskLite[],
  title: string,
): { frame: Id; cards: Id[] } | null {
  const have = new Set(taskCards(board).values())
  const fresh = [...new Map(tasks.filter((t) => !have.has(t.id)).map((t) => [t.id, t])).values()]
  if (!fresh.length) return null
  const probe = layoutTasks(fresh, { x: 0, y: 0 })
  const origin = spot(probe.frame.w, probe.frame.h)
  const { frame, at } = layoutTasks(fresh, origin)
  const frameId = ulid()
  const cardOf = new Map<string, Id>()
  let z = Math.max(0, ...Object.values(board.cards).map((c) => c.z))
  const now = Date.now()
  const ops: (() => void)[] = [
    () => {
      const f = new Y.Map<unknown>()
      for (const [k, v] of Object.entries({ title: title.slice(0, 80), ...frame, colour: 'neutral', by: 'owner' })) f.set(k, v)
      doc.getMap<unknown>('frames').set(frameId, f)
    },
  ]
  for (const t of fresh) {
    const id = ulid()
    cardOf.set(t.id, id)
    const p = at.get(t.id) ?? origin
    const zz = ++z
    ops.push(() => {
      const m = new Y.Map<unknown>()
      for (const [k, v] of Object.entries({ kind: 'task', x: p.x, y: p.y, w: TASK_SIZE[0], h: TASK_SIZE[1], z: zz, title: t.title.slice(0, 120), state: 'live', frame: frameId, by: 'owner', fit: true })) m.set(k, v)
      const versions = new Y.Array<unknown>()
      versions.push([{ source: { t: 'task', task: t.id }, at: now, by: 'owner' }])
      m.set('versions', versions)
      doc.getMap<unknown>('cards').set(id, m)
    })
  }
  // The links among every task card once these are placed: the same rule the sync keeps.
  const after: Board = { ...board, cards: { ...board.cards } }
  for (const [task, id] of cardOf) {
    const p = at.get(task) ?? origin
    after.cards[id] = { id, kind: 'task', x: p.x, y: p.y, w: TASK_SIZE[0], h: TASK_SIZE[1], z: 0, title: '', state: 'live', by: 'owner', versions: [{ source: { t: 'task', task }, at: now, by: 'owner' }] }
  }
  // Each new task's blockers that are on the board (new cards or ones already there): the rule the sync keeps. Tasks
  // already on the board that a new one blocks get their links from the sync, which knows their blockers.
  const known = new Map<string, TaskLite>(fresh.map((t) => [t.id, t]))
  for (const l of planTaskSync(after, known).add) ops.push(() => doc.getMap<unknown>('links').set(ulid(), linkValue(l.from, l.to)))
  chunked(doc, OWNER, ops)
  return { frame: frameId, cards: [...cardOf.values()] }
}

export const TASK_CARD_GAP = 40

const overlaps = (a: [number, number, number, number], b: [number, number, number, number]) =>
  a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]

/**
 * Open task `task` from card `from` (a task graph view card): its task card already on the board (linked from `from`
 * if it is not yet), else a new task card right of `from` — moved down past whatever is there — with a dashed link
 * from `from` labelled with the task's title. Returns the task card and its box (to focus it).
 */
export function openTaskBeside(doc: Y.Doc, board: Board, from: Id, task: string, title: string): { id: Id; placed: boolean; box: [number, number, number, number] } {
  const linked = (to: Id) => Object.values(board.links).some((l) => (l.from === from && l.to === to) || (l.from === to && l.to === from))
  const link = (to: Id) => addLink(doc, { from, to, label: title, style: 'dashed', arrow: 'end' })
  const cards = Object.values(board.cards).filter((c) => c.state !== 'archived')
  const have = cards.find((c) => taskOf(c) === task)
  if (have) {
    if (have.id !== from && !linked(have.id)) link(have.id)
    return { id: have.id, placed: false, box: [have.x, have.y, have.w, have.h] }
  }
  const src = board.cards[from]
  const [w, h] = TASK_SIZE
  const box: [number, number, number, number] = [src ? src.x + src.w + TASK_CARD_GAP : 0, src ? src.y : 0, w, h]
  for (let i = 0; i < 40 && cards.some((c) => overlaps(box, [c.x, c.y, c.w, c.h])); i++) box[1] += h + TASK_CARD_GAP / 2
  const id = addCard(doc, { kind: 'task', title: title.slice(0, 120), x: box[0], y: box[1], w, h, source: { t: 'task', task }, fit: true })
  link(id)
  return { id, placed: true, box }
}

/** A move a task card offers: a `task_move` to `to`, or (`done`) closing it as done — the detail page's own gates. */
export type TaskMove = { label: string; to: TaskState; done?: true }

/**
 * The moves the task's detail page allows (`actionsFor`, 67 §7.2): Start, Back to backlog, Unblock, Send back, Mark
 * done (only for its reviewer). Cancel stays on the detail page, behind its confirmation.
 */
export function taskMoves(t: Task): TaskMove[] {
  const a = actionsFor(t)
  const out: TaskMove[] = []
  if (a.start) out.push({ label: 'Start', to: 'todo' })
  if (a.toBacklog) out.push({ label: 'Back to backlog', to: 'backlog' })
  if (a.unblock) out.push({ label: 'Unblock', to: 'todo' })
  if (a.sendBack) out.push({ label: 'Send back', to: 'doing' })
  if (a.canMarkDone) out.push({ label: 'Mark done', to: 'done', done: true })
  return out
}
