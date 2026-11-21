// 208: the Attention graph's fixture — 40 nodes in 8 trees plus 2 boundary peers, every node kind, AgentState, wait and
// outcome; 18 pair edges; a scripted delta sequence. Tests use GRAPH_FIXTURE (a fixed instant); the dev page shifts
// times with graphFixture(Date.now()). Expected TTL outcomes: contract §9.2.
import type { AgentState, AgentGraph, GraphEdge, GraphNode, GraphNodeKind, GraphOutcome, GraphWait, ObservedMeta, ObservedState, SessionActivity } from '@/rpc/bindings/method.agent.output'
import type { AgentGraphChanged } from '@/rpc/bindings/event.agent_2egraph.output'

export const GRAPH_FIXTURE_NOW = 1_800_000_000_000
const MIN = 60_000

const LIVE = new Set(['running', 'awaiting', 'approval'])
function activity(state: AgentState, extra: Partial<SessionActivity> = {}): SessionActivity {
  return { asks: null, children: 0, jobs: 0, last_active: null, live: LIVE.has(state.state), ptys: 0, state, turn_started_at: null, waiting_since_ms: null, ...extra }
}
function observed(state: ObservedState): ObservedMeta {
  return { can_cancel: false, cost_rule: 'inside_parent', description: null, detail: null, external: 'x', source: 'omp_task', state, stop: null, tool_call: null }
}
const RUN: AgentState = { state: 'running' }
const IDLE: AgentState = { state: 'idle' }
const ERR: AgentState = { state: 'error' }

type Spec = {
  id: string; kind: GraphNodeKind; parent?: string; title: string; ago: number; state?: AgentState
  outcome?: GraphOutcome; waits?: GraphWait[]; agent?: string; project?: string; desk?: string; remote?: boolean
  observed?: ObservedState; jobs?: number; started?: number; model?: string
}
const SPECS: Spec[] = [
  // r1 Ship 2.5 release — a busy root: approval, awaiting, job wait, ACP children, observed children
  { id: 'r1', kind: 'root', title: 'Ship 2.5 release', ago: 0, state: RUN, project: 'release-2.5', started: 60 },
  { id: 'r1.builder', kind: 'child', parent: 'r1', title: 'builder', ago: 1, state: { state: 'approval', tool: 'bash' }, started: 58 },
  { id: 'r1.builder.linter', kind: 'child', parent: 'r1.builder', title: 'linter', ago: 2, state: { state: 'awaiting', from: 'builder' }, started: 50 },
  { id: 'r1.builder.tests', kind: 'child', parent: 'r1.builder', title: 'tests', ago: 3, state: RUN, waits: ['job'], jobs: 2, started: 49 },
  { id: 'r1.docs', kind: 'child', parent: 'r1', title: 'docs', ago: 6, outcome: 'finished', started: 57 },
  { id: 'r1.reviewer', kind: 'child', parent: 'r1', title: 'reviewer', ago: 8, outcome: 'failed', started: 56 },
  { id: 'r1.scout', kind: 'child', parent: 'r1', title: 'scout', ago: 12, outcome: 'finished', started: 55 },
  { id: 'r1.claude', kind: 'child', parent: 'r1', title: 'impl', ago: 0, state: RUN, agent: 'claude', model: 'anthropic/claude-sonnet-5', started: 54 },
  { id: 'r1.claude.explore', kind: 'observed', parent: 'r1.claude', title: 'explore', ago: 1, state: RUN, observed: 'running', started: 40 },
  { id: 'r1.claude.plan', kind: 'observed', parent: 'r1.claude', title: 'plan', ago: 9, outcome: 'finished', observed: 'completed', started: 41 },
  { id: 'r1.codex', kind: 'child', parent: 'r1', title: 'port', ago: 4, state: ERR, outcome: 'failed', agent: 'codex', model: 'openai/gpt-5.5', started: 53 },
  // r2 Fix flaky CI — idle root, cancelled / failed / timed out children, an omp harness with observed tasks
  { id: 'r2', kind: 'root', title: 'Fix flaky CI', ago: 30, outcome: 'finished', project: 'theuth', started: 120 },
  { id: 'r2.bisect', kind: 'child', parent: 'r2', title: 'bisect', ago: 20, state: { state: 'cancelled' }, outcome: 'cancelled', started: 110 },
  { id: 'r2.repro', kind: 'child', parent: 'r2', title: 'repro', ago: 9.5, state: ERR, outcome: 'failed', started: 109 },
  { id: 'r2.repro.min', kind: 'child', parent: 'r2.repro', title: 'minimise', ago: 11, outcome: 'finished', started: 100 },
  { id: 'r2.verify', kind: 'child', parent: 'r2', title: 'verify', ago: 7, outcome: 'timed_out', started: 108 },
  { id: 'r2.omp', kind: 'child', parent: 'r2', title: 'sweep', ago: 7, outcome: 'finished', agent: 'omp', started: 107 },
  { id: 'r2.omp.task1', kind: 'observed', parent: 'r2.omp', title: 'task 1', ago: 5, outcome: 'finished', observed: 'completed', started: 90 },
  { id: 'r2.omp.task2', kind: 'observed', parent: 'r2.omp', title: 'task 2', ago: 3, state: ERR, outcome: 'failed', observed: 'failed', started: 89 },
  // r3 ops desk — a desk with an ask (waiting on you), an expired child, and a live grandchild pinning its parent
  { id: 'r3', kind: 'desk', title: 'ops desk', ago: 300, desk: 'ops', outcome: 'finished', started: 5000 },
  { id: 'r3.sweep', kind: 'child', parent: 'r3', title: 'nightly-sweep', ago: 40, state: RUN, waits: ['user'], started: 400 },
  { id: 'r3.rotate', kind: 'child', parent: 'r3', title: 'rotate-keys', ago: 200, outcome: 'finished', started: 399 },
  { id: 'r3.audit', kind: 'child', parent: 'r3', title: 'audit', ago: 2, outcome: 'finished', started: 398 },
  { id: 'r3.audit.grep', kind: 'child', parent: 'r3.audit', title: 'grep', ago: 0, state: RUN, started: 30 },
  { id: 'r3.audit.diff', kind: 'child', parent: 'r3.audit', title: 'diff', ago: 1, state: { state: 'awaiting', from: 'audit' }, started: 29 },
  // r4 Nightly routine — a root in its fade, children expired
  { id: 'r4', kind: 'root', title: 'Nightly routine 11-21', ago: 230, outcome: 'finished', project: 'theuth', started: 600 },
  { id: 'r4.summarize', kind: 'child', parent: 'r4', title: 'summarize', ago: 232, outcome: 'finished', started: 599 },
  { id: 'r4.index', kind: 'child', parent: 'r4', title: 'index', ago: 235, outcome: 'finished', started: 598 },
  // r5 Refactor store — an expired root kept by a live descendant waiting on its children
  { id: 'r5', kind: 'root', title: 'Refactor store', ago: 250, outcome: 'finished', started: 700 },
  { id: 'r5.migrator', kind: 'child', parent: 'r5', title: 'migrator', ago: 0, state: RUN, waits: ['children'], started: 690 },
  { id: 'r5.migrator.check', kind: 'child', parent: 'r5.migrator', title: 'check', ago: 1, state: RUN, started: 20 },
  { id: 'r5.old', kind: 'child', parent: 'r5', title: 'old-pass', ago: 245, outcome: 'finished', started: 689 },
  // r6 Billing export — owned by another engine (last-known state)
  { id: 'r6', kind: 'root', title: 'Billing export', ago: 15, state: RUN, project: 'billing', remote: true, started: 100 },
  { id: 'r6.fetch', kind: 'child', parent: 'r6', title: 'fetch', ago: 16, outcome: 'finished', remote: true, started: 99 },
  { id: 'r6.render', kind: 'child', parent: 'r6', title: 'render', ago: 15, state: RUN, remote: true, started: 98 },
  // r7 concierge desk — two attention items on the desk, a child in its fade
  { id: 'r7', kind: 'desk', title: 'concierge desk', ago: 50, desk: 'concierge', outcome: 'finished', started: 9000 },
  { id: 'r7.reply', kind: 'child', parent: 'r7', title: 'reply', ago: 9, outcome: 'finished', started: 60 },
  // r8 Scratch — an expired root with nothing to keep it
  { id: 'r8', kind: 'root', title: 'Scratch', ago: 241, outcome: 'finished', started: 300 },
  // boundary peers
  { id: 'peer:alice', kind: 'peer', title: 'alice', ago: 50, started: 3000 },
  { id: 'peer:mini-ops', kind: 'peer', title: 'mini-ops', ago: 120, started: 3000 },
]

const rootOf = (id: string): string => (id.startsWith('peer:') ? id : (id.split('.')[0] as string))

function node(s: Spec, now: number): GraphNode {
  const state = s.state ?? IDLE
  return {
    activity: activity(state, { jobs: s.jobs ?? 0 }),
    agent: s.agent ?? null,
    desk: s.desk ?? null,
    id: s.id,
    kind: s.kind,
    last_event_ms: now - s.ago * MIN,
    model: s.kind === 'peer' ? '' : (s.model ?? 'anthropic/claude-opus-5-5'),
    name: s.kind === 'child' || s.kind === 'observed' ? s.title : null,
    observed: s.observed ? observed(s.observed) : null,
    outcome: s.outcome ?? null,
    owner: s.kind === 'peer' ? '' : s.remote ? 'mini' : 'studio',
    parent: s.parent ?? null,
    project: s.project ?? (s.parent ? (SPECS.find((x) => x.id === rootOf(s.id))?.project ?? null) : null),
    remote: s.remote ?? s.kind === 'peer',
    root: rootOf(s.id),
    started_ms: now - (s.started ?? 0) * MIN,
    title: s.title,
    waits: s.waits ?? [],
  }
}

type EdgeSpec = [a: string, b: string, ab: number, ba: number, results: number, open: number, ago: number, lastFrom: string]
const EDGES: EdgeSpec[] = [
  ['r1', 'r1.builder', 3, 1, 0, 0, 2, 'r1.builder'],
  ['r1.builder', 'r1.builder.linter', 2, 1, 1, 1, 1, 'r1.builder'],
  ['r1.builder', 'r1.builder.tests', 1, 0, 0, 0, 3, 'r1.builder'],
  ['r1', 'r1.docs', 1, 1, 1, 0, 6, 'r1.docs'],
  ['r1', 'r1.reviewer', 1, 1, 1, 0, 8, 'r1.reviewer'],
  ['r1.builder', 'r1.reviewer', 4, 3, 0, 0, 9, 'r1.reviewer'],
  ['r1.claude', 'r1.codex', 2, 2, 0, 0, 4, 'r1.codex'],
  ['r1', 'r2', 1, 0, 0, 0, 25, 'r1'],
  ['r2', 'r2.repro', 1, 1, 1, 0, 9.5, 'r2.repro'],
  ['r2', 'r2.verify', 1, 1, 1, 0, 7, 'r2.verify'],
  ['r3', 'r3.sweep', 1, 2, 0, 0, 40, 'r3.sweep'],
  ['r3.audit', 'r3.audit.diff', 0, 1, 0, 1, 1, 'r3.audit.diff'],
  ['r3', 'r6', 0, 2, 0, 0, 15, 'r6'],
  ['peer:alice', 'r7', 3, 2, 0, 0, 50, 'r7'],
  ['peer:mini-ops', 'r3', 1, 1, 0, 0, 120, 'r3'],
  ['r5.migrator', 'r5.migrator.check', 1, 0, 0, 1, 1, 'r5.migrator'],
  ['r1', 'r8', 1, 0, 0, 0, 241, 'r1'],
  ['r4', 'r4.summarize', 0, 1, 1, 0, 232, 'r4.summarize'],
]

function edge([a, b, ab, ba, results, open, ago, lastFrom]: EdgeSpec, now: number, i: number): GraphEdge {
  return { a, ab, b, ba, id: `${a}|${b}`, last_from: lastFrom, last_id: `x-${String(i).padStart(2, '0')}`, last_ms: now - ago * MIN, open, results }
}

export const GRAPH_FIXTURE_EPOCH = GRAPH_FIXTURE_NOW - 60 * MIN

export function graphFixture(now: number): AgentGraph {
  return {
    at_ms: now,
    edges: EDGES.map((e, i) => edge(e, now, i)),
    epoch_ms: now - 60 * MIN,
    nodes: SPECS.map((s) => node(s, now)),
    older: { newest_ms: now - 300 * MIN, nodes: 12, roots: 3 },
    seq: 7,
    unchanged: false,
    window_ms: 1440 * MIN,
  }
}
export const GRAPH_FIXTURE: AgentGraph = graphFixture(GRAPH_FIXTURE_NOW)

/** Open attention items per session at the fixture instant (the 207 list joined on item.session). */
export const GRAPH_FIXTURE_ATTENTION: ReadonlyMap<string, number> = new Map([
  ['r1.builder', 1],
  ['r3', 1],
  ['r3.sweep', 1],
  ['r7', 2],
])

const base = (seq: number, atMs: number): AgentGraphChanged => ({ at_ms: atMs, edges: [], edges_removed: [], epoch_ms: GRAPH_FIXTURE_EPOCH, nodes: [], older: null, pulses: [], removed: [], resync: false, seq, type: 'AgentGraphChanged' })
const at = (sec: number) => GRAPH_FIXTURE_NOW + sec * 1000
const fmt = (state: AgentState, outcome: GraphOutcome | null, sec: number): GraphNode => ({
  ...node({ id: 'r1.builder.fmt', kind: 'child', parent: 'r1.builder', title: 'fmt', ago: 0, state, started: 0 }, GRAPH_FIXTURE_NOW),
  last_event_ms: at(sec),
  outcome,
})

/** Applied in order to GRAPH_FIXTURE (seq 7). Expected reducer results: contract §9.4. */
export const GRAPH_FIXTURE_DELTAS: AgentGraphChanged[] = [
  // 0: builder spawns fmt
  { ...base(8, at(1)), nodes: [{ ...node(SPECS[1] as Spec, GRAPH_FIXTURE_NOW), activity: activity({ state: 'approval', tool: 'bash' }, { children: 3 }), last_event_ms: at(1) }, fmt(RUN, null, 1)] },
  // 1: fmt messages builder
  { ...base(9, at(2)), edges: [{ a: 'r1.builder', ab: 0, b: 'r1.builder.fmt', ba: 1, id: 'r1.builder|r1.builder.fmt', last_from: 'r1.builder.fmt', last_id: 'x-90', last_ms: at(2), open: 1, results: 0 }], pulses: [{ at_ms: at(2), edge: 'r1.builder|r1.builder.fmt', from: 'r1.builder.fmt', id: 'x-90', kind: 'sent', to: 'r1.builder' }] },
  // 2: builder's approval allowed: running again
  { ...base(10, at(3)), nodes: [{ ...node(SPECS[1] as Spec, GRAPH_FIXTURE_NOW), activity: activity(RUN, { children: 3 }), last_event_ms: at(3) }] },
  // 3: fmt finishes; its result reaches builder
  { ...base(11, at(4)), nodes: [fmt(IDLE, 'finished', 4)], edges: [{ a: 'r1.builder', ab: 0, b: 'r1.builder.fmt', ba: 2, id: 'r1.builder|r1.builder.fmt', last_from: 'r1.builder.fmt', last_id: 'x-91', last_ms: at(4), open: 0, results: 1 }], pulses: [{ at_ms: at(4), edge: 'r1.builder|r1.builder.fmt', from: 'r1.builder.fmt', id: 'x-91', kind: 'result', to: 'r1.builder' }] },
  // 4: Scratch deleted
  { ...base(12, at(5)), removed: ['r8'], edges_removed: ['r1|r8'] },
  // 5: stale (seq 12 again)
  { ...base(12, at(6)) },
  // 6: gap (seq 14 after 12)
  { ...base(14, at(7)) },
  // 7: a new activation
  { ...base(1, at(8)), epoch_ms: GRAPH_FIXTURE_EPOCH + 1 },
  // 8: resync
  { ...base(13, at(9)), resync: true },
]
