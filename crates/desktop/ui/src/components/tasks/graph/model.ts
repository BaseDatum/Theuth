// 214 D1/D8: the project task graph as pure data — tasks → nodes and edges, the ELK graph, the structural hash. No
// React, no `@xyflow/react`: the canvas (`Canvas.tsx`) only places and draws what this returns, and the rules are tested
// in node (`model.test.ts`).
import type { ElkNode } from 'elkjs/lib/elk-api.js'
import { ELK_OPTIONS, fnv } from '@/lib/diagram/elk'
import type { PopoutTarget } from '@/lib/popout'
import type { Task, TaskState } from '@/rpc/bindings/method.roster.output'
import { type Blocker, parseBlocker, shortId } from '../helpers'

/** The two filter chips (D8). */
export type GraphOpts = { hideDone: boolean; parentEdges: boolean }
export const DEFAULT_OPTS: GraphOpts = { hideDone: false, parentEdges: false }

/** A task card, 240 × 84. `waiting`: it has a blocker that is still open (a task not done, or a decision/person). */
export type TaskNodeModel = { kind: 'task'; id: string; task: Task; closed: boolean; waiting: boolean }
/** A blocker that is not a task on this graph: `decision:…`, `person:…`, a system phrase, or a task of another project.
 * One pill per distinct blocker text, shared by every task it blocks. */
export type PillNodeModel = { kind: 'pill'; id: string; blocker: Blocker; label: string }
export type NodeModel = TaskNodeModel | PillNodeModel
/** `blocks`: source blocks target (D1, `target.blockers ∋ source`); `open` while the source is not done/cancelled.
 * `pill`: a non-task blocker into the task it blocks. `parent`: parent → child, dashed, only with the chip on. */
export type EdgeModel = { id: string; kind: 'blocks' | 'pill' | 'parent'; source: string; target: string; open: boolean }
export type GraphModel = { nodes: NodeModel[]; edges: EdgeModel[]; hash: string }

export const TASK_W = 240
export const TASK_H = 84
export const PILL_H = 28
/** The inline editor's width (it lies over the canvas's right edge; fitting leaves it this much room). */
export const PANEL_W = 380

/** A pill's width from its label: icon + padding + ~7 px a character, clamped. */
export const pillWidth = (label: string): number => Math.min(220, Math.max(88, 40 + label.length * 7))
export const sizeOf = (n: NodeModel): { w: number; h: number } => (n.kind === 'task' ? { w: TASK_W, h: TASK_H } : { w: pillWidth(n.label), h: PILL_H })

const isClosed = (s: TaskState): boolean => s === 'done' || s === 'cancelled'

/** A pill's text: `decision 1a2b3c`, `person sky`, a foreign task's short id, or the phrase itself. */
function pillLabel(b: Blocker): string {
  if (b.kind === 'decision') return `decision ${b.id.length > 10 ? shortId(b.id) : b.id}`
  if (b.kind === 'person') return b.id
  if (b.kind === 'task') return `task ${shortId(b.id)}`
  return b.raw
}

/** 214 A1: tasks that used to block `t` and were dropped from its `blockers` when they closed. Read loosely: the field
 * lands with the next wire bindings, and an older engine never sends it. */
export const resolvedBlockers = (t: Task): readonly string[] => (t as Task & { resolved_blockers?: string[] }).resolved_blockers ?? []

/**
 * The graph of `tasks` (one project's, or every task for "All projects"). Node order is creation order (ELK keeps
 * model order, so a new task lands after its siblings); edges are deduplicated and a task blocking itself is ignored.
 * Cycles need no special care: nothing here walks the graph, and ELK's layered algorithm breaks them itself. The
 * hash covers node ids, pill sizes and edges only — never states, titles or owners — so a state change never moves a
 * node. A finished dependency (A1 `resolved_blockers`) keeps its edge, faded, under the same id as while it blocked:
 * a blocker resolving never relayouts.
 */
export function taskGraph(tasks: readonly Task[], opts: GraphOpts = DEFAULT_OPTS): GraphModel {
  const all = new Map(tasks.map((t) => [t.id, t]))
  const shown = [...tasks].filter((t) => !(opts.hideDone && isClosed(t.state))).sort((a, b) => a.created_ms - b.created_ms || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
  const visible = new Set(shown.map((t) => t.id))
  const nodes: NodeModel[] = []
  const pills = new Map<string, PillNodeModel>()
  const edges = new Map<string, EdgeModel>()
  for (const t of shown) {
    let waiting = false
    for (const raw of t.blockers) {
      const b = parseBlocker(raw)
      if (b.kind === 'task' && all.has(b.id)) {
        const src = all.get(b.id)!
        const open = !isClosed(src.state)
        waiting ||= open
        // A hidden (done) blocker is satisfied: no edge, no pill.
        if (b.id === t.id || !visible.has(b.id)) continue
        const id = `b:${b.id}>${t.id}`
        edges.set(id, { id, kind: 'blocks', source: b.id, target: t.id, open })
        continue
      }
      waiting = true
      const pid = `x:${raw}`
      if (!pills.has(pid)) pills.set(pid, { kind: 'pill', id: pid, blocker: b, label: pillLabel(b) })
      const id = `p:${raw}>${t.id}`
      edges.set(id, { id, kind: 'pill', source: pid, target: t.id, open: true })
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
    nodes.push({ kind: 'task', id: t.id, task: t, closed: isClosed(t.state), waiting: waiting && !isClosed(t.state) })
  }
  nodes.push(...[...pills.values()].sort((a, b) => (a.id < b.id ? -1 : 1)))
  // Sorted by id: one hash is one edge order, so a cached layout's routes (`e:<index>`) always match these edges.
  const edgeList = [...edges.values()].sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
  return { nodes, edges: edgeList, hash: graphHash(nodes, edgeList) }
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

/** The model as an ELK graph: `n:<id>` nodes in model order, `e:<index>` edges (what `flattenElk` reads back). */
export function toElk(m: GraphModel): ElkNode {
  return {
    id: 'root',
    layoutOptions: TASK_ELK,
    children: m.nodes.map((n) => {
      const s = sizeOf(n)
      return { id: `n:${n.id}`, width: s.w, height: s.h }
    }),
    edges: m.edges.map((e, i) => ({ id: `e:${i}`, sources: [`n:${e.source}`], targets: [`n:${e.target}`] })),
  }
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
