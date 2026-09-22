import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { ProjectIcon } from '@/components/sidebar/ProjectStyle'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from '@/components/ui/select'
import { queryKeys, useApi } from '@/lib/rpc'

/** Distinct project labels across the known sessions (51 §3), sorted. */
export function useProjects(): string[] {
  const api = useApi()
  const { data } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  return [...new Set(data?.flatMap((s) => (s.project ? [s.project] : [])))].sort()
}

// Radix Select refuses '' as an item value, so labels are prefixed and the two commands get their own values.
const NONE = 'none'
const NEW = 'new'
const item = (label: string) => `p:${label}`

/** Existing project, none, or a new label typed inline. `value` is the label as typed (untrimmed) or `null`. */
export function ProjectPicker({ id, value, onChange }: { id?: string; value: string | null; onChange(v: string | null): void }) {
  const projects = useProjects()
  const [creating, setCreating] = useState(false)
  const selected = creating ? NEW : value === null ? NONE : item(value)
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
        />
      )}
    </div>
  )
}
