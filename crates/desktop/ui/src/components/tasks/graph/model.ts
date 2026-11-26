// 214 D1/D8: the project task graph as pure data — tasks → nodes and edges, the ELK graph, the structural hash, the
// grid that stands in when ELK fails. No React, no `@xyflow/react`: the canvas (`Canvas.tsx`) only places and draws what
// this returns, and the rules are tested in node (`model.test.ts`, `blank.test.ts`).
import type { ElkNode } from 'elkjs/lib/elk-api.js'
import { ELK_OPTIONS, fnv, type Layout } from '@/lib/diagram/elk'
import type { PopoutTarget } from '@/lib/popout'
import type { Task, TaskState } from '@/rpc/bindings/method.roster.output'
import { type Blocker, parseBlocker, shortId } from '../helpers'
import { type ChildStats, childStats, descendants, resolveAccents, type ShownAccent } from '../hierarchy'

/** The filter chips (D8): hide done, the hierarchy (parent → child) edges, children showing an ancestor's accent. */
export type GraphOpts = { hideDone: boolean; parentEdges: boolean; inheritAccent: boolean }
export const DEFAULT_OPTS: GraphOpts = { hideDone: false, parentEdges: true, inheritAccent: false }

/**
 * A task card, 240 × 84. `waiting`: it has a blocker that is still open (a task not done, or a decision/person) or an
 * open child (derived, never a stored blocker). `children`: its children's progress; `accent`: its own or, with the
 * chip on, an ancestor's (`inherited`); `inCycle`: it sits on a blocker cycle (the warning names it).
 */
export type TaskNodeModel = {
  kind: 'task'
  id: string
  task: Task
  closed: boolean
  waiting: boolean
  children: ChildStats | null
  accent: ShownAccent | null
  inCycle: boolean
}
/** A blocker that is not a task on this graph: `decision:…`, `person:…`, a system phrase, a task of another project,
 * or (`ghost`) a known task the filter leaves out. One pill per distinct blocker text, shared by every task it blocks. */
export type PillNodeModel = { kind: 'pill'; id: string; blocker: Blocker; label: string; ghost: boolean }
export type NodeModel = TaskNodeModel | PillNodeModel
/** `blocks`: source blocks target (D1, `target.blockers ∋ source`); `open` while the source is not done/cancelled.
 * `pill`: a non-task blocker into the task it blocks. `parent`: parent → child, dashed, only with the chip on. */
export type EdgeKind = 'blocks' | 'pill' | 'parent'
export type EdgeModel = { id: string; kind: EdgeKind; source: string; target: string; open: boolean }
/** `cycles`: each blocker cycle among the shown tasks, as task ids in order (drawn, flagged, never fatal). */
export type GraphModel = { nodes: NodeModel[]; edges: EdgeModel[]; hash: string; cycles: string[][] }

export const TASK_W = 240
export const TASK_H = 84
export const PILL_H = 28
/** The inline editor's width (it lies over the canvas's right edge; fitting leaves it this much room). */
export const PANEL_W = 380
/** Room around the drawing when it fits. */
export const FIT_PAD = 32

/** A pill's width from its label: icon + padding + ~7 px a character, clamped. */
export const pillWidth = (label: string): number => Math.min(220, Math.max(88, 40 + label.length * 7))
export const sizeOf = (n: NodeModel): { w: number; h: number } => (n.kind === 'task' ? { w: TASK_W, h: TASK_H } : { w: pillWidth(n.label), h: PILL_H })

const isClosed = (s: TaskState): boolean => s === 'done' || s === 'cancelled'

/** A pill's text: `decision 1a2b3c`, `person sky`, a known task's title, a foreign task's short id, or the phrase. */
function pillLabel(b: Blocker, title?: string): string {
  if (b.kind === 'decision') return `decision ${b.id.length > 10 ? shortId(b.id) : b.id}`
  if (b.kind === 'person') return b.id
  if (b.kind === 'task') return title ? (title.length > 26 ? `${title.slice(0, 25)}…` : title) : `task ${shortId(b.id)}`
  return b.raw
}

/** 214 A1: tasks that used to block `t` and were dropped from its `blockers` when they closed. Read loosely: an older
 * engine never sends it. */
export const resolvedBlockers = (t: Task): readonly string[] => (t as Task & { resolved_blockers?: string[] }).resolved_blockers ?? []

/** The tasks the graph shows with `under` set: that task and everything below it (unknown id: every task). */
export function underScope(tasks: readonly Task[], under: string | null): readonly Task[] {
  if (under === null || !tasks.some((t) => t.id === under)) return tasks
  const keep = descendants(tasks, under).add(under)
  return tasks.filter((t) => keep.has(t.id))
}

/**
 * Every blocker cycle among `tasks` (task blockers only; ids in walk order), by Tarjan's strongly connected components:
 * a component of two or more tasks is a cycle. The engine refuses one, but a mesh replica can still merge one in.
 */
export function blockerCycles(tasks: readonly Pick<Task, 'id' | 'blockers'>[]): string[][] {
  const ids = new Set(tasks.map((t) => t.id))
  const next = new Map(tasks.map((t) => [t.id, t.blockers.filter((b) => b !== t.id && ids.has(b))]))
  const index = new Map<string, number>()
  const low = new Map<string, number>()
  const stack: string[] = []
  const on = new Set<string>()
  const out: string[][] = []
  let n = 0
  // Iterative, so a long chain never overflows the call stack.
  for (const root of ids) {
    if (index.has(root)) continue
    const work: [string, number][] = [[root, 0]]
    while (work.length) {
      const top = work[work.length - 1]!
      const [v, i] = top
      if (i === 0) {
        index.set(v, n)
        low.set(v, n++)
        stack.push(v)
        on.add(v)
      }
      const succ = next.get(v)!
      if (i < succ.length) {
        top[1]++
        const w = succ[i]!
        if (!index.has(w)) work.push([w, 0])
        else if (on.has(w)) low.set(v, Math.min(low.get(v)!, index.get(w)!))
        continue
      }
      work.pop()
      const parent = work[work.length - 1]
      if (parent) low.set(parent[0], Math.min(low.get(parent[0])!, low.get(v)!))
      if (low.get(v) === index.get(v)) {
        const comp: string[] = []
        let w: string
        do {
          w = stack.pop()!
          on.delete(w)
          comp.push(w)
        } while (w !== v)
        if (comp.length > 1) out.push(comp.reverse())
      }
    }
  }
  return out
}

/**
 * The graph of `tasks` (one project's, or every task for "All projects"), optionally only the subtree `under` a task.
 * Node order is creation order (ELK keeps model order, so a new task lands after its siblings); edges are deduplicated
 * and a task blocking itself is ignored. An edge never names a node the model lacks: a blocker the filter leaves out is
 * a ghost pill while it is open and nothing once it is done, a parent outside the view draws no edge. Cycles are kept
 * (ELK's layered algorithm breaks them) and reported in `cycles`. The hash covers node ids, pill sizes and edges only —
 * never states, titles, owners or accents — so a state change never moves a node. A finished dependency (A1
 * `resolved_blockers`) keeps its edge, faded, under the same id as while it blocked: a blocker resolving never relayouts.
 */
export function taskGraph(tasks: readonly Task[], opts: GraphOpts = DEFAULT_OPTS, under: string | null = null): GraphModel {
  const all = new Map(tasks.map((t) => [t.id, t]))
  const shown = underScope(tasks, under)
    .filter((t) => !(opts.hideDone && isClosed(t.state)))
    .sort((a, b) => a.created_ms - b.created_ms || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
  const visible = new Set(shown.map((t) => t.id))
  const kids = childStats(tasks)
  const accents = resolveAccents(tasks, opts.inheritAccent)
  const cycles = blockerCycles(shown)
  const looped = new Set(cycles.flat())
  const nodes: NodeModel[] = []
  const pills = new Map<string, PillNodeModel>()
  const edges = new Map<string, EdgeModel>()
  const pill = (raw: string, b: Blocker, target: string, title?: string) => {
    const pid = `x:${raw}`
    if (!pills.has(pid)) pills.set(pid, { kind: 'pill', id: pid, blocker: b, label: pillLabel(b, title), ghost: title !== undefined })
    const id = `p:${raw}>${target}`
    edges.set(id, { id, kind: 'pill', source: pid, target, open: true })
  }
  for (const t of shown) {
    let waiting = false
    for (const raw of t.blockers) {
      const b = parseBlocker(raw)
      const src = b.kind === 'task' ? all.get(b.id) : undefined
      if (src) {
        const open = !isClosed(src.state)
        waiting ||= open
        if (b.id === t.id) continue
        if (visible.has(b.id)) {
          const id = `b:${b.id}>${t.id}`
          edges.set(id, { id, kind: 'blocks', source: b.id, target: t.id, open })
        } else if (open) pill(raw, b, t.id, src.title)
        // A hidden blocker that is done is satisfied: no edge, no pill.
        continue
      }
      waiting = true
      pill(raw, b, t.id)
    }
    for (const id of resolvedBlockers(t)) {
      const eid = `b:${id}>${t.id}`
      // A live blocker wins; a hidden (done) or unknown one draws nothing.
      if (id === t.id || edges.has(eid) || !visible.has(id)) continue
      edges.set(eid, { id: eid, kind: 'blocks', source: id, target: t.id, open: false })
    }
    if (opts.parentEdges && t.parent && t.parent !== t.id && visible.has(t.parent)) {
      const id = `c:${t.parent}>${t.id}`
      edges.set(id, { id, kind: 'parent', source: t.parent, target: t.id, open: false })
    }
    const children = kids.get(t.id) ?? null
    const closed = isClosed(t.state)
    nodes.push({
      kind: 'task',
      id: t.id,
      task: t,
      closed,
      waiting: (waiting || (children?.open ?? 0) > 0) && !closed,
      children,
      accent: accents.get(t.id) ?? null,
      inCycle: looped.has(t.id),
    })
  }
  nodes.push(...[...pills.values()].sort((a, b) => (a.id < b.id ? -1 : 1)))
  // Sorted by id: one hash is one edge order, so a cached layout's routes (`e:<index>`) always match these edges.
  const edgeList = [...edges.values()].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
  return { nodes, edges: edgeList, hash: graphHash(nodes, edgeList), cycles }
}

/** Sorted ids (pills with their width) and sorted edges: the layout's identity. */
export function graphHash(nodes: readonly NodeModel[], edges: readonly EdgeModel[]): string {
  const ids = nodes.map((n) => (n.kind === 'pill' ? `${n.id}#${pillWidth(n.label)}` : n.id)).sort()
  const es = edges.map((e) => e.id).sort()
  return `tg:${nodes.length}:${fnv(`${ids.join('\n')}\n--\n${es.join('\n')}`)}`
}

/** ELK options of the task graph: layered left → right (D8), wider gaps than a chat diagram so the cards breathe. */
export const TASK_ELK = {
  ...ELK_OPTIONS,
  'elk.direction': 'RIGHT',
  'elk.spacing.nodeNode': '28',
  'elk.layered.spacing.nodeNodeBetweenLayers': '72',
  'elk.layered.spacing.edgeNodeBetweenLayers': '24',
}

/**
 * The model as an ELK graph: `n:<id>` nodes in model order, `e:<index>` edges (what `flattenElk` reads back; the index
 * is the model's, so routes still match). ELK's JSON import throws on an edge whose end is not a node ("Referenced
 * shape does not exist"), so such an edge — and a self-loop — is left out here rather than failing the whole layout.
 */
export function toElk(m: GraphModel): ElkNode {
  const ids = new Set(m.nodes.map((n) => n.id))
  return {
    id: 'root',
    layoutOptions: TASK_ELK,
    children: m.nodes.map((n) => {
      const s = sizeOf(n)
      return { id: `n:${n.id}`, width: s.w, height: s.h }
    }),
    edges: m.edges.flatMap((e, i) => (e.source !== e.target && ids.has(e.source) && ids.has(e.target) ? [{ id: `e:${i}`, sources: [`n:${e.source}`], targets: [`n:${e.target}`] }] : [])),
  }
}

/** The stand-in when ELK fails or times out: the nodes in model order on a square-ish grid, no routes (edges then
 * smooth-step). Every node gets a place, so the graph is never blank. */
export function gridLayout(m: GraphModel): Layout {
  const cols = Math.max(1, Math.ceil(Math.sqrt(m.nodes.length)))
  const nodes = m.nodes.map((n, i) => {
    const s = sizeOf(n)
    return { id: n.id, x: (i % cols) * (TASK_W + 72), y: Math.floor(i / cols) * (TASK_H + 40), w: s.w, h: s.h }
  })
  return { nodes, edges: [], groups: [], width: cols * (TASK_W + 72), height: Math.ceil(m.nodes.length / cols) * (TASK_H + 40) }
}

/**
 * The padding a fit leaves around the drawing (px). With the editor open its width is kept clear on the right — but
 * never so much that less than a card's width stays: a padding wider than the canvas makes React Flow's fit push the
 * whole drawing off the left edge (a narrow tile or window then shows an empty canvas).
 */
export function fitPadding(width: number, panelOpen: boolean): { top: number; right: number; bottom: number; left: number } {
  const covered = panelOpen ? Math.min(PANEL_W, Math.max(0, width - TASK_W - 2 * FIT_PAD)) : 0
  return { top: FIT_PAD, right: covered + FIT_PAD, bottom: FIT_PAD, left: FIT_PAD }
}

/** The route's `project` search value: TanStack JSON-decodes search, so `?project=2026` arrives as a number. */
export function searchProject(v: unknown): string | undefined {
  if (typeof v === 'string') return v
  if (typeof v === 'number' || typeof v === 'boolean') return String(v)
  return undefined
}

/** `/tasks/graph` for `project` (`null` = all projects, `''` in the URL like the board). The value is JSON-quoted as
 * the router's `stringifySearch` writes it, so a label like `007` or `1e3` reads back as the same text. */
export const graphPath = (project: string | null): string => `/tasks/graph?project=${encodeURIComponent(JSON.stringify(project ?? ''))}`

/** The graph in its own window (D6): one window per project. The key is a hash of the label: the window label keeps
 * only `[A-Za-z0-9_-]`, so the raw text would let `a b` and `a_b` share a window. */
export const graphPopout = (project: string | null): PopoutTarget => ({ kind: 'page', path: graphPath(project), key: `tasks-graph-${fnv(project ?? '')}` })

/**
 * Every task that waits on `id`, directly or through others (its transitive `blocks`, read from the tasks' `blockers`):
 * adding any of them as a blocker of `id` would close a cycle, so the picker hides them. Cycle-safe.
 */
export function dependents(tasks: readonly Pick<Task, 'id' | 'blockers'>[], id: string): Set<string> {
  const waiting = new Map<string, string[]>()
  for (const t of tasks) for (const b of t.blockers) waiting.set(b, [...(waiting.get(b) ?? []), t.id])
  const out = new Set<string>()
  const stack = [id]
  while (stack.length) {
    for (const next of waiting.get(stack.pop()!) ?? []) {
      if (out.has(next) || next === id) continue
      out.add(next)
      stack.push(next)
    }
  }
  return out
}

/** State badge tone (D8), a theme token so both themes read. */
export const STATE_TONE: Record<TaskState, string> = {
  backlog: 'var(--color-fg-tertiary)',
  todo: 'var(--color-info)',
  doing: 'var(--color-fg-accent)',
  review: 'var(--color-warning)',
  blocked: 'var(--color-danger)',
  done: 'var(--color-success)',
  cancelled: 'var(--color-fg-tertiary)',
}
