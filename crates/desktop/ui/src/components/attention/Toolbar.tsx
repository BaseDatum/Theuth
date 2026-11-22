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

const SELECT = 'h-8 w-auto shrink-0 gap-1 px-2 text-xs whitespace-nowrap'

/**
 * 207 §6: search (`/`), sort and project on the first row; the kind chips with counts on one row that scrolls
 * sideways — any combination of chips, none lit = every kind. Esc in the search clears it (the list's own Esc then
 * clears the selection).
 */
export function Toolbar({ filter, sort, kinds, projects, searchRef, onFilter, onSort }: Props) {
  const toggleKind = (kind: string) => {
    const next = new Set(filter.kinds)
    if (!next.delete(kind)) next.add(kind)
    onFilter({ ...filter, kinds: next })
  }
  const showProjects = projects.length > 1 || (projects.length === 1 && projects[0]?.name !== null)
  return (
    <div className="flex shrink-0 flex-col gap-2 border-b border-border-subtle py-2">
      <div className="flex min-w-0 items-center gap-1.5 px-3">
        <div className="relative min-w-0 flex-1">
          <Input
            ref={searchRef}
            dictate={false}
            value={filter.query}
            placeholder="Search"
            aria-label="Search inbox"
            title="Search title, agent, tool, task, project, path ( / )"
            className="h-8 rounded-[10px] bg-layer-2 pr-7 pl-8 text-base md:text-[13px]"
            onChange={(e) => onFilter({ ...filter, query: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === 'Escape' && filter.query) {
                e.stopPropagation()
                onFilter({ ...filter, query: '' })
              }
            }}
          />
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-fg-tertiary" aria-hidden />
          {filter.query && (
            <button type="button" aria-label="Clear search" onClick={() => onFilter({ ...filter, query: '' })} className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded p-0.5 text-fg-tertiary hover:text-foreground">
              <X className="size-3.5" />
            </button>
          )}
        </div>
        <Select value={sort} onValueChange={(v) => onSort(v as SortKey)}>
          <SelectTrigger size="sm" aria-label="Sort" className={SELECT}>
            <span className="text-fg-tertiary">Sort</span>
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
        {showProjects && (
          <Select value={projectValue(filter.project)} onValueChange={(v) => onFilter({ ...filter, project: projectOf(v) })}>
            <SelectTrigger size="sm" aria-label="Project" className={cn(SELECT, 'max-w-32 [&_[data-slot=select-value]]:truncate', filter.project.type !== 'all' && 'border-foreground')}>
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
        )}
      </div>
      <div className="flex gap-1.5 overflow-x-auto px-3 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden" role="group" aria-label="Kinds">
        {filter.subtree && (
          // 208 §7.4: the graph's node click restricts the list to that tree; the chip names it and clears it.
          <span className="flex h-6 max-w-48 shrink-0 items-center gap-1 rounded-full border border-fg-accent bg-fg-accent/10 pr-1 pl-2.5 text-xs whitespace-nowrap text-foreground">
            <span className="truncate">In {filter.subtree.label}</span>
            <button type="button" aria-label="Clear tree filter" title="Clear tree filter" onClick={() => onFilter({ ...filter, subtree: null })} className="rounded-full p-0.5 text-fg-tertiary hover:text-foreground">
              <X className="size-3" />
            </button>
          </span>
        )}
        {kinds
          .filter((k) => k.n > 0)
          .map(({ kind, n }) => {
            const on = filter.kinds.has(kind)
            return (
              <button
                key={kind}
                type="button"
                aria-pressed={on}
                onClick={() => toggleKind(kind)}
                className={cn(
                  'flex h-6 shrink-0 items-center gap-1.5 rounded-full border px-2.5 text-xs whitespace-nowrap motion-safe:transition-colors',
                  on ? 'border-foreground bg-foreground text-background' : 'border-border-subtle text-fg-secondary hover:bg-accent/60',
                )}
              >
                {kindFilterLabel(kind)}
                <span className={cn('tabular-nums', on ? 'opacity-70' : 'text-fg-tertiary')}>{n}</span>
              </button>
            )
          })}
      </div>
    </div>
  )
}
