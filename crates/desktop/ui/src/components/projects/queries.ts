import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { useNotes } from '@/components/notes/useNotes'
import { errorText } from '@/components/settings/shared'
import { useTasks } from '@/components/tasks/queries'
import { notify } from '@/lib/notify'
import { queryKeys, useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { DocSummary, Project, ProjectDeleted, SessionSummary } from '@/rpc/bindings/method.agent.output'
import type { Task } from '@/rpc/bindings/method.roster.output'
import { onTopic } from '@/store/sessions'
import { projectsKey, useProjectRows } from './list'

/** A project's metadata, string → string (223 Data). */
export type Metadata = Record<string, string>

/** What a project groups: its root chats, tasks and notes (boards included). */
export type ProjectCounts = { chats: number; tasks: number; notes: number }

const EMPTY: ProjectCounts = { chats: 0, tasks: 0, notes: 0 }

/** Per-label counts over the lists the client already holds (the wire returns rows only). */
export function projectCounts(
  sessions: readonly Pick<SessionSummary, 'project' | 'parent'>[],
  tasks: readonly Pick<Task, 'project'>[],
  notes: readonly Pick<DocSummary, 'project'>[],
): Record<string, ProjectCounts> {
  const out: Record<string, ProjectCounts> = {}
  const bump = (p: string | null, k: keyof ProjectCounts) => {
    if (p) (out[p] ??= { ...EMPTY })[k] += 1
  }
  for (const s of sessions) if (!s.parent) bump(s.project, 'chats')
  for (const t of tasks) bump(t.project, 'tasks')
  for (const n of notes) bump(n.project, 'notes')
  return out
}

export const countsOf = (all: Record<string, ProjectCounts>, name: string): ProjectCounts => all[name] ?? EMPTY

const byStatusThenName = (rows: Project[]) =>
  [...rows].sort((a, b) => Number(a.status === 'archived') - Number(b.status === 'archived') || a.name.localeCompare(b.name))

/** Every project, archived last, then by name. */
export const useProjectList = () => useProjectRows(byStatusThenName)

/** One project by name (from the list); `undefined` while loading or when it does not exist. */
export function useProject(name: string): { project: Project | undefined; isPending: boolean; error: Error | null } {
  const list = useProjectList()
  return { project: list.data?.find((p) => p.name === name), isPending: list.isPending, error: list.error }
}

/** Counts for every label, from the sessions, tasks and notes lists. */
export function useProjectCounts(): Record<string, ProjectCounts> {
  const can = useQuerySupport(['list_sessions'])
  const api = useApi()
  const sessions = useQuery({ enabled: can, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const tasks = useTasks()
  const notes = useNotes()
  return projectCounts(sessions.data ?? [], tasks.data ?? [], notes.data ?? [])
}

/** Refetch the projects list and the styles (style lives on the same row). */
export function useInvalidateProjects() {
  const qc = useQueryClient()
  return () => {
    void qc.invalidateQueries({ queryKey: projectsKey })
    void qc.invalidateQueries({ queryKey: queryKeys.projectStyles })
  }
}

/** `agent.projects` (223): a project changed here, in another window or on a peer. */
export function useLiveProjects(): void {
  const invalidate = useInvalidateProjects()
  useEffect(() => onTopic('agent.projects@1', invalidate), [invalidate])
}

export type NewProject = { name: string; description: string | null; metadata: Metadata | null }

/** `project_create`; the engine refuses a name that already has a row, toasted verbatim. */
export function useCreateProject() {
  const api = useApi()
  const invalidate = useInvalidateProjects()
  return useMutation({
    mutationFn: ({ name, description, metadata }: NewProject) => api.project_create(name, description, null, null, metadata),
    onSuccess: invalidate,
    onError: (e) => notify.error(errorText(e)),
  })
}

/** Absent fields stay as they are; `description: ''` clears it. */
export type ProjectPatch = { description?: string; status?: 'active' | 'archived'; metadata?: Metadata }

/** `project_update` for `name`: absent fields stay; on an implicit label it creates the row. */
export function useUpdateProject(name: string) {
  const api = useApi()
  const qc = useQueryClient()
  const invalidate = useInvalidateProjects()
  return useMutation({
    scope: { id: `project-update:${name}` },
    mutationFn: (p: ProjectPatch) => api.project_update(name, p.description ?? null, p.status ?? null, null, null, p.metadata ?? null),
    onSuccess: (row) => {
      qc.setQueryData<Project[]>(projectsKey, (rows) => rows?.map((r) => (r.name === row.name ? row : r)))
      invalidate()
    },
    onError: (e) => notify.error(errorText(e)),
  })
}

/** `project_delete`: detaches every item, then drops the row; the lists that carried the label refetch. */
export function useDeleteProject() {
  const api = useApi()
  const qc = useQueryClient()
  const invalidate = useInvalidateProjects()
  return useMutation({
    mutationFn: (name: string) => api.project_delete(name),
    onSuccess: (d, name) => {
      invalidate()
      for (const key of [queryKeys.sessions, queryKeys.notes, ['tasks']]) void qc.invalidateQueries({ queryKey: key })
      notify(`Deleted ${name}`, { description: detachedText(d) })
    },
    onError: (e) => notify.error(errorText(e)),
  })
}

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`

/** "3 chats, 1 task and 2 notes"; `null` when nothing carries the label. */
export function itemsText(c: ProjectCounts): string | null {
  const parts = [c.chats && plural(c.chats, 'chat'), c.tasks && plural(c.tasks, 'task'), c.notes && plural(c.notes, 'note')].filter(Boolean) as string[]
  if (!parts.length) return null
  return parts.length === 1 ? parts[0]! : `${parts.slice(0, -1).join(', ')} and ${parts.at(-1)}`
}

/** What `project_delete` detached, for the toast after it. */
export function detachedText(d: ProjectDeleted): string {
  const parts = [
    d.sessions && plural(d.sessions, 'chat'),
    d.tasks && plural(d.tasks, 'task'),
    d.docs && plural(d.docs, 'note'),
    d.notebooks && plural(d.notebooks, 'notebook'),
    d.meetings && plural(d.meetings, 'meeting'),
  ].filter(Boolean)
  return parts.length ? `Detached ${parts.join(', ')}.` : 'Nothing carried the label.'
}
