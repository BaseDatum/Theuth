import { Search, X } from 'lucide-react'
import type { Ref } from 'react'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { cn } from '@/lib/utils'
import { kindFilterLabel } from './labels'
import { type ListFilter, type ProjectFilter, SORTS, type SortKey } from './model'

/** The project select's value: `all`, `none`, or `p:<name>` (a project may be called anything, even "all"). */
const projectValue = (p: ProjectFilter): string => (p.type === 'project' ? `p:${p.name}` : p.type)
const projectOf = (v: string): ProjectFilter => (v === 'all' ? { type: 'all' } : v === 'none' ? { type: 'none' } : { type: 'project', name: v.slice(2) })

type Props = {
  filter: ListFilter
  sort: SortKey
  kinds: readonly { kind: string; n: number }[]
  projects: readonly { name: string | null; n: number }[]
  searchRef?: Ref<HTMLInputElement>
  onFilter: (f: ListFilter) => void
  onSort: (s: SortKey) => void
}

/**
 * 207 §6: search (`/`), sort, project and the kind chips with counts — any combination of chips; none lit = every kind.
 * Esc in the search clears it (the list's own Esc then clears the selection).
 */
export function Toolbar({ filter, sort, kinds, projects, searchRef, onFilter, onSort }: Props) {
  const toggleKind = (kind: string) => {
    const next = new Set(filter.kinds)
    if (!next.delete(kind)) next.add(kind)
    onFilter({ ...filter, kinds: next })
  }
  return (
    <div className="flex shrink-0 flex-col gap-2 border-b border-border-subtle px-3 py-2">
      <div className="flex items-center gap-1.5">
        <div className="relative min-w-0 flex-1">
          <Search className="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-fg-tertiary" />
          <Input
            ref={searchRef}
            value={filter.query}
            placeholder="Search title, agent, tool, project…"
            aria-label="Search attention"
            className="h-8 rounded-[10px] bg-layer-2 pl-8 text-base md:text-[13px]"
            onChange={(e) => onFilter({ ...filter, query: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && filter.query) {
                e.stopPropagation()
                onFilter({ ...filter, query: '' })
              }
            }}
          />
          {filter.query && (
            <button type="button" aria-label="Clear search" onClick={() => onFilter({ ...filter, query: '' })} className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded p-0.5 text-fg-tertiary hover:text-foreground">
              <X className="size-3.5" />
            </button>
          )}
        </div>
        <Select value={sort} onValueChange={(v) => onSort(v as SortKey)}>
          <SelectTrigger size="sm" aria-label="Sort" className="h-8 w-auto gap-1 px-2 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent align="end">
            {SORTS.map((s) => (
              <SelectItem key={s.key} value={s.key}>
                {s.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="flex flex-wrap items-center gap-1" role="group" aria-label="Kinds">
        {kinds.map(({ kind, n }) => {
          const on = filter.kinds.has(kind)
          return (
            <button
              key={kind}
              type="button"
              aria-pressed={on}
              onClick={() => toggleKind(kind)}
              className={cn('flex h-6 items-center gap-1 rounded-full border px-2 text-xs motion-safe:transition-colors', on ? 'border-fg-accent bg-accent' : 'border-border-subtle text-fg-secondary hover:bg-accent/60')}
            >
              {kindFilterLabel(kind)}
              <span className="tabular-nums text-fg-tertiary">{n}</span>
            </button>
          )
        })}
        {projects.length > 1 || (projects.length === 1 && projects[0]?.name !== null) ? (
          <Select value={projectValue(filter.project)} onValueChange={(v) => onFilter({ ...filter, project: projectOf(v) })}>
            <SelectTrigger size="sm" aria-label="Project" className={cn('ml-auto h-6 w-auto max-w-40 gap-1 rounded-full px-2 text-xs', filter.project.type !== 'all' && 'border-fg-accent bg-accent')}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              <SelectItem value="all">All projects</SelectItem>
              {projects.map((p) =>
                p.name === null ? (
                  <SelectItem key="none" value="none">
                    No project <span className="tabular-nums text-fg-tertiary">{p.n}</span>
                  </SelectItem>
                ) : (
                  <SelectItem key={p.name} value={`p:${p.name}`}>
                    {p.name} <span className="tabular-nums text-fg-tertiary">{p.n}</span>
                  </SelectItem>
                ),
              )}
            </SelectContent>
          </Select>
        ) : null}
      </div>
    </div>
  )
}
