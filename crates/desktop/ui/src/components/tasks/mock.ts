import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import type { Task } from '@/rpc/bindings/Task'
import type { TaskEvent } from '@/rpc/bindings/TaskEvent'

/**
 * Review fixtures for the 67 screens while the engine answers `not built`: on only with `?mock=1` in the URL or
 * `localStorage.theuth_mock67 = "1"`. Never default behaviour.
 */
export function mockOn(): boolean {
  try {
    return new URLSearchParams(window.location.search).get('mock') === '1' || localStorage.getItem('theuth_mock67') === '1'
  } catch {
    return false
  }
}

const H = 3_600_000
const now = Date.now()
const id = (n: number) => `0192a000-0000-7000-8000-${String(n).padStart(12, '0')}`

const task = (n: number, t: Partial<Task> & Pick<Task, 'title' | 'owner' | 'state' | 'project'>): Task => ({
  id: id(n),
  done_when: null,
  brief: `Brief for **${t.title}**.\n\n- step one\n- step two`,
  reviewer: null,
  blockers: [],
  blocks: [],
  parent: null,
  where: null,
  harness: null,
  external: [],
  thread: null,
  checkout: null,
  schedule: null,
  created_by: 'user',
  created_ms: now - (n + 2) * H,
  updated_ms: now - n * (H / 3),
  no_progress_count: 0,
  next_wake_after_ms: null,
  ...t,
})

export const TASKS: Task[] = [
  task(1, { title: 'Fix the flaky deploy on build-A', owner: { type: 'agent', name: 'ops' }, state: 'doing', project: 'sre', where: 'build-A', harness: 'omp',
    reviewer: { type: 'user' }, checkout: { session: 'sess-ops-1', engine: 'build-A', epoch: 1, progress_at_ms: now - H / 6, orphaned: false, taken: false }, external: ['https://github.com/example/sre/issues/42'] }),
  task(2, { title: 'Rotate the Grafana API token', owner: { type: 'agent', name: 'ops' }, state: 'blocked', project: 'sre', blockers: ['decision:apr-7f3a'], reviewer: { type: 'user' } }),
  task(3, { title: 'Write the Q4 release notes', owner: { type: 'agent', name: 'writer' }, state: 'review', project: 'launch', reviewer: { type: 'user' }, created_by: 'agent:theuth' }),
  task(4, { title: 'Draft the landing page copy', owner: { type: 'agent', name: 'writer' }, state: 'todo', project: 'launch', blockers: [id(3)], reviewer: { type: 'agent', name: 'ops' } }),
  task(5, { title: 'Review the incident runbook', owner: { type: 'user' }, state: 'review', project: 'sre' }),
  task(6, { title: 'Nightly dependency audit', owner: { type: 'agent', name: 'ops' }, state: 'done', project: 'sre', schedule: ['deps-audit', now - 8 * H], created_by: 'system' }),
  task(7, { title: 'Ask Sky about the pricing table', owner: { type: 'agent', name: 'writer' }, state: 'blocked', project: 'launch', blockers: ['person:sky'] }),
  task(8, { title: 'Old migration spike', owner: { type: 'agent', name: 'research' }, state: 'cancelled', project: null }),
  task(9, { title: 'Evaluate the new tracing backend', owner: { type: 'agent', name: 'ops' }, state: 'backlog', project: 'sre', reviewer: { type: 'user' } }),
]

export const EVENTS: TaskEvent[] = [
  { at_ms: now - 5 * H, actor: 'user', session: null, field: 'created', before: null, after: 'todo' },
  { at_ms: now - 4 * H, actor: 'agent:ops', session: 'sess-ops-1', field: 'state', before: 'todo', after: 'doing' },
  { at_ms: now - 3 * H, actor: 'agent:theuth', session: 'sess-theuth', field: 'where', before: null, after: 'build-A' },
  { at_ms: now - H, actor: 'user', session: null, field: 'comment', before: null, after: 'Check the relay logs first.' },
]

const file = (name: string, title: string, reports_to: string, extra: Partial<AgentSummary['file']> = {}): AgentSummary['file'] => ({
  name, title, reports_to, model: 'anthropic/claude-opus-5-5', effort: 'high', tools: 'mutating', mcp: null,
  budget: { type: 'none' }, capabilities: null, paused: false, proposed_by: null,
  body: `You are ${title}. Your charter is to keep things running.`, ...extra,
})

export const AGENTS: AgentSummary[] = [
  { file: file('ops', 'Ops', 'theuth', { mcp: ['grafana', 'argocd_prod'], budget: { type: 'monthly', amount: 40, currency: 'USD' }, capabilities: 'Production health, incidents, Grafana and ArgoCD. Not code review.' }),
    status: 'working', desk: 'sess-ops-desk', open_tasks: 2, spent_month: 31.2 },
  { file: file('writer', 'Writer', 'ops', { tools: 'read_only' }), status: 'blocked', desk: null, open_tasks: 3, spent_month: 0 },
  { file: file('research', 'Research', 'theuth', { paused: true, proposed_by: 'agent:ops', budget: { type: 'monthly', amount: 20, currency: 'USD' } }),
    status: 'proposed', desk: null, open_tasks: 0, spent_month: 0 },
]

export const MESH: MeshStatus = {
  self_id: 'e1a4c0ffee00000000000000000000000000000000000000000000000000mac1',
  self_name: 'mac-1',
  mesh_name: 'work',
  peers: [
    { id: 'b1ea5e0000000000000000000000000000000000000000000000000000buildA', name: 'build-A', addrs: ['10.0.0.12:4433'], reachable: true, last_seen_ms: now - 5_000, hub_pref: 'prefer' },
    { id: 'c0de0000000000000000000000000000000000000000000000000000container3', name: 'container-3', addrs: [], reachable: false, last_seen_ms: now - 2 * H, hub_pref: 'eligible' },
  ],
  lease: { holder: 'b1ea5e0000000000000000000000000000000000000000000000000000buildA', since_ms: now - 30 * H, reason: 'prefer' },
  traffic: [{ alpn: 'theuth/records/1', frames_in: 412, frames_out: 398, bytes_in: 91_204, bytes_out: 88_710 }],
  replica: { captured: 120, applied: 233, relayed: 0, echo: 0, outbox_rows: 41 },
}

export const PAIRING = 'theuth-mesh:1:e1a4c0ffee…mac1:relay=https://iroh.relay.theuth.io'
