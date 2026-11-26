import { describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { readBoard } from './canvas/model'
import { addCard } from './canvas/ops'
import { TASK_CARD, leavesCard, openTaskBeside, taskGraphPath, viewCardPath, viewCardTitle } from './view-card'

describe('view card paths', () => {
  it('takes the app view routes, query kept', () => {
    expect(viewCardPath(taskGraphPath('theuth web'))).toBe('/tasks/graph?project=theuth%20web')
    expect(viewCardPath(' /tasks ')).toBe('/tasks')
    expect(viewCardPath('/agents/ada')).toBe('/agents/ada')
  })

  it('refuses what is not an app view, and a whiteboard inside a card', () => {
    for (const bad of ['/whiteboards/01J0ABC', '/c/abc', 'https://x.y/tasks', 'tasks', '/tasks/../c/x', '/nope', '', 42]) {
      expect(viewCardPath(bad)).toBeNull()
    }
  })

  it('titles the task graph with its project', () => {
    expect(viewCardTitle(taskGraphPath('theuth'))).toBe('Task graph · theuth')
    expect(viewCardTitle(taskGraphPath(null))).toBe('Task graph')
    expect(viewCardTitle('/agents')).toBe('Agents')
  })

  it('sends chats to the workspace and whiteboards to a pane', () => {
    expect(leavesCard('/c/s%201?x=1')).toEqual({ kind: 'chat', session: 's 1', href: '/c/s%201?x=1' })
    expect(leavesCard('/new')).toEqual({ kind: 'outer', href: '/new' })
    expect(leavesCard('/whiteboards/01J0')).toEqual({ kind: 'pane', href: '/whiteboards/01J0' })
    expect(leavesCard('/tasks/T1')).toEqual({ kind: 'task', id: 'T1', href: '/tasks/T1' })
    expect(leavesCard('/tasks/T%201?tab=x')).toEqual({ kind: 'task', id: 'T 1', href: '/tasks/T%201?tab=x' })
    expect(leavesCard('/tasks/graph?project=a')).toBeNull()
    expect(leavesCard('/tasks')).toBeNull()
  })
})

describe('a task opened from a view card', () => {
  const graph = (doc: Y.Doc) => addCard(doc, { kind: 'view', title: 'Task graph', x: 0, y: 0, w: 560, h: 400, source: { t: 'view', path: taskGraphPath('p') } })

  it('lands right of the card, linked dashed with the task title', () => {
    const doc = new Y.Doc()
    const g = graph(doc)
    const r = openTaskBeside(doc, readBoard(doc), g, 'T1', 'Fix login')
    const b = readBoard(doc)
    expect(r.placed).toBe(true)
    expect(b.cards[r.id]).toMatchObject({ kind: 'view', title: 'Fix login', x: 600, y: 0, w: TASK_CARD[0], h: TASK_CARD[1] })
    expect(b.cards[r.id]!.versions[0]!.source).toEqual({ t: 'view', path: '/tasks/T1' })
    expect(Object.values(b.links)).toEqual([expect.objectContaining({ from: g, to: r.id, label: 'Fix login', style: 'dashed' })])
  })

  it('moves down past a card already there', () => {
    const doc = new Y.Doc()
    const g = graph(doc)
    addCard(doc, { kind: 'sticky', title: 'S', x: 620, y: 10, w: 220, h: 150, source: { t: 'sticky', text: 'x' } })
    const r = openTaskBeside(doc, readBoard(doc), g, 'T1', 'Fix login')
    expect(r.box[1]).toBeGreaterThanOrEqual(160)
  })

  it('reuses the task card that is already on the board, linking it once', () => {
    const doc = new Y.Doc()
    const g = graph(doc)
    const first = openTaskBeside(doc, readBoard(doc), g, 'T1', 'Fix login')
    const again = openTaskBeside(doc, readBoard(doc), g, 'T1', 'Fix login')
    expect(again).toMatchObject({ id: first.id, placed: false })
    const b = readBoard(doc)
    expect(Object.values(b.cards)).toHaveLength(2)
    expect(Object.values(b.links)).toHaveLength(1)
    // From another view card: the same task card, newly linked from that one.
    const board = addCard(doc, { kind: 'view', title: 'Tasks', x: 0, y: 600, w: 560, h: 400, source: { t: 'view', path: '/tasks' } })
    expect(openTaskBeside(doc, readBoard(doc), board, 'T1', 'Fix login').id).toBe(first.id)
    expect(Object.values(readBoard(doc).links)).toHaveLength(2)
  })
})
