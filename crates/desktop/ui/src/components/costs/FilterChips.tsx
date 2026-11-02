// 160 §7.3: one chip per dimension; each opens a Command multi-select whose options carry their spend for the current
// range under the other filters (facets). Active chips are filled and removable; a session chip searches titles.

import { Check, Search, X } from 'lucide-react'
import { memo, useState } from 'react'
import { Command, CommandEmpty, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { BASIS_COLOR, type Dim, DIMS, type FilterOption, type Filters } from '@/lib/costs/cube'
import { int, usdShort } from '@/lib/costs/format'
import type { CostBasis } from '@/rpc/bindings/CostBasis'
import { cn } from '@/lib/utils'
import { OwnerGlyph, Swatch } from './ui'

export const DIM_LABEL: Record<Dim, string> = { provider: 'Provider', model: 'Model', owner: 'Owner', kind: 'Kind', via: 'Via', basis: 'Basis', project: 'Project' }
const ORDER: Dim[] = ['provider', 'model', 'owner', 'kind', 'via', 'basis', 'project']

const chip = (on: boolean) =>
  cn(
    'inline-flex max-w-full items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50',
    on ? 'border-solid border-[var(--chart-1)] bg-[color-mix(in_srgb,var(--chart-1)_12%,transparent)] text-fg-secondary' : 'border-dashed border-border-strong text-fg-tertiary hover:text-foreground',
  )

function DimChip({ dim, options, selected, onChange }: { dim: Dim; options: FilterOption[]; selected: string[]; onChange: (ids: string[]) => void }) {
  const on = selected.length > 0
  const names = selected.map((id) => options.find((o) => o.id === id)?.label ?? id)
  const value = !on ? 'all' : names.length <= 2 ? names.join(' · ') : `${names.length} selected`
  const toggle = (id: string) => onChange(selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id])
  return (
    <span className={chip(on)}>
      <Popover>
        <PopoverTrigger className="inline-flex min-w-0 items-center gap-1.5 outline-none">
          {DIM_LABEL[dim]} <b className="truncate font-medium text-foreground">{value}</b>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-72 p-0">
          <Command>
            <CommandInput placeholder={`Filter ${DIM_LABEL[dim].toLowerCase()}…`} />
            <CommandList>
              <CommandEmpty>Nothing in this range.</CommandEmpty>
              {options.map((o) => (
                <CommandItem key={o.id} value={`${o.label} ${o.id}`} onSelect={() => toggle(o.id)} className="gap-2">
                  <Check className={cn('size-3.5', selected.includes(o.id) ? 'opacity-100' : 'opacity-0')} />
                  {dim === 'basis' && <Swatch color={BASIS_COLOR[o.id as CostBasis]} />}
                  {dim === 'owner' && <OwnerGlyph kind={o.kind ?? 'user'} />}
                  <span className="min-w-0 flex-1 truncate">{o.label}</span>
                  <span className="font-mono text-[11px] text-fg-tertiary tabular-nums">{o.usd > 0 ? usdShort(o.usd) : `${int(o.calls)} calls`}</span>
                </CommandItem>
              ))}
            </CommandList>
            {on && (
              <button type="button" onClick={() => onChange([])} className="w-full border-t border-border px-3 py-2 text-left text-xs text-fg-secondary hover:bg-layer-2">
                Clear {DIM_LABEL[dim].toLowerCase()}
              </button>
            )}
          </Command>
        </PopoverContent>
      </Popover>
      {on && (
        <button type="button" aria-label={`Clear ${DIM_LABEL[dim]}`} onClick={() => onChange([])} className="-mr-1 rounded-full p-0.5 hover:text-foreground">
          <X className="size-3" />
        </button>
      )}
    </span>
  )
}

function SessionChip({ q, onChange }: { q: string; onChange: (q: string | undefined) => void }) {
  const [draft, setDraft] = useState(q)
  return (
    <span className={chip(!!q)}>
      <Popover onOpenChange={(o) => o && setDraft(q)}>
        <PopoverTrigger className="inline-flex min-w-0 items-center gap-1.5 outline-none">
          Session <b className="truncate font-medium text-foreground">{q ? `“${q}”` : 'any'}</b>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-72 p-2">
          <form
            onSubmit={(e) => {
              e.preventDefault()
              onChange(draft.trim() || undefined)
            }}
            className="flex items-center gap-2"
          >
            <Search className="size-3.5 text-fg-tertiary" />
            <Input autoFocus value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="Title contains…" className="h-8" />
          </form>
          <p className="mt-1.5 px-1 text-[11px] text-fg-tertiary">Enter applies; matches root session titles.</p>
        </PopoverContent>
      </Popover>
      {q && (
        <button type="button" aria-label="Clear session search" onClick={() => onChange(undefined)} className="-mr-1 rounded-full p-0.5 hover:text-foreground">
          <X className="size-3" />
        </button>
      )}
    </span>
  )
}

type Props = { options: Record<Dim, FilterOption[]>; filters: Filters; set: (patch: Filters) => void }

export const FilterChips = memo(function FilterChips({ options, filters, set }: Props) {
  const any = DIMS.some((d) => filters[d]?.length) || !!filters.q
  return (
    <div className="flex flex-wrap items-center gap-2">
      {ORDER.map((d) => (
        <DimChip key={d} dim={d} options={options[d]} selected={filters[d] ?? []} onChange={(ids) => set({ [d]: ids.length ? ids : undefined })} />
      ))}
      <SessionChip q={filters.q ?? ''} onChange={(q) => set({ q })} />
      {any && (
        <button type="button" onClick={() => set(Object.fromEntries([...DIMS, 'q'].map((k) => [k, undefined])))} className="text-xs text-fg-tertiary underline-offset-2 hover:text-foreground hover:underline">
          Clear all
        </button>
      )}
    </div>
  )
})
