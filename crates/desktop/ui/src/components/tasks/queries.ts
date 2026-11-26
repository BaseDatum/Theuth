import { useQuerySupport } from '@/rpc/capabilities'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { notify as toast } from '@/lib/notify'
import { useProjects } from '@/components/chat/ProjectPicker'
import { errorText } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'
import type { EngineAgent } from "@/rpc/bindings/method.roster.output"
import type { AgentSummary } from "@/rpc/bindings/method.roster.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import type { Task } from "@/rpc/bindings/method.roster.output"
import type { TaskEvent } from "@/rpc/bindings/method.roster.output"
import type { DoneBecause, TaskFilter, TaskPatch } from '@/rpc/bindings/method.roster.input'
import type { TaskState } from "@/rpc/bindings/method.roster.output"
import { on } from '@/store/sessions'
import { ownerText } from './helpers'
import { AGENTS, EVENTS, MESH, TASKS, mockOn } from './mock'

/** Query keys of the 67 records; `tasks` prefixes every list and `task(id)`. */
export const keys = {
  tasks: ['tasks'] as const,
  tasksList: (f: TaskFilter) => ['tasks', 'list', f] as const,
  task: (id: string) => ['tasks', 'get', id] as const,
  taskEvents: (id: string) => ['tasks', 'events', id] as const,
  agents: queryKeys.roster,
  engineAgents: [...queryKeys.roster, 'engine-agents'] as const,
  mesh: ['mesh'] as const,
}

export const EMPTY_FILTER: TaskFilter = { owner: null, state: null, engine: null, project: null }

const matches = (t: Task, f: TaskFilter) =>
  (f.owner == null || ownerText(t.owner) === f.owner) &&
  (f.state == null || f.state.includes(t.state)) &&
  (f.engine == null || t.where === f.engine) &&
  (f.project == null || t.project === f.project)

export function useTasks(filter: TaskFilter = EMPTY_FILTER) {
  const canQuery1 = useQuerySupport(["tasks_list"])
  const api = useApi()
  return useQuery({ enabled: canQuery1,
    queryKey: keys.tasksList(filter),
    queryFn: () => (mockOn() ? TASKS.filter((t) => matches(t, filter)) : api.tasks_list(filter)),
  })
}

export function useTask(id: string, enabled = true) {
  const canQuery2 = useQuerySupport(["task_get"])
  const api = useApi()
  return useQuery({
    queryKey: keys.task(id),
    queryFn: () => (mockOn() ? TASKS.find((t) => t.id === id) ?? Promise.reject(new Error('no such task')) : api.task_get(id)),
    enabled: canQuery2 && enabled,
  })
}

export function useTaskEvents(id: string) {
  const canQuery3 = useQuerySupport(["task_events"])
  const api = useApi()
  return useQuery({ enabled: canQuery3, queryKey: keys.taskEvents(id), queryFn: (): Promise<TaskEvent[]> | TaskEvent[] => (mockOn() ? EVENTS : api.task_events(id)) })
}

export function useAgents() {
  const canQuery4 = useQuerySupport(["agents_list"])
  const api = useApi()
  return useQuery({ enabled: canQuery4, queryKey: keys.agents, queryFn: (): Promise<AgentSummary[]> | AgentSummary[] => (mockOn() ? AGENTS : api.agents_list()) })
}

/** 182: every engine's replicated roster (`engine_agents_list`); the mock has no peers. */
export function useEngineAgents() {
  const canQuery5 = useQuerySupport(["engine_agents_list"])
  const api = useApi()
  return useQuery({ enabled: canQuery5, queryKey: keys.engineAgents, queryFn: (): Promise<EngineAgent[]> | EngineAgent[] => (mockOn() ? [] : api.engine_agents_list()) })
}

/** Session ids that never move with a task: agent desks and harness (`agent`) sessions. */
export function useFixedSessions(): string[] {
  const canQuery6 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const agents = useAgents()
  const sessions = useQuery({ enabled: canQuery6, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  return [...(agents.data ?? []).flatMap((a) => (a.desk ? [a.desk] : [])), ...(sessions.data ?? []).flatMap((s) => (s.agent || s.desk ? [s.id] : []))]
}

export function useMesh() {
  const canQuery7 = useQuerySupport(["mesh_status"])
  const api = useApi()
  return useQuery({ enabled: canQuery7, queryKey: keys.mesh, queryFn: (): Promise<MeshStatus> | MeshStatus => (mockOn() ? MESH : api.mesh_status()) })
}

/**
 * Project labels the board knows: every active project (223: a project exists when it has a row) plus every label a
 * session or a task still carries without one (implicit, 51 §3), so old data keeps its board.
 */
export function useBoardProjects(): string[] {
  const known = useProjects()
  const tasks = useTasks()
  return [...new Set([...known, ...(tasks.data?.flatMap((t) => (t.project ? [t.project] : [])) ?? [])])].sort()
}

/** `task_move` as a mutation: refetches the lists (and the task's events) on success, toasts the engine's refusal verbatim. */
export function useMoveTask() {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  return useMutation({
    mutationFn: ({ id, state, note }: { id: string; state: TaskState; note?: string }) => api.task_move(id, state, note?.trim() || null),
    onSuccess: (t) => invalidate(t.id),
    onError: (e) => toast.error(errorText(e)),
  })
}

/** `task_move_home` (182): toasts `Moved to <name>`, or the engine's refusal verbatim; refetches the lists. */
export function useMoveHome() {
  const api = useApi()
  const invalidate = useInvalidateTasks()
  return useMutation({
    mutationFn: ({ id, to }: { id: string; to: string; name: string }) => api.task_move_home(id, to),
    onSuccess: (t, v) => {
      invalidate(t.id)
      toast(`Moved to ${v.name}`)
    },
    onError: (e) => toast.error(errorText(e)),
  })
}

/** Refetch every task list and one task after a write. */
export function useInvalidateTasks() {
  const qc = useQueryClient()
  return (id?: string) => {
    void qc.invalidateQueries({ queryKey: keys.tasks })
    if (id) void qc.invalidateQueries({ queryKey: keys.taskEvents(id) })
  }
}

/** The live refresh (214 D5): a trailing debounce of `LIVE_MS`, but never more than `LIVE_MAX_MS` behind the first event
 * of a burst (an agent writing tasks back to back still sees the graph move). */
const LIVE_MS = 250
const LIVE_MAX_MS = 1000

/**
 * 214 D5: every task list (and open task) refetches shortly after a task changes anywhere — an agent planning in a
 * chat, another window, the board. `tasks.changed@1` names every committed write; `tasks.activity@1` is the fallback
 * an older engine still sends. Listening by topic key only, so no generated event module is imported.
 */
export function useLiveTasks(): void {
  const qc = useQueryClient()
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined
    let first = 0
    const fire = () => {
      timer = undefined
      first = 0
      void qc.invalidateQueries({ queryKey: keys.tasks })
    }
    const bump = () => {
      const now = Date.now()
      if (first === 0) first = now
      clearTimeout(timer)
      timer = setTimeout(fire, Math.max(0, Math.min(LIVE_MS, first + LIVE_MAX_MS - now)))
    }
    const offs = [
      on('tasks.changed@1', 'TaskChanged', bump),
      on('tasks.activity@1', 'ActivityAppended', (e) => {
        if (/^(task_|child_|escalation_)/.test(e.entry.kind)) bump()
      }),
    ]
    return () => {
      clearTimeout(timer)
      for (const off of offs) off()
    }
  }, [qc])
}

/**
 * `task_update` for one task: the answer lands in its `task(id)` cache at once (an editor shows the saved value with
 * no flash of the old one), then the lists refetch. Calls are serialised per task (`scope`), and `patch` may be a
 * function of the latest cached record, so two quick blocker edits never build on the same stale list.
 */
export function useTaskUpdate(id: string) {
  const api = useApi()
  const qc = useQueryClient()
  const invalidate = useInvalidateTasks()
  return useMutation({
    scope: { id: `task-update:${id}` },
    mutationFn: (patch: TaskPatch | ((latest: Task | undefined) => TaskPatch)) =>
      api.task_update(id, typeof patch === 'function' ? patch(qc.getQueryData<Task>(keys.task(id))) : patch),
    onSuccess: (t) => {
      qc.setQueryData(keys.task(id), t)
      invalidate(id)
    },
    onError: (e) => toast.error(errorText(e)),
  })
}

/** Why a task closes as done: its stated criteria when it has them, else the intent (the engine refuses
 * `criteria_met` on a task with no `done_when`). */
export const doneBecause = (t: Pick<Task, 'done_when'>): DoneBecause => (t.done_when?.trim() ? { type: 'criteria_met' } : { type: 'intent_met' })

/** Close a task as done — the one path every "done" button takes: `task_update { state: done, done_because }`. */
export function useCloseDone() {
  const api = useApi()
  const qc = useQueryClient()
  const invalidate = useInvalidateTasks()
  return useMutation({
    mutationFn: ({ id, because }: { id: string; because: DoneBecause }) => api.task_update(id, { state: 'done', done_because: because }),
    onSuccess: (t) => {
      qc.setQueryData(keys.task(t.id), t)
      invalidate(t.id)
    },
    onError: (e) => toast.error(errorText(e)),
  })
}
