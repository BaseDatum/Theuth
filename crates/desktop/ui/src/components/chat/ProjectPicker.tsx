import { useQuerySupport, useSupported } from '@/rpc/capabilities'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { projectsKey, useProjectRows } from '@/components/projects/list'
import { errorText } from '@/components/settings/shared'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from '@/components/ui/select'
import { notify } from '@/lib/notify'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Project } from '@/rpc/bindings/method.agent.output'

const activeNames = (rows: Project[]) => rows.flatMap((p) => (p.status === 'archived' ? [] : [p.name]))

/** Project labels, sorted: every active project (223 `projects_list`) plus every label the known sessions carry (51 §3). */
export function useProjects(): string[] {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const { data } = useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const rows = useProjectRows(activeNames).data ?? []
  return [...new Set([...rows, ...(data?.flatMap((s) => (s.project ? [s.project] : [])) ?? [])])].sort()
}

// Radix Select refuses '' as an item value, so labels are prefixed and the two commands get their own values.
const NONE = 'none'
const NEW = 'new'
const item = (label: string) => `p:${label}`

/**
 * Existing project, none, or a new label typed inline. `value` is the label as typed (untrimmed) or `null`.
 * `extra`: labels known only outside the sessions (the notes list's), offered beside theirs.
 * 223: a typed new name becomes a project (`project_create`) when the field is left or Enter is pressed, on an engine
 * that has the method; elsewhere it stays a bare label, as before.
 */
export function ProjectPicker({
  id,
  value,
  onChange,
  extra = [],
}: {
  id?: string
  value: string | null
  onChange(v: string | null): void
  extra?: readonly string[]
}) {
  const sessions = useProjects()
  const projects = extra.length ? [...new Set([...sessions, ...extra])].sort() : sessions
  const [creating, setCreating] = useState(false)
  const selected = creating ? NEW : value === null ? NONE : item(value)
  // Strict (not `useQuerySupport`): a legacy engine without the method must not toast a refusal on every new label.
  const canCreate = useSupported({ methods: ['project_create'] })
  const api = useApi()
  const qc = useQueryClient()
  const create = useMutation({
    mutationFn: (name: string) => api.project_create(name, null, null, null, null),
    onSuccess: (p) => {
      qc.setQueryData<Project[]>(projectsKey, (rows) => (rows ? [...rows.filter((r) => r.name !== p.name), p] : rows))
      void qc.invalidateQueries({ queryKey: projectsKey })
      onChange(p.name)
      setCreating(false)
    },
    onError: (e) => notify.error(errorText(e)),
  })
  const commit = () => {
    const name = value?.trim()
    if (canCreate && name && !projects.includes(name) && !create.isPending) create.mutate(name)
  }
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Select
        value={selected}
        onValueChange={(v) => {
          setCreating(v === NEW)
          onChange(v.startsWith('p:') ? v.slice(2) : null)
        }}
      >
        <SelectTrigger id={id} size="sm" className="w-48" aria-label="Project">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={NONE}>None</SelectItem>
          {projects.map((p) => (
            <SelectItem key={p} value={item(p)}>
              <ProjectIcon project={p} className="size-3.5" />
              {p}
            </SelectItem>
          ))}
          <SelectSeparator />
          <SelectItem value={NEW}>New project…</SelectItem>
        </SelectContent>
      </Select>
      {creating && (
        <Input
          autoFocus
          className="h-8 w-48"
          placeholder="Project name"
          maxLength={128}
          value={value ?? ''}
          onChange={(e) => onChange(e.target.value || null)}
          onBlur={commit}
          onKeyDown={(e) => e.key === 'Enter' && commit()}
        />
      )}
    </div>
  )
}
