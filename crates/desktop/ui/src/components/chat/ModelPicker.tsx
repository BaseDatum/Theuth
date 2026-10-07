// 124 §3: the cmdk body every model picker shares — providers level → a provider's models, with the
// §2 word search over every model whenever the input is non-empty. Ranking is ours (`shouldFilter={false}`).

import { Check, ChevronLeft, ChevronRight } from 'lucide-react'
import { useState } from 'react'
import { Command, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import { modelsOfProvider, providersOf, rankModels, searchTerms } from '@/lib/model-search'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'

/** The provider row that lists `extra` ids (on-file values missing from the catalog). */
export const OTHER = 'other'

export type Row =
  | { kind: 'current'; value: string; id: string; name: string }
  | { kind: 'none'; value: 'none'; label: string; current: boolean }
  | { kind: 'back'; value: 'back' }
  | { kind: 'provider'; value: string; id: string; count: number; current: boolean }
  | { kind: 'model'; value: string; m: ModelInfo; current: boolean }
  | { kind: 'empty'; value: 'empty' }

export type View = {
  mode: 'providers' | 'models' | 'search'
  /** The provider whose models show (`models` mode), after the §3.5 single-provider shortcut. */
  provider: string | null
  /** In display order: the first `provider`/`model` row is where the highlight starts. */
  rows: Row[]
}

/** `extra` ids as catalog rows under the `other` provider, appended after `models` (catalog order). */
export function withExtra(models: ModelInfo[], extra: string[] | undefined): ModelInfo[] {
  const add = (extra ?? []).filter((id) => !models.some((m) => m.id === id))
  if (add.length === 0) return models
  return [...models, ...add.map((id) => ({ id, name: id, provider: OTHER, context: 0, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null }))]
}

/** The rows of one picker state (§3.1–§3.3, §3.5); pure, so the levels are unit-testable without React. */
export function viewFor(a: { models: ModelInfo[]; model: string | undefined; provider: string | null; query: string; none?: { label: string }; extra?: string[] }): View {
  const models = withExtra(a.models, a.extra)
  const providers = providersOf(models)
  const current = a.model === undefined ? undefined : models.find((m) => m.id === a.model)
  const currentRow: Row[] = a.model === undefined ? [] : [{ kind: 'current', value: `current:${a.model}`, id: a.model, name: current?.name ?? a.model }]
  const search = searchTerms(a.query).length > 0
  if (search) {
    const results: Row[] = rankModels(models, a.query)
      .filter((m) => m.id !== a.model)
      .map((m) => ({ kind: 'model', value: `model:${m.id}`, m, current: false }))
    const rows = [...currentRow, ...(results.length ? results : [{ kind: 'empty', value: 'empty' } as const])]
    return { mode: 'search', provider: a.provider, rows }
  }
  const provider = a.provider ?? (providers.length === 1 ? providers[0]!.id : null)
  if (provider !== null) {
    const list: Row[] = modelsOfProvider(models, provider).map((m) => ({ kind: 'model', value: `model:${m.id}`, m, current: m.id === a.model }))
    const rows: Row[] = [...(providers.length > 1 ? [{ kind: 'back', value: 'back' } as const] : []), ...list]
    return { mode: 'models', provider, rows }
  }
  const rows: Row[] = [
    ...currentRow,
    ...(a.none ? [{ kind: 'none', value: 'none', label: a.none.label, current: a.model === undefined } as const] : []),
    ...providers.map((p): Row => ({ kind: 'provider', value: `provider:${p.id}`, id: p.id, count: p.count, current: current?.provider === p.id })),
  ]
  return { mode: 'providers', provider: null, rows }
}

/** `200k` for a context window; empty when unknown (0). */
export function contextLabel(context: number): string {
  return context > 0 ? `${Math.round(context / 1000)}k` : ''
}

export type ModelPickerProps = {
  models: ModelInfo[]
  /** The current id (session, draft, or form value); `undefined` → no Current row. */
  model: string | undefined
  onSelect(id: string): void
  /** The current model's effort levels (providers level only), with `clear`. */
  effort?: { levels: string[]; current: string | null | undefined; onSelect(level: string | null): void }
  /** A first row ("Engine default", "Off") that selects `none.value` — forms. */
  none?: { value: string; label: string }
  /** Ids not in `models` (a stale on-file value) listed under `other` so they stay selectable. */
  extra?: string[]
  className?: string
}

export function ModelPicker({ models, model, onSelect, effort, none, extra, className }: ModelPickerProps) {
  const [query, setQuery] = useState('')
  const [provider, setProvider] = useState<string | null>(null)
  const view = viewFor({ models, model, provider, query, none, extra })
  const canGoBack = view.rows.some((r) => r.kind === 'back')
  // cmdk highlights the first DOM item after every search change, mount, and level switch; the §3 start
  // row (first provider / first model / first result) is therefore first in the DOM, and the Current, none
  // and back rows that display above it come last in the DOM with `order-first`. Arrow keys follow the DOM.
  const lead = (row: Row) => row.kind === 'current' || row.kind === 'none' || row.kind === 'back'
  const pick = (row: Row) => {
    switch (row.kind) {
      case 'current':
        return onSelect(row.id)
      case 'none':
        return onSelect(none!.value)
      case 'back':
        return setProvider(null)
      case 'provider':
        return setProvider(row.id)
      case 'model':
        return onSelect(row.m.id)
      case 'empty':
        return
    }
  }
  const item = (row: Row) => {
    switch (row.kind) {
      case 'current':
        return (
          <CommandItem key={row.value} value={row.value} className="order-first" onSelect={() => pick(row)}>
            <span className="truncate">{row.name}</span>
            <Check className="ml-auto size-4" />
          </CommandItem>
        )
      case 'none':
        return (
          <CommandItem key={row.value} value={row.value} className="order-first" onSelect={() => pick(row)}>
            <span className="truncate">{row.label}</span>
            {row.current && <Check className="ml-auto size-4" />}
          </CommandItem>
        )
      case 'back':
        return (
          <CommandItem key={row.value} value={row.value} className="order-first" onSelect={() => pick(row)}>
            <ChevronLeft className="size-4" />
            All providers
          </CommandItem>
        )
      case 'provider':
        return (
          <CommandItem key={row.value} value={row.value} onSelect={() => pick(row)}>
            <span className="truncate">{row.id}</span>
            <span className="ml-auto shrink-0 text-xs text-fg-tertiary">{row.count} models</span>
            {row.current && <Check className="size-4" />}
            <ChevronRight className="size-4 opacity-70" />
          </CommandItem>
        )
      case 'model':
        return (
          <CommandItem key={row.value} value={row.value} onSelect={() => pick(row)}>
            <span className="truncate">{row.m.name}</span>
            <span className="ml-auto shrink-0 text-xs text-fg-tertiary">
              {view.mode === 'search' ? row.m.provider : [contextLabel(row.m.context), row.m.image ? 'vision' : ''].filter(Boolean).join(' · ')}
            </span>
            {row.current && <Check className="size-4" />}
          </CommandItem>
        )
      case 'empty':
        return (
          <CommandItem key={row.value} value={row.value} disabled className="justify-center text-fg-tertiary">
            No model matches.
          </CommandItem>
        )
    }
  }
  const grouped = (kind: Row['kind'], heading: string) => {
    const rows = view.rows.filter((r) => r.kind === kind)
    return rows.length ? <CommandGroup heading={heading}>{rows.map(item)}</CommandGroup> : null
  }
  return (
    <Command shouldFilter={false} className={className}>
      <CommandInput
        placeholder="Search models"
        value={query}
        onValueChange={setQuery}
        onKeyDown={(e) => {
          // The input's own value, not `query`: React state may lag one keystroke behind here.
          if (e.key === 'Backspace' && e.currentTarget.value === '' && canGoBack) {
            e.preventDefault()
            setProvider(null)
          }
        }}
      />
      <CommandList className="max-h-96 [&_[cmdk-list-sizer]]:flex [&_[cmdk-list-sizer]]:flex-col">
        {view.mode === 'models' ? grouped('model', `${view.provider} · ${view.rows.filter((r) => r.kind === 'model').length} models`) : view.mode === 'search' ? view.rows.filter((r) => r.kind !== 'current').map(item) : grouped('provider', 'Providers')}
        {view.rows.filter(lead).map(item)}
        {view.mode === 'providers' && effort && effort.levels.length > 0 && (
          <>
            <CommandSeparator />
            <CommandGroup heading="Effort">
              {effort.levels.map((lvl) => (
                <CommandItem key={lvl} value={`effort:${lvl}`} onSelect={() => effort.onSelect(lvl)}>
                  {lvl}
                  {lvl === effort.current && <Check className="ml-auto size-4" />}
                </CommandItem>
              ))}
              <CommandItem value="effort:clear" onSelect={() => effort.onSelect(null)}>
                clear
                {!effort.current && <Check className="ml-auto size-4" />}
              </CommandItem>
            </CommandGroup>
          </>
        )}
      </CommandList>
    </Command>
  )
}
