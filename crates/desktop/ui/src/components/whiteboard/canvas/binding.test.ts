import { describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { dragEnd, edgesOf, nodesOf, toAbsolute } from './binding'
import { mkBoard, mkCard, mkFrame, mkStroke } from './fixtures/testing'
import { readBoard } from './model'
import { addCard, addFrame, moveItems } from './ops'

describe('the Yjs ↔ React Flow binding (§8.4)', () => {
  const F = mkFrame('F', 100, 100, 600, 400)
  const board = mkBoard({
    frames: [F],
    cards: [mkCard('in', 150, 160, 100, 100, { frame: 'F', z: 3 }), mkCard('out', 900, 100, 100, 100, { z: 1 }), mkCard('arch', 0, 0, 10, 10, { state: 'archived' })],
  })

  it('frames are parent nodes listed before their children; children are parent-relative', () => {
    const nodes = nodesOf(board, { selection: ['in'] })
    expect(nodes.map((n) => n.id)).toEqual(['F', 'out', 'in'])
    const child = nodes.find((n) => n.id === 'in')!
    expect(child.type === 'card' && child.parentId).toBe('F')
    expect(child.position).toEqual({ x: 50, y: 60 })
    expect(child.selected).toBe(true)
    expect(nodes.find((n) => n.id === 'out')!.position).toEqual({ x: 900, y: 100 })
    expect(nodes.find((n) => n.id === 'F')!.zIndex).toBeLessThan(0)
  })

  it('absolute ↔ relative round trip', () => {
    expect(toAbsolute(board, 'in', { x: 50, y: 60 })).toEqual({ x: 150, y: 160 })
    expect(toAbsolute(board, 'out', { x: 5, y: 6 })).toEqual({ x: 5, y: 6 })
  })

  it('drag end: a card dropped out of its frame clears membership, dropped into one joins it', () => {
    const w1 = dragEnd(board, [{ id: 'in', position: { x: 900, y: 600 } }]) // relative → absolute 1000,700: outside F
    expect(w1.cards).toEqual([{ id: 'in', x: 1000, y: 700, frame: null }])
    const w2 = dragEnd(board, [{ id: 'out', position: { x: 300, y: 200 } }])
    expect(w2.cards).toEqual([{ id: 'out', x: 300, y: 200, frame: 'F' }])
    expect(dragEnd(board, [{ id: 'out', position: { x: 900, y: 100 } }]).cards).toEqual([]) // no move, no write
  })

  it('drag end of a frame carries its cards by the same delta (absolute in the doc)', () => {
    const w = dragEnd(board, [{ id: 'F', position: { x: 130, y: 90 } }])
    expect(w.frames).toEqual([{ id: 'F', x: 130, y: 90 }])
    expect(w.cards).toEqual([{ id: 'in', x: 180, y: 150 }])
  })

  it('riders: free strokes touching exactly the moved card come along', () => {
    const b = mkBoard({ cards: [mkCard('A', 0, 0, 100, 100), mkCard('B', 400, 0, 100, 100)], ink: [mkStroke('r', [90, 10, 30, 5]), mkStroke('both', [90, 10, 320, 5])] })
    expect(dragEnd(b, [{ id: 'A', position: { x: 10, y: 0 } }]).riders.map((s) => s.id)).toEqual(['r'])
  })

  it('write on drag end only: nothing reaches the doc until moveItems runs once', () => {
    const doc = new Y.Doc()
    const a = addCard(doc, { kind: 'sticky', title: 'a', x: 0, y: 0, w: 100, h: 100 })
    addFrame(doc, { title: 'F', x: 500, y: 500, w: 400, h: 400 })
    const before = readBoard(doc)
    let updates = 0
    doc.on('update', () => updates++)
    // A drag is many intermediate positions on the React Flow side; the binding computes nothing from them.
    for (let i = 0; i < 30; i++) nodesOf(before, { selection: [a] })
    expect(updates).toBe(0)
    moveItems(doc, before, dragEnd(before, [{ id: a, position: { x: 600, y: 600 } }]))
    expect(updates).toBe(1)
    const after = readBoard(doc)
    expect(after.cards[a]).toMatchObject({ x: 600, y: 600 })
    expect(after.cards[a]!.frame).toBe(Object.keys(after.frames)[0])
    // The re-derived node is relative to its new parent.
    expect(nodesOf(after, { selection: [] }).find((n) => n.id === a)!.position).toEqual({ x: 100, y: 100 })
  })

  it('links render only between drawn cards', () => {
    const b = mkBoard({ cards: [mkCard('A', 0, 0, 1, 1), mkCard('B', 0, 0, 1, 1), mkCard('X', 0, 0, 1, 1, { state: 'archived' })] })
    b.links = {
      l1: { id: 'l1', from: 'A', to: 'B', style: 'solid', arrow: 'end', by: 'owner', from_port: 'right' },
      l2: { id: 'l2', from: 'A', to: 'X', style: 'solid', arrow: 'end', by: 'owner' },
    }
    const e = edgesOf(b, [])
    expect(e.map((x) => [x.id, x.sourceHandle])).toEqual([['l1', 's-right']])
  })
})
