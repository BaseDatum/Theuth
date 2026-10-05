import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { useProjects } from '@/components/chat/ProjectPicker'
import { errorText } from '@/components/settings/shared'
import { queryKeys, useApi } from '@/lib/rpc'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import type { Task } from '@/rpc/bindings/Task'
import type { TaskEvent } from '@/rpc/bindings/TaskEvent'
import type { TaskFilter } from '@/rpc/bindings/TaskFilter'
import type { TaskState } from '@/rpc/bindings/TaskState'
import { ownerText } from './helpers'
import { AGENTS, EVENTS, MESH, TASKS, mockOn } from './mock'

/** Query keys of the 67 records; `tasks` prefixes every list and `task(id)`. */
export const keys = {
  tasks: ['tasks'] as const,
  tasksList: (f: TaskFilter) => ['tasks', 'list', f] as const,
  task: (id: string) => ['tasks', 'get', id] as const,
  taskEvents: (id: string) => ['tasks', 'events', id] as const,
  agents: queryKeys.roster,
  mesh: ['mesh'] as const,
}

export const EMPTY_FILTER: TaskFilter = { owner: null, state: null, engine: null, project: null }

const matches = (t: Task, f: TaskFilter) =>
  (f.owner === null || ownerText(t.owner) === f.owner) &&
  (f.state === null || f.state.includes(t.state)) &&
  (f.engine === null || t.where === f.engine) &&
  (f.project === null || t.project === f.project)

export function useTasks(filter: TaskFilter = EMPTY_FILTER) {
  const api = useApi()
  return useQuery({
    queryKey: keys.tasksList(filter),
    queryFn: () => (mockOn() ? TASKS.filter((t) => matches(t, filter)) : api.tasks_list(filter)),
  })
}

export function useTask(id: string, enabled = true) {
  const api = useApi()
  return useQuery({
    queryKey: keys.task(id),
    queryFn: () => (mockOn() ? TASKS.find((t) => t.id === id) ?? Promise.reject(new Error('no such task')) : api.task_get(id)),
    enabled,
  })
}

export function useTaskEvents(id: string) {
  const api = useApi()
  return useQuery({ queryKey: keys.taskEvents(id), queryFn: (): Promise<TaskEvent[]> | TaskEvent[] => (mockOn() ? EVENTS : api.task_events(id)) })
}

export function useAgents() {
  const api = useApi()
  return useQuery({ queryKey: keys.agents, queryFn: (): Promise<AgentSummary[]> | AgentSummary[] => (mockOn() ? AGENTS : api.agents_list()) })
}

export function useMesh() {
  const api = useApi()
  return useQuery({ queryKey: keys.mesh, queryFn: (): Promise<MeshStatus> | MeshStatus => (mockOn() ? MESH : api.mesh_status()) })
}

/**
 * Project labels the board knows: every session's plus every task's. A project exists when a session or a task
 * carries the label (51 §3), so a task created under a new name is what creates the project.
 */
export function useBoardProjects(): string[] {
  const fromSessions = useProjects()
  const tasks = useTasks()
  return [...new Set([...fromSessions, ...(tasks.data?.flatMap((t) => (t.project ? [t.project] : [])) ?? [])])].sort()
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

/** Refetch every task list and one task after a write. */
export function useInvalidateTasks() {
  const qc = useQueryClient()
  return (id?: string) => {
    void qc.invalidateQueries({ queryKey: keys.tasks })
    if (id) void qc.invalidateQueries({ queryKey: keys.taskEvents(id) })
  }
}
