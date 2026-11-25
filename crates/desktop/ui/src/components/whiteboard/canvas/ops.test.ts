import { describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { MAX_UPDATE_B64 } from '@/lib/doc/provider'
import { pack, type Pt } from '../ink/pack'
import { buildStrokes } from '../ink/stroke'
import { hundredCards, strokes as fillStrokes } from './fixtures/generate'
import { ENGINE, LOCAL, OWNER, readBoard } from './model'
import { addCard, addStrokes, chunked, eraseStrokes, holdGroup, moveItems, patchCard, pinMark, pushVersion, removeFrame, addFrame, setSticky, dim, archive, restore } from './ops'
import { createUndo } from './undo'
import { dragEnd } from './binding'

/** Records every update with its origin. */
function record(doc: Y.Doc) {
  const out: { origin: unknown; bytes: number }[] = []
  doc.on('update', (u: Uint8Array, origin: unknown) => out.push({ origin, bytes: u.length }))
  return out
}
const b64Len = (n: number) => Math.ceil(n / 3) * 4

describe('owner ops (§8.3, §2.9)', () => {
  it('origins: owner edits are wb-owner; snapshots and sends are wb-local', () => {
    const doc = new Y.Doc()
    const log = record(doc)
    const id = addCard(doc, { kind: 'sticky', title: 'T', x: 0, y: 0, w: 220, h: 150, source: { t: 'sticky', text: 'a' } })
    patchCard(doc, id, { x: 10 })
    setSticky(doc, id, 'b')
    expect(log.every((l) => l.origin === OWNER)).toBe(true)
    const b = readBoard(doc)
    expect(b.cards[id]).toMatchObject({ x: 10, kind: 'sticky', state: 'live', by: 'owner' })
    expect(b.cards[id]!.versions).toHaveLength(1) // in place
    expect(b.cards[id]!.versions[0]!.source).toEqual({ t: 'sticky', text: 'b' })
  })

  it('patchCard deletes a key set to undefined (absent, never null)', () => {
    const doc = new Y.Doc()
    const id = addCard(doc, { kind: 'page', title: 'p', x: 0, y: 0, w: 180, h: 390 })
    dim(doc, [id], 'too busy')
    expect(readBoard(doc).cards[id]).toMatchObject({ state: 'dimmed', tag: 'too busy' })
    restore(doc, [id])
    const m = doc.getMap<Y.Map<unknown>>('cards').get(id)!
    expect(m.has('tag')).toBe(false)
    patchCard(doc, id, { frame: 'F' })
    patchCard(doc, id, { frame: undefined })
    expect(m.has('frame')).toBe(false)
    archive(doc, [id])
    expect(readBoard(doc).cards[id]!.state).toBe('archived')
  })

  it('versioned kinds append; a push removes the placeholder', () => {
    const doc = new Y.Doc()
    const id = addCard(doc, { kind: 'page', title: 'p', x: 0, y: 0, w: 180, h: 390 })
    patchCard(doc, id, { placeholder: 'Mock C' })
    pushVersion(doc, id, { t: 'page', session: 's', call_id: 'a' })
    pushVersion(doc, id, { t: 'page', session: 's', call_id: 'b' })
    const c = readBoard(doc).cards[id]!
    expect(c.versions.map((v) => (v.source.t === 'page' ? v.source.call_id : null))).toEqual(['a', 'b'])
    expect(c.placeholder).toBeUndefined()
  })

  it("undo removes the owner's stroke and never an 'engine' write", () => {
    const doc = new Y.Doc()
    const undo = createUndo(doc)
    // The engine places a card (as the provider applies it: origin 'engine').
    const engineDoc = new Y.Doc()
    const eid = addCard(engineDoc, { kind: 'sticky', title: 'agent', x: 500, y: 0, w: 220, h: 150 })
    Y.applyUpdate(doc, Y.encodeStateAsUpdate(engineDoc), ENGINE)
    const pts: Pt[] = Array.from({ length: 30 }, (_, i) => [i * 3, Math.sin(i) * 10, 0.5, i * 8])
    const { strokes } = buildStrokes({ pts, tool: 'pen', colour: 'ink', width: 2.8, hasPressure: true, at: 1 }, 1, [])
    addStrokes(doc, strokes)
    undo.stopCapturing()
    expect(Object.keys(readBoard(doc).ink)).toHaveLength(1)
    undo.undo()
    expect(Object.keys(readBoard(doc).ink)).toHaveLength(0)
    expect(readBoard(doc).cards[eid]).toBeDefined()
    expect(undo.canUndo()).toBe(false) // the engine write is not on the stack
    undo.redo()
    expect(Object.keys(readBoard(doc).ink)).toHaveLength(1)
  })

  it('wb-local bookkeeping never enters the undo stack', () => {
    const doc = new Y.Doc()
    const undo = createUndo(doc)
    doc.transact(() => doc.getMap('marks').set('m', new Y.Map()), LOCAL)
    expect(undo.canUndo()).toBe(false)
  })

  it('chunked: ≤ 60 entry writes per transaction', () => {
    const doc = new Y.Doc()
    const sizes: number[] = []
    let n = 0
    doc.on('afterTransaction', () => {
      sizes.push(n)
      n = 0
    })
    const ink = doc.getMap<unknown>('ink')
    chunked(doc, OWNER, Array.from({ length: 250 }, (_, i) => () => {
      n++
      ink.set(`k${i}`, i)
    }))
    expect(sizes).toEqual([60, 60, 60, 60, 10])
  })

  it('chunked: ≈ 30 kB a transaction for heavy entries', () => {
    const doc = new Y.Doc()
    const log = record(doc)
    const ink = doc.getMap<unknown>('ink')
    chunked(doc, OWNER, Array.from({ length: 20 }, (_, i) => () => ink.set(`k${i}`, new Uint8Array(9000))))
    for (const l of log) expect(l.bytes).toBeLessThan(32_000)
    expect(log.length).toBeGreaterThan(5)
  })

  it('every transaction’s update ≤ 48 KiB base64 for a 4 000-point stroke', () => {
    const doc = new Y.Doc()
    const log = record(doc)
    const pts: Pt[] = Array.from({ length: 9000 }, (_, i) => [Math.cos(i / 30) * (200 + i / 4), Math.sin(i / 30) * (200 + i / 4), 0.5 + 0.4 * Math.sin(i), i * 4])
    const { strokes } = buildStrokes({ pts, tool: 'pen', colour: 'ink', width: 2.8, hasPressure: true, at: 1 }, 1e6 /* no simplification */, [])
    expect(strokes.length).toBeGreaterThan(2)
    addStrokes(doc, strokes)
    expect(log).toHaveLength(strokes.length) // one stroke per transaction
    for (const l of log) expect(b64Len(l.bytes)).toBeLessThanOrEqual(MAX_UPDATE_B64)
    expect(pack([0, 0], pts.slice(0, 4000)).length).toBe(24_000)
  })

  it('bulk fixture writes and lasso deletes stay under the cap', () => {
    const doc = new Y.Doc()
    const log = record(doc)
    hundredCards(doc)
    fillStrokes(doc, 400)
    eraseStrokes(doc, Object.keys(readBoard(doc).ink))
    for (const l of log) expect(b64Len(l.bytes)).toBeLessThanOrEqual(MAX_UPDATE_B64)
    expect(Object.keys(readBoard(doc).cards)).toHaveLength(100)
    expect(Object.keys(readBoard(doc).ink)).toHaveLength(0)
  })

  it('frame move writes the frame and its cards in one owner transaction, with riders; removeFrame keeps cards', () => {
    const doc = new Y.Doc()
    const a = addCard(doc, { kind: 'sticky', title: 'a', x: 100, y: 100, w: 100, h: 100 })
    const f = addFrame(doc, { title: 'F', x: 50, y: 50, w: 400, h: 300, cards: [a] })
    const { strokes } = buildStrokes({ pts: [[190, 150, 0.5, 0], [260, 150, 0.5, 8]], tool: 'pen', colour: 'ink', width: 2, hasPressure: false, at: 1 }, 1, [])
    addStrokes(doc, strokes)
    const before = readBoard(doc)
    const log = record(doc)
    moveItems(doc, before, dragEnd(before, [{ id: f, position: { x: 150, y: 90 } }]))
    expect(log).toHaveLength(1)
    const after = readBoard(doc)
    expect(after.frames[f]).toMatchObject({ x: 150, y: 90 })
    expect(after.cards[a]).toMatchObject({ x: 200, y: 140, frame: f })
    expect(after.ink[strokes[0]!.id]!.o).toEqual([290, 190])
    removeFrame(doc, f)
    expect(readBoard(doc).cards[a]!.frame).toBeUndefined()
    expect(readBoard(doc).frames[f]).toBeUndefined()
  })

  it('review M1: undo and redo of a bulk erase replay in chunks under the 48 KiB cap, in one press each', () => {
    const doc = new Y.Doc()
    const undo = createUndo(doc)
    // 60 heavy strokes (4 000 points, 24 kB each): ≈ 1.4 MB of ink, far over one update.
    const ids: string[] = []
    for (let k = 0; k < 60; k++) {
      const pts: Pt[] = Array.from({ length: 4000 }, (_, i) => [k * 50 + Math.cos(i / 7) * 20, Math.sin(i / 7) * 20 + i / 40, 0.5, i * 4])
      const { strokes } = buildStrokes({ pts, tool: 'pen', colour: 'ink', width: 2.8, hasPressure: true, at: k }, 1e6, [])
      addStrokes(doc, strokes)
      ids.push(...strokes.map((s) => s.id))
    }
    const log = record(doc)
    eraseStrokes(doc, ids)
    expect(readBoard(doc).ink).toEqual({})
    const erased = log.length
    undo.undo() // one press
    expect(Object.keys(readBoard(doc).ink).sort()).toEqual([...ids].sort())
    expect(log.length - erased).toBeGreaterThan(1) // replayed chunk by chunk
    undo.redo() // one press
    expect(readBoard(doc).ink).toEqual({})
    undo.undo()
    for (const l of log) expect(b64Len(l.bytes)).toBeLessThanOrEqual(MAX_UPDATE_B64)
    // Undo goes on to the strokes, one drawn stroke a press.
    undo.undo()
    expect(Object.keys(readBoard(doc).ink)).toHaveLength(ids.length - 1)
  })

  it('an eraser drag (a held group) is one undo step, each write still its own bounded transaction', () => {
    const doc = new Y.Doc()
    const undo = createUndo(doc)
    for (let k = 0; k < 5; k++) addStrokes(doc, buildStrokes({ pts: [[k * 30, 0, 0.5, 0], [k * 30 + 10, 5, 0.5, 8]], tool: 'pen', colour: 'ink', width: 2, hasPressure: false, at: k }, 1, []).strokes)
    const ids = Object.keys(readBoard(doc).ink)
    const log = record(doc)
    const release = holdGroup()
    for (const id of ids) eraseStrokes(doc, [id])
    release()
    expect(log).toHaveLength(5)
    undo.undo()
    expect(Object.keys(readBoard(doc).ink)).toHaveLength(5)
    undo.undo()
    expect(Object.keys(readBoard(doc).ink)).toHaveLength(4) // the strokes were drawn one undo step each
  })

  it('pin / unpin a mark', () => {
    const doc = new Y.Doc()
    const m = new Y.Map<unknown>()
    doc.transact(() => {
      m.set('state', 'sent')
      doc.getMap('marks').set('M', m)
    }, LOCAL)
    pinMark(doc, 'M', true)
    expect(m.get('state')).toBe('pinned')
    pinMark(doc, 'M', false)
    expect(m.get('state')).toBe('answered')
  })
})
