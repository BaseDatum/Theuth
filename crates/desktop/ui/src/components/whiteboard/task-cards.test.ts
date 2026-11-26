import { describe, expect, it, vi } from 'vitest'
import * as Y from 'yjs'

vi.mock('@/components/tiles/open', () => ({ openViewFromCard: vi.fn(() => 'tile:view') }))

import { openViewFromCard } from '@/components/tiles/open'
import type { Task } from '@/rpc/bindings/method.roster.output'
import { readBoard } from './canvas/model'
import { addCard, addLink } from './canvas/ops'
import { createUndo } from './canvas/undo'
import {
  TASK_SIZE, addTasks, layoutTasks, openTaskBeside, openTaskPane, planTaskSync, syncTaskCards, taskLayers, taskMoves, taskOf, type TaskLite,
} from './task-cards'
import { taskGraphPath } from './view-card'

const lite = (id: string, blockers: string[] = [], title = `Task ${id}`): TaskLite => ({ id, title, blockers })
const known = (...ts: TaskLite[]) => new Map(ts.map((t) => [t.id, t]))
const taskCard = (doc: Y.Doc, task: string, x = 0) =>
  addCard(doc, { kind: 'task', title: `Task ${task}`, x, y: 0, w: TASK_SIZE[0], h: TASK_SIZE[1], source: { t: 'task', task } })
const spot = () => ({ x: 1000, y: 500 })

describe('layered layout from blockers', () => {
  it('puts every blocker left of the tasks it blocks', () => {
    // a → b → c, a → c, d alone, e blocked by something off the set.
    const tasks = [lite('c', ['b', 'a']), lite('b', ['a']), lite('a'), lite('d'), lite('e', ['decision:x', 'zz'])]
    expect(taskLayers(tasks)).toEqual([['a', 'd', 'e'], ['b'], ['c']])
    const { at, frame } = layoutTasks(tasks, { x: 100, y: 50 })
    expect(at.get('a')!.x).toBeLessThan(at.get('b')!.x)
    expect(at.get('b')!.x).toBeLessThan(at.get('c')!.x)
    expect(at.get('a')!.x).toBe(at.get('d')!.x)
    expect(at.get('d')!.y).toBeGreaterThan(at.get('a')!.y)
    // The frame holds every card.
    for (const p of at.values()) {
      expect(p.x).toBeGreaterThanOrEqual(frame.x)
      expect(p.y).toBeGreaterThanOrEqual(frame.y)
      expect(p.x + TASK_SIZE[0]).toBeLessThanOrEqual(frame.x + frame.w)
      expect(p.y + TASK_SIZE[1]).toBeLessThanOrEqual(frame.y + frame.h)
    }
  })

  it('cuts a blocker cycle instead of looping', () => {
    const layers = taskLayers([lite('a', ['b']), lite('b', ['a']), lite('c', ['c'])])
    expect(layers.flat().sort()).toEqual(['a', 'b', 'c'])
  })
})

describe('blocker-link sync', () => {
  it('adds a blocks link between task cards and removes it when the blocker goes, never touching user links', () => {
    const doc = new Y.Doc()
    const a = taskCard(doc, 'A')
    const b = taskCard(doc, 'B', 400)
    const note = addCard(doc, { kind: 'sticky', title: 'S', x: 0, y: 400, w: 200, h: 150, source: { t: 'sticky', text: 'x' } })
    const user = addLink(doc, { from: a, to: note, label: 'see' })
    const userAB = addLink(doc, { from: b, to: a, label: 'mine' })

    expect(syncTaskCards(doc, readBoard(doc), known(lite('A'), lite('B', ['A'])))).toBe(true)
    let links = Object.values(readBoard(doc).links)
    const synced = links.filter((l) => l.sync === 'task-blocker')
    expect(synced).toEqual([expect.objectContaining({ from: a, to: b, label: 'blocks', style: 'dashed', arrow: 'end' })])
    // Idempotent.
    expect(syncTaskCards(doc, readBoard(doc), known(lite('A'), lite('B', ['A'])))).toBe(false)

    // The blocker is cleared: the sync link goes, the user's links stay.
    syncTaskCards(doc, readBoard(doc), known(lite('A'), lite('B')))
    links = Object.values(readBoard(doc).links)
    expect(links.map((l) => l.id).sort()).toEqual([user, userAB].sort())
  })

  it('removes the links of a deleted task and of an archived card, and keeps one of two duplicates', () => {
    const doc = new Y.Doc()
    const a = taskCard(doc, 'A')
    const b = taskCard(doc, 'B', 400)
    const c = taskCard(doc, 'C', 800)
    syncTaskCards(doc, readBoard(doc), known(lite('A'), lite('B', ['A']), lite('C', ['B'])))
    expect(Object.values(readBoard(doc).links)).toHaveLength(2)
    // Another client wrote the same link meanwhile.
    doc.getMap('links').set('ZZZ', new Y.Map(Object.entries({ from: a, to: b, label: 'blocks', style: 'dashed', arrow: 'end', by: 'owner', sync: 'task-blocker' })))
    // C deleted (absent from the store).
    const p = planTaskSync(readBoard(doc), known(lite('A'), lite('B', ['A'])))
    expect(p.add).toEqual([])
    expect(p.remove).toHaveLength(2)
    syncTaskCards(doc, readBoard(doc), known(lite('A'), lite('B', ['A'])))
    expect(Object.values(readBoard(doc).links).map((l) => [l.from, l.to])).toEqual([[a, b]])
    void c
  })

  it('a pair the user already linked gets no second link', () => {
    const doc = new Y.Doc()
    const a = taskCard(doc, 'A')
    const b = taskCard(doc, 'B', 400)
    addLink(doc, { from: a, to: b, label: 'first this' })
    expect(planTaskSync(readBoard(doc), known(lite('A'), lite('B', ['A']))).add).toEqual([])
  })

  it('card titles follow their task, outside the undo stack', () => {
    const doc = new Y.Doc()
    const undo = createUndo(doc)
    const a = taskCard(doc, 'A')
    undo.manager.clear()
    syncTaskCards(doc, readBoard(doc), known(lite('A', [], 'Renamed')))
    expect(readBoard(doc).cards[a]!.title).toBe('Renamed')
    expect(undo.canUndo()).toBe(false)
  })
})

describe('add tasks', () => {
  it('places a frame, one card per task and the blocker links — one undo step', () => {
    const doc = new Y.Doc()
    const undo = createUndo(doc)
    const r = addTasks(doc, readBoard(doc), spot, [lite('B', ['A']), lite('A'), lite('C', ['B', 'person:sky'])], 'theuth')!
    const b = readBoard(doc)
    expect(b.frames[r.frame]).toMatchObject({ title: 'theuth', x: 1000, y: 500 })
    expect(r.cards).toHaveLength(3)
    const byTask = Object.fromEntries(r.cards.map((id) => [taskOf(b.cards[id]!), b.cards[id]!]))
    for (const c of Object.values(byTask)) expect(c).toMatchObject({ kind: 'task', frame: r.frame, fit: true })
    expect(byTask.A!.x).toBeLessThan(byTask.B!.x)
    expect(byTask.B!.x).toBeLessThan(byTask.C!.x)
    const links = Object.values(b.links).map((l) => [l.from, l.to, l.sync, l.label])
    expect(links).toHaveLength(2)
    expect(links).toEqual(expect.arrayContaining([
      [byTask.A!.id, byTask.B!.id, 'task-blocker', 'blocks'],
      [byTask.B!.id, byTask.C!.id, 'task-blocker', 'blocks'],
    ]))
    // The sync agrees with what was placed.
    expect(planTaskSync(b, known(lite('A'), lite('B', ['A']), lite('C', ['B'])))).toEqual({ add: [], remove: [], retitle: [] })

    undo.undo()
    const after = readBoard(doc)
    expect([Object.keys(after.cards), Object.keys(after.frames), Object.keys(after.links)]).toEqual([[], [], []])
    undo.redo()
    expect(Object.keys(readBoard(doc).cards)).toHaveLength(3)
  })

  it('skips tasks already on the board, linking the new ones to them', () => {
    const doc = new Y.Doc()
    const a = taskCard(doc, 'A')
    const r = addTasks(doc, readBoard(doc), spot, [lite('A'), lite('B', ['A'])], 'Tasks')!
    expect(r.cards).toHaveLength(1)
    const b = readBoard(doc)
    expect(Object.values(b.links).map((l) => [l.from, l.to])).toEqual([[a, r.cards[0]]])
    expect(addTasks(doc, b, spot, [lite('A'), lite('B')], 'Tasks')).toBeNull()
  })
})

describe('a task opened from a view card', () => {
  const graph = (doc: Y.Doc) => addCard(doc, { kind: 'view', title: 'Task graph', x: 0, y: 0, w: 560, h: 400, source: { t: 'view', path: taskGraphPath('p') } })

  it('lands right of the card as a task card, linked dashed with the task title', () => {
    const doc = new Y.Doc()
    const g = graph(doc)
    const r = openTaskBeside(doc, readBoard(doc), g, 'T1', 'Fix login')
    const b = readBoard(doc)
    expect(r.placed).toBe(true)
    expect(b.cards[r.id]).toMatchObject({ kind: 'task', title: 'Fix login', x: 600, y: 0, w: TASK_SIZE[0], h: TASK_SIZE[1] })
    expect(b.cards[r.id]!.versions[0]!.source).toEqual({ t: 'task', task: 'T1' })
    expect(Object.values(b.links)).toEqual([expect.objectContaining({ from: g, to: r.id, label: 'Fix login', style: 'dashed' })])
  })

  it('moves down past a card already there', () => {
    const doc = new Y.Doc()
    const g = graph(doc)
    addCard(doc, { kind: 'sticky', title: 'S', x: 620, y: 10, w: 220, h: 150, source: { t: 'sticky', text: 'x' } })
    expect(openTaskBeside(doc, readBoard(doc), g, 'T1', 'Fix login').box[1]).toBeGreaterThanOrEqual(160)
  })

  it('reuses the task card already on the board, linking it once', () => {
    const doc = new Y.Doc()
    const g = graph(doc)
    const first = openTaskBeside(doc, readBoard(doc), g, 'T1', 'Fix login')
    expect(openTaskBeside(doc, readBoard(doc), g, 'T1', 'Fix login')).toMatchObject({ id: first.id, placed: false })
    expect(Object.values(readBoard(doc).links)).toHaveLength(1)
    const board = addCard(doc, { kind: 'view', title: 'Tasks', x: 0, y: 600, w: 560, h: 400, source: { t: 'view', path: '/tasks' } })
    expect(openTaskBeside(doc, readBoard(doc), board, 'T1', 'Fix login').id).toBe(first.id)
    expect(Object.values(readBoard(doc).links)).toHaveLength(2)
  })
})

describe('task card actions', () => {
  it('opens the task detail through the shared view-tile opener', () => {
    const navigate = vi.fn()
    expect(openTaskPane('T 1', 'Fix login', navigate)).toBe('tile:view')
    expect(openViewFromCard).toHaveBeenCalledWith({ path: '/tasks/T%201', title: 'Fix login' }, { navigate })
  })

  it("offers the detail page's legal moves only", () => {
    const t = (state: Task['state'], extra: Partial<Task> = {}) => ({ state, blockers: [], owner: { type: 'user' }, reviewer: null, ...extra }) as unknown as Task
    expect(taskMoves(t('backlog')).map((m) => m.to)).toEqual(['todo'])
    expect(taskMoves(t('todo')).map((m) => m.to)).toEqual(['backlog'])
    expect(taskMoves(t('blocked', { blockers: ['decision:1'] }))).toEqual([])
    expect(taskMoves(t('blocked', { blockers: ['engine:down'] })).map((m) => m.label)).toEqual(['Unblock'])
    expect(taskMoves(t('review')).map((m) => [m.to, m.done ?? false])).toEqual([['doing', false], ['done', true]])
    expect(taskMoves(t('review', { owner: { type: 'agent', name: 'ops' }, reviewer: { type: 'agent', name: 'boss' } })).map((m) => m.to)).toEqual(['doing'])
    expect(taskMoves(t('done'))).toEqual([])
  })
})
