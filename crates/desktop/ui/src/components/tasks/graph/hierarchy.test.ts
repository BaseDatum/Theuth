// The blank-graph regression and the hierarchy rules: real ELK (the bundled build, no worker) over realistic task
// shapes, the fit padding against React Flow's own viewport maths, and the pure hierarchy helpers.
import { getViewportForBounds } from '@xyflow/react'
import ELK from 'elkjs/lib/elk.bundled.js'
import { describe, expect, it } from 'vitest'
import { flattenElk } from '@/lib/diagram/elk'
import type { Task } from '@/rpc/bindings/method.roster.output'
import { ancestors, childStats, descendants, parentExcluded, resolveAccents, waitingOnChildren } from '../hierarchy'
import { blockerCycles, FIT_PAD, fitPadding, type GraphModel, type GraphOpts, gridLayout, PANEL_W, taskGraph, toElk } from './model'

let clock = 1_762_000_000_000
const uuid = (n: number) => `01a1b316-a5fa-747f-a16e-${String(n).padStart(12, '0')}`
const task = (n: number, over: Partial<Task> = {}): Task => ({
  accent: null,
  attachments: [],
  blockers: [],
  blocks: [],
  brief: '',
  checkout: null,
  created_by: 'agent:hans',
  created_ms: clock++,
  done_when: null,
  external: [],
  harness: null,
  home: null,
  id: uuid(n),
  next_wake_after_ms: null,
  no_progress_count: 0,
  owner: { type: 'agent', name: 'hans' },
  parent: null,
  priority: 'medium',
  project: 'listing',
  resolved_blockers: [],
  reviewer: null,
  schedule: null,
  state: 'todo',
  thread: null,
  title: `task ${n}`,
  updated_ms: 0,
  where: null,
  ...over,
})
const OPTS: GraphOpts = { hideDone: false, parentEdges: true, inheritAccent: false }

/**
 * The owner's planning shape: an epic E3 with a chain of children (each blocked by the one before), the last child
 * added by hand as a blocker of the epic itself, a done child, a decision blocker, and a task of another project.
 */
function epicWithChain(): Task[] {
  const e3 = task(1, { title: 'E3 Verification ships', state: 'blocked' })
  const s1 = task(2, { title: 'E3.1 Capture', parent: e3.id, state: 'done' })
  const s2 = task(3, { title: 'E3.2 Judge', parent: e3.id, blockers: [s1.id], state: 'doing' })
  const s3 = task(4, { title: 'E3.3 Queue', parent: e3.id, blockers: [s2.id, 'decision:01a1b5'] })
  const s4 = task(5, { title: 'E3.4 Badge', parent: s3.id, blockers: [s3.id, uuid(99)] })
  // The blocker the user added: the chain's last child blocks its own parent — legitimate, not a cycle.
  return [{ ...e3, blockers: [s3.id] }, s1, s2, s3, s4]
}

async function layout(m: GraphModel) {
  return flattenElk(await new ELK().layout(toElk(m)))
}

describe('the graph never blanks (regression)', () => {
  it('a child chain whose last child blocks its parent lays out with every node placed', async () => {
    const tasks = epicWithChain()
    for (const opts of [OPTS, { ...OPTS, parentEdges: false }, { ...OPTS, hideDone: true }]) {
      const m = taskGraph(tasks, opts)
      expect(m.cycles).toEqual([])
      const l = await layout(m)
      const placed = new Map(l.nodes.map((n) => [n.id, n]))
      for (const n of m.nodes) {
        const at = placed.get(n.id)
        expect(at, n.id).toBeDefined()
        expect(Number.isFinite(at!.x) && Number.isFinite(at!.y)).toBe(true)
      }
    }
  })

  it('the hierarchy edge and the blocker edge between the same pair are both drawn, the derived wait adds none', () => {
    const tasks = epicWithChain()
    const [e3, , , s3] = tasks
    const m = taskGraph(tasks, OPTS)
    const between = m.edges.filter((e) => [e.source, e.target].sort().join() === [e3!.id, s3!.id].sort().join())
    expect(between.map((e) => e.kind).sort()).toEqual(['blocks', 'parent'])
    // E3 waits on its open children (derived) as well as on s3; no edge is added for it.
    expect(m.nodes.find((n) => n.id === e3!.id)).toMatchObject({ waiting: true, children: { done: 1, total: 3, open: 2 } })
    expect(m.edges.filter((e) => e.target === e3!.id).map((e) => e.kind)).toEqual(['blocks'])
  })

  it('no edge ever names a node the model lacks; ELK would refuse the whole graph for one', async () => {
    const tasks = epicWithChain()
    const s3 = tasks[3]!
    // "Under E3.3": E3.4 stays, its blocker E3.3 stays; E3.2 (blocking E3.3) is outside → a ghost pill, not a dangling edge.
    const m = taskGraph(tasks, OPTS, s3.id)
    const ids = new Set(m.nodes.map((n) => n.id))
    for (const e of m.edges) expect(ids.has(e.source) && ids.has(e.target), e.id).toBe(true)
    expect(m.nodes.filter((n) => n.kind === 'pill').map((n) => n.kind === 'pill' && [n.label, n.ghost])).toEqual([
      ['E3.2 Judge', true],
      ['task 000099', false],
      ['decision 01a1b5', false],
    ])
    // A dangling edge (an older model, a race) is dropped by `toElk` instead of failing the layout.
    const bad: GraphModel = { ...m, edges: [...m.edges, { id: 'b:gone>x', kind: 'blocks', source: 'gone', target: s3.id, open: true }] }
    const raw = { ...toElk(bad), edges: [...toElk(bad).edges!, { id: 'e:99', sources: ['n:gone'], targets: [`n:${s3.id}`] }] }
    await expect(new ELK().layout(raw)).rejects.toThrow(/does not exist/)
    await expect(layout(bad)).resolves.toBeDefined()
  })

  it('the grid stand-in places every node', () => {
    const m = taskGraph(epicWithChain(), OPTS)
    const g = gridLayout(m)
    expect(g.nodes.map((n) => n.id)).toEqual(m.nodes.map((n) => n.id))
    expect(new Set(g.nodes.map((n) => `${n.x},${n.y}`)).size).toBe(m.nodes.length)
  })

  it('a fit with the editor open on a narrow canvas keeps the drawing on screen', () => {
    const bounds = { x: 0, y: 0, width: 1500, height: 600 }
    const onScreen = (pad: { top: number; right: number; bottom: number; left: number }, width: number) => {
      const v = getViewportForBounds(bounds, width, 600, 0.1, 1, { top: `${pad.top}px`, right: `${pad.right}px`, bottom: `${pad.bottom}px`, left: `${pad.left}px` })
      const left = v.x
      const right = v.x + bounds.width * v.zoom
      return right > 0 && left < width
    }
    // What the canvas used to pass: the editor's whole width on the right, however narrow the canvas — every card off
    // the left edge (the empty graph).
    const legacy = { top: FIT_PAD, right: PANEL_W + FIT_PAD, bottom: FIT_PAD, left: FIT_PAD }
    expect(onScreen(legacy, 400)).toBe(false)
    for (const width of [300, 400, 520, 1200]) expect(onScreen(fitPadding(width, true), width), `${width}px`).toBe(true)
    expect(fitPadding(1200, true).right).toBe(PANEL_W + FIT_PAD)
    expect(fitPadding(1200, false).right).toBe(FIT_PAD)
  })
})

describe('blocker cycles', () => {
  it('are found, flagged on their nodes and drawn, never refused', async () => {
    const a = task(10, { blockers: [uuid(12)] })
    const b = task(11, { blockers: [uuid(10), uuid(11)] })
    const c = task(12, { blockers: [uuid(11)] })
    const d = task(13, { blockers: [uuid(12)] })
    expect(blockerCycles([a, b, c, d]).map((cy) => [...cy].sort())).toEqual([[uuid(10), uuid(11), uuid(12)]])
    const m = taskGraph([a, b, c, d], OPTS)
    expect(m.nodes.filter((n) => n.kind === 'task' && n.inCycle).map((n) => n.id).sort()).toEqual([uuid(10), uuid(11), uuid(12)])
    // The self-blocker draws no edge.
    expect(m.edges.some((e) => e.source === e.target)).toBe(false)
    expect((await layout(m)).nodes).toHaveLength(4)
  })
})

describe('edge kinds', () => {
  it('blocks, finished dependency, outside blocker and parent → child are told apart; the toggle hides only the hierarchy', () => {
    const p = task(20)
    const k = task(21, { parent: p.id, blockers: [uuid(22), 'person:sky'], resolved_blockers: [uuid(23)] })
    const ts = [p, k, task(22), task(23, { state: 'done' })]
    const kinds = (opts: GraphOpts) => taskGraph(ts, opts).edges.map((e) => `${e.kind}:${e.open}:${e.source.slice(-2)}>${e.target.slice(-2)}`)
    expect(kinds(OPTS)).toEqual(['blocks:true:22>21', 'blocks:false:23>21', 'parent:false:20>21', 'pill:true:ky>21'])
    expect(kinds({ ...OPTS, parentEdges: false })).toEqual(['blocks:true:22>21', 'blocks:false:23>21', 'pill:true:ky>21'])
  })
})

describe('the under filter', () => {
  it('shows a task and all its descendants, at any depth, cycle-safe', () => {
    const ts = epicWithChain()
    const [e3, s1, s2, s3, s4] = ts
    expect([...descendants(ts, e3!.id)].sort()).toEqual([s1!.id, s2!.id, s3!.id, s4!.id].sort())
    expect(taskGraph(ts, OPTS, s3!.id).nodes.filter((n) => n.kind === 'task').map((n) => n.id)).toEqual([s3!.id, s4!.id])
    // An unknown id shows everything; a parent loop (a replica's) still ends.
    expect(taskGraph(ts, OPTS, 'gone').nodes.filter((n) => n.kind === 'task')).toHaveLength(5)
    const loop = [task(30, { parent: uuid(31) }), task(31, { parent: uuid(30) })]
    expect([...descendants(loop, uuid(30))]).toEqual([uuid(31)])
  })
})

describe('hierarchy helpers', () => {
  it('children progress leaves cancelled ones out; the derived wait reads n of m', () => {
    const ts = [task(40), task(41, { parent: uuid(40), state: 'done' }), task(42, { parent: uuid(40) }), task(43, { parent: uuid(40), state: 'cancelled' })]
    expect(childStats(ts).get(uuid(40))).toEqual({ done: 1, total: 2, open: 1 })
    expect(waitingOnChildren(childStats(ts).get(uuid(40)))).toBe('waiting on 1 of 2 children')
    expect(waitingOnChildren({ done: 2, total: 2, open: 0 })).toBeNull()
  })

  it('ancestors read root first; a task may not take itself or anything under it as parent', () => {
    const ts = epicWithChain()
    const [e3, , , s3, s4] = ts
    expect(ancestors(ts, s4!.id).map((t) => t.title)).toEqual(['E3 Verification ships', 'E3.3 Queue'])
    expect(parentExcluded(ts, s3!.id).sort()).toEqual([s3!.id, s4!.id].sort())
    expect(parentExcluded(ts, e3!.id)).toHaveLength(5)
  })

  it('an accent is the task’s own, or with inherit its nearest ancestor’s, faded', () => {
    const ts = [task(50, { accent: 'lapis' }), task(51, { parent: uuid(50) }), task(52, { parent: uuid(51), accent: 'plum' }), task(53, { parent: uuid(52) }), task(54, { accent: 'teal' })]
    const own = resolveAccents(ts, false)
    expect([...own.entries()].map(([id, a]) => [id.slice(-2), a])).toEqual([
      ['50', { key: 'lapis', inherited: false }],
      ['52', { key: 'plum', inherited: false }],
    ])
    const inherited = resolveAccents(ts, true)
    expect(inherited.get(uuid(51))).toEqual({ key: 'lapis', inherited: true })
    expect(inherited.get(uuid(53))).toEqual({ key: 'plum', inherited: true })
    // An unknown name is no accent; a parent loop ends.
    expect(inherited.has(uuid(54))).toBe(false)
    expect(resolveAccents([task(60, { parent: uuid(61) }), task(61, { parent: uuid(60) })], true).size).toBe(0)
    expect(taskGraph(ts, { ...OPTS, inheritAccent: true }).nodes.find((n) => n.id === uuid(53))).toMatchObject({ accent: { key: 'plum', inherited: true } })
  })
})
