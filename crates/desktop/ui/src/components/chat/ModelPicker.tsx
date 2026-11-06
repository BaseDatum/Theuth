// 124 §3: the cmdk body every model picker shares — providers level → a provider's models, with the
// §2 word search over every model whenever the input is non-empty. Ranking is ours (`shouldFilter={false}`).

import { Check, ChevronLeft, ChevronRight, Star } from 'lucide-react'
import { useCommandState } from 'cmdk'
import { useState } from 'react'
import { Command, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import { cn } from '@/lib/utils'
import { modelsOfProvider, providersOf, rankModels, searchTerms } from '@/lib/model-search'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'

/** The provider row that lists `extra` ids (on-file values missing from the catalog). */
export const OTHER = 'other'

export type Row =
  /** `provider` is set when the model is in the catalog (incl. `other` for `extra` ids). */
  | { kind: 'current'; value: string; id: string; name: string; provider: string | undefined }
  | { kind: 'none'; value: 'none'; label: string; current: boolean }
  | { kind: 'back'; value: 'back' }
  /** 127: a starred catalog model, providers level only, in favorites order. */
  | { kind: 'favorite'; value: string; m: ModelInfo; current: boolean }
  | { kind: 'provider'; value: string; id: string; count: number; current: boolean }
  | { kind: 'model'; value: string; m: ModelInfo; current: boolean; favorite: boolean }
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
  return [...models, ...add.map((id) => ({ id, name: id, provider: OTHER, context: 0, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null, local: false }))]
}

/** 177 U4: the providers that serve `local` models. */
export const localProviders = (models: ModelInfo[]): Set<string> => new Set(models.filter((m) => m.local).map((m) => m.provider))

/** Stable partition: the rows `isLocal` accepts first (177 U4, the summary pickers). */
const localsFirst = <T,>(rows: T[], isLocal: (r: T) => boolean): T[] => [...rows.filter(isLocal), ...rows.filter((r) => !isLocal(r))]

/** The rows of one picker state (§3.1–§3.3, §3.5, 127 favorites); pure, so the levels are unit-testable without React. `localFirst` (177 U4): local providers and search hits lead. */
export function viewFor(a: { models: ModelInfo[]; model: string | undefined; provider: string | null; query: string; none?: { label: string }; extra?: string[]; favorites?: string[]; localFirst?: boolean }): View {
  const models = withExtra(a.models, a.extra)
  const local = localProviders(models)
  const providers = a.localFirst ? localsFirst(providersOf(models), (p) => local.has(p.id)) : providersOf(models)
  const favorites = a.favorites ?? []
  const current = a.model === undefined ? undefined : models.find((m) => m.id === a.model)
  const currentRow: Row[] = a.model === undefined ? [] : [{ kind: 'current', value: `current:${a.model}`, id: a.model, name: current?.name ?? a.model, provider: current?.provider }]
  const modelRow = (m: ModelInfo, current: boolean): Row => ({ kind: 'model', value: `model:${m.id}`, m, current, favorite: favorites.includes(m.id) })
  const search = searchTerms(a.query).length > 0
  if (search) {
    const ranked = rankModels(models, a.query).filter((m) => m.id !== a.model)
    const results: Row[] = (a.localFirst ? localsFirst(ranked, (m) => m.local) : ranked).map((m) => modelRow(m, false))
    const rows = [...currentRow, ...(results.length ? results : [{ kind: 'empty', value: 'empty' } as const])]
    return { mode: 'search', provider: a.provider, rows }
  }
  const provider = a.provider ?? (providers.length === 1 ? providers[0]!.id : null)
  if (provider !== null) {
    const list: Row[] = modelsOfProvider(models, provider).map((m) => modelRow(m, m.id === a.model))
    const rows: Row[] = [...(providers.length > 1 ? [{ kind: 'back', value: 'back' } as const] : []), ...list]
    return { mode: 'models', provider, rows }
  }
  // 127: favorites first (only those in the catalog, starred order); the Current row only when it is not among them.
  const favoriteRows: Row[] = favorites.flatMap((id) => {
    const m = models.find((x) => x.id === id)
    return m ? [{ kind: 'favorite', value: `fav:${id}`, m, current: id === a.model } as const] : []
  })
  const rows: Row[] = [
    ...favoriteRows,
    ...(favoriteRows.some((r) => r.kind === 'favorite' && r.current) ? [] : currentRow),
    ...(a.none ? [{ kind: 'none', value: 'none', label: a.none.label, current: a.model === undefined } as const] : []),
    ...providers.map((p): Row => ({ kind: 'provider', value: `provider:${p.id}`, id: p.id, count: p.count, current: current?.provider === p.id })),
  ]
  return { mode: 'providers', provider: null, rows }
}

/** The model id a highlighted cmdk value names (`model:` / `fav:` rows), else `null` — the `Alt+Enter` target. */
export function favoriteTarget(value: string): string | null {
  if (value.startsWith('model:')) return value.slice('model:'.length)
  if (value.startsWith('fav:')) return value.slice('fav:'.length)
  return null
}

/** `200k` for a context window; empty when unknown (0). */
export function contextLabel(context: number): string {
  return context > 0 ? `${Math.round(context / 1000)}k` : ''
}

export type ModelPickerProps = {
  /** 177 U4: local providers and local search hits first (the summary-model pickers). */
  localFirst?: boolean
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
  /** 127: starred ids (engine-wide); the Favorites group on the providers level and the filled stars. */
  favorites?: string[]
  /** 127: the star button / `Alt+Enter`; absent → no stars. */
  onToggleFavorite?(id: string): void
  className?: string
  /** Merged over the list's `max-h-96` (the phone dialog makes it fill the visible band, 127 §6). */
  listClassName?: string
}

/** The search input; lives inside `Command` so `Alt+Enter` can read cmdk's highlighted value (`useCommandState`). */
function SearchInput({ query, setQuery, canGoBack, back, onToggleFavorite }: { query: string; setQuery(q: string): void; canGoBack: boolean; back(): void; onToggleFavorite?(id: string): void }) {
  const highlighted = useCommandState((s) => s.value)
  return (
    <CommandInput
      placeholder="Search models"
      value={query}
      onValueChange={setQuery}
      onKeyDown={(e) => {
        // The input's own value, not `query`: React state may lag one keystroke behind here.
        if (e.key === 'Backspace' && e.currentTarget.value === '' && canGoBack) {
          e.preventDefault()
          back()
        }
        // cmdk's root handler selects on any Enter; stop it before it bubbles there.
        const target = e.key === 'Enter' && e.altKey && onToggleFavorite ? favoriteTarget(highlighted) : null
        if (target !== null) {
          e.preventDefault()
          e.stopPropagation()
          onToggleFavorite!(target)
        }
      }}
    />
  )
}

/** The `CommandItem` look for the rows cmdk does not own (`leadRow`). */
const LEAD_ROW = 'flex w-full cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg]:size-4 [&_svg:not([class*=text-])]:text-muted-foreground'

/** 177 U4: the small tag of a model (or provider) served by a local model server. */
export function LocalTag() {
  return <span className="shrink-0 rounded-sm border border-border px-1 text-[10px] leading-4 text-fg-tertiary">local</span>
}

export function ModelPicker({ models, model, onSelect, effort, none, extra, favorites, onToggleFavorite, className, listClassName, localFirst }: ModelPickerProps) {
  const [query, setQuery] = useState('')
  const [provider, setProvider] = useState<string | null>(null)
  const view = viewFor({ models, model, provider, query, none, extra, favorites, localFirst })
  const local = localProviders(models)
  const canGoBack = view.rows.some((r) => r.kind === 'back')
  // cmdk 1.1.1 always moves the highlight to the first `[cmdk-item]` in the DOM after a search change
  // (and a controlled `value` cannot veto that: it reports the move through `onValueChange` and keeps
  // it until the prop changes). The §3 start row — first provider / first model / first result — must
  // therefore be the first cmdk item, so the Current, none and back rows above it are plain buttons:
  // clickable and Tab-reachable, outside ↑/↓/Enter. Picking Current is a no-op anyway; back has Backspace.
  const leadRow = (row: Row) => {
    switch (row.kind) {
      case 'current':
        return (
          <button key={row.value} type="button" className={LEAD_ROW} onClick={() => onSelect(row.id)}>
            <span className="truncate">{row.name}</span>
            {row.provider && local.has(row.provider) && <LocalTag />}
            {row.provider && <span className="ml-auto shrink-0 text-xs text-fg-tertiary">{row.provider}</span>}
            <Check className={row.provider ? undefined : 'ml-auto'} />
          </button>
        )
      case 'none':
        return (
          <button key={row.value} type="button" className={LEAD_ROW} onClick={() => onSelect(none!.value)}>
            <span className="truncate">{row.label}</span>
            {row.current && <Check className="ml-auto" />}
          </button>
        )
      case 'back':
        return (
          <button key={row.value} type="button" className={LEAD_ROW} onClick={() => setProvider(null)}>
            <ChevronLeft />
            All providers
          </button>
        )
      default:
        return null
    }
  }
  // Always visible (phones have no hover); the click must not reach cmdk's item `onClick` (= select).
  const star = (id: string, favorite: boolean) =>
    onToggleFavorite && (
      <button
        type="button"
        aria-label={favorite ? 'Remove from favorites' : 'Add to favorites'}
        className="-my-1 -mr-1 rounded-sm p-1 hover:bg-foreground/10"
        onClick={(e) => {
          e.preventDefault()
          e.stopPropagation()
          onToggleFavorite(id)
        }}
      >
        <Star className={cn('size-4', favorite && 'fill-current text-foreground')} />
      </button>
    )
  const item = (row: Row) => {
    switch (row.kind) {
      case 'provider':
        return (
          <CommandItem key={row.value} value={row.value} onSelect={() => setProvider(row.id)}>
            <span className="truncate">{row.id}</span>
            {local.has(row.id) && <LocalTag />}
            <span className="ml-auto shrink-0 text-xs text-fg-tertiary">{row.count} models</span>
            {row.current && <Check className="size-4" />}
            <ChevronRight className="size-4 opacity-70" />
          </CommandItem>
        )
      case 'favorite':
        return (
          <CommandItem key={row.value} value={row.value} onSelect={() => onSelect(row.m.id)}>
            <span className="truncate">{row.m.name}</span>
            {row.m.local && <LocalTag />}
            <span className="ml-auto shrink-0 text-xs text-fg-tertiary">{[row.m.provider, contextLabel(row.m.context)].filter(Boolean).join(' · ')}</span>
            {row.current && <Check className="size-4" />}
            {star(row.m.id, true)}
          </CommandItem>
        )
      case 'model':
        return (
          <CommandItem key={row.value} value={row.value} onSelect={() => onSelect(row.m.id)}>
            <span className="truncate">{row.m.name}</span>
            {row.m.local && <LocalTag />}
            <span className="ml-auto shrink-0 text-xs text-fg-tertiary">
              {view.mode === 'search' ? row.m.provider : [contextLabel(row.m.context), row.m.image ? 'vision' : ''].filter(Boolean).join(' · ')}
            </span>
            {row.current && <Check className="size-4" />}
            {star(row.m.id, row.favorite)}
          </CommandItem>
        )
      case 'empty':
        return (
          <div key={row.value} className="py-6 text-center text-sm text-fg-tertiary">
            No model matches.
          </div>
        )
      default:
        return null
    }
  }
  const grouped = (kind: Row['kind'], heading: string) => {
    const rows = view.rows.filter((r) => r.kind === kind)
    return rows.length ? <CommandGroup heading={heading}>{rows.map(item)}</CommandGroup> : null
  }
  return (
    <Command shouldFilter={false} className={className}>
      <SearchInput query={query} setQuery={setQuery} canGoBack={canGoBack} back={() => setProvider(null)} onToggleFavorite={onToggleFavorite} />
      <CommandList className={cn('max-h-96', listClassName)}>
        {/* 127: favorites are the first cmdk items, so the highlight starts on them and Enter picks one. */}
        {view.mode === 'providers' && grouped('favorite', 'Favorites')}
        <div className="p-1 empty:hidden">{view.rows.map(leadRow)}</div>
        {view.mode === 'models' ? grouped('model', `${view.provider} · ${view.rows.filter((r) => r.kind === 'model').length} models`) : view.mode === 'search' ? view.rows.map(item) : grouped('provider', 'Providers')}
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
