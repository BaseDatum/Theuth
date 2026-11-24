import { describe, expect, it } from 'vitest'
import type { Task } from '@/rpc/bindings/method.roster.output'
import { graphPopout, searchProject, taskGraph, toElk } from './model'

let clock = 1
const task = (id: string, over: Partial<Task> = {}): Task => ({
  attachments: [],
  blockers: [],
  blocks: [],
  brief: '',
  checkout: null,
  created_by: 'user',
  created_ms: clock++,
  done_when: null,
  external: [],
  harness: null,
  home: null,
  id,
  next_wake_after_ms: null,
  no_progress_count: 0,
  owner: { type: 'user' },
  parent: null,
  priority: 'medium',
  project: 'p',
  reviewer: null,
  schedule: null,
  state: 'todo',
  thread: null,
  title: id,
  updated_ms: 0,
  where: null,
  ...over,
})

const edgeIds = (ts: Task[], opts = { hideDone: false, parentEdges: false }) => taskGraph(ts, opts).edges.map((e) => `${e.kind}:${e.source}>${e.target}`)

describe('taskGraph', () => {
  it('draws B → A when A is blocked by B, open while B is not done', () => {
    const g = taskGraph([task('a', { blockers: ['b', 'c'] }), task('b'), task('c', { state: 'done' })])
    expect(g.edges.map((e) => [e.source, e.target, e.open])).toEqual([
      ['b', 'a', true],
      ['c', 'a', false],
    ])
    expect(g.nodes.find((n) => n.id === 'a')).toMatchObject({ kind: 'task', waiting: true })
  })

  it('a task whose task blockers are all done is not waiting', () => {
    const g = taskGraph([task('a', { blockers: ['c'] }), task('c', { state: 'done' })])
    expect(g.nodes.find((n) => n.id === 'a')).toMatchObject({ waiting: false })
  })

  it('turns decision, person and foreign-task blockers into one shared pill each', () => {
    const g = taskGraph([task('a', { blockers: ['decision:d1', 'person:sky', 'zz-other-project'] }), task('b', { blockers: ['person:sky'] })])
    const pills = g.nodes.filter((n) => n.kind === 'pill')
    expect(pills.map((p) => p.kind === 'pill' && [p.blocker.kind, p.label])).toEqual([
      ['decision', 'decision d1'],
      ['person', 'sky'],
      ['task', 'task roject'],
    ])
    expect(g.edges.filter((e) => e.source === 'x:person:sky').map((e) => e.target)).toEqual(['a', 'b'])
    expect(g.nodes.find((n) => n.id === 'b')).toMatchObject({ waiting: true })
  })

  it('survives cycles, self-blockers and duplicate blockers', () => {
    const ts = [task('a', { blockers: ['b', 'b', 'a'] }), task('b', { blockers: ['a'] })]
    expect(edgeIds(ts)).toEqual(['blocks:a>b', 'blocks:b>a'])
    const elk = toElk(taskGraph(ts))
    expect(elk.children?.map((c) => c.id)).toEqual(['n:a', 'n:b'])
    expect(elk.edges?.map((e) => [e.sources, e.targets])).toEqual([
      [['n:a'], ['n:b']],
      [['n:b'], ['n:a']],
    ])
  })

  it('hide done drops closed tasks and their edges, no pill for a satisfied blocker', () => {
    const ts = [task('a', { blockers: ['c'] }), task('c', { state: 'done' }), task('d', { state: 'cancelled' })]
    const g = taskGraph(ts, { hideDone: true, parentEdges: false })
    expect(g.nodes.map((n) => n.id)).toEqual(['a'])
    expect(g.edges).toEqual([])
  })

  it('parent edges only with the chip on and only between shown tasks', () => {
    const ts = [task('root'), task('kid', { parent: 'root' }), task('orphan', { parent: 'gone' })]
    expect(edgeIds(ts)).toEqual([])
    expect(edgeIds(ts, { hideDone: false, parentEdges: true })).toEqual(['parent:root>kid'])
  })

  it('the hash ignores states, titles and owners, but not ids or edges', () => {
    const base = [task('a', { blockers: ['b'] }), task('b')]
    const h = taskGraph(base).hash
    const restyled = base.map((t) => ({ ...t, state: 'doing' as const, title: 'renamed', owner: { type: 'agent' as const, name: 'ops' } }))
    expect(taskGraph(restyled).hash).toBe(h)
    expect(taskGraph([...base].reverse()).hash).toBe(h)
    expect(taskGraph([base[0]!, base[1]!, task('c')]).hash).not.toBe(h)
    expect(taskGraph([{ ...base[0]!, blockers: [] }, base[1]!]).hash).not.toBe(h)
  })

  it('orders nodes by creation so a new task lands after its siblings', () => {
    const ts = [task('late', { created_ms: 50 }), task('early', { created_ms: 10 })]
    expect(taskGraph(ts).nodes.map((n) => n.id)).toEqual(['early', 'late'])
  })
})

describe('route helpers', () => {
  it('reads a JSON-decoded project search value as text', () => {
    expect(searchProject('theuth')).toBe('theuth')
    expect(searchProject(2026)).toBe('2026')
    expect(searchProject('')).toBe('')
    expect(searchProject(null)).toBeUndefined()
    expect(searchProject({ a: 1 })).toBeUndefined()
  })

  it('pops one window per project', () => {
    expect(graphPopout('a b')).toEqual({ kind: 'page', path: '/tasks/graph?project=a%20b', key: 'tasks-graph-a b' })
    expect(graphPopout(null)).toEqual({ kind: 'page', path: '/tasks/graph?project=', key: 'tasks-graph-' })
  })
})
