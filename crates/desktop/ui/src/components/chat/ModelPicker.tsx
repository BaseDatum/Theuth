// 124 §3: the cmdk body every model picker shares — providers level → a provider's models, with the
// §2 word search over every model whenever the input is non-empty. Ranking is ours (`shouldFilter={false}`).
// 2026-11-11 (Sky): the selected model's effort levels are a chip row directly under its row, wherever it shows.
// 2026-11-17 (Sky): picking a model with effort levels keeps the picker open on its chips, focused; a chip, or a model
// without levels, completes the choice (`onClose`). Outside click, Escape or Tab-away close it with the model kept.

import { Check, ChevronDown, ChevronLeft, ChevronRight, Star } from 'lucide-react'
import { useCommandState } from 'cmdk'
import { type ComponentProps, Fragment, type KeyboardEvent, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Command, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import { cn } from '@/lib/utils'
import { modelsOfProvider, providersOf, rankModels, searchTerms } from '@/lib/model-search'
import { BUILTIN_LABEL } from '@/components/settings/providers'
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"

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

/** A bare catalog row for an id the catalog does not describe (`extra` ids, a harness's model options). */
export const stubModel = (id: string, name: string, provider: string): ModelInfo => ({ id, name, provider, context: 0, reasoning: false, image: false, efforts: [], budget_min: null, budget_max: null, local: false })

/** `extra` ids as catalog rows under the `other` provider, appended after `models` (catalog order). */
export function withExtra(models: ModelInfo[], extra: string[] | undefined): ModelInfo[] {
  const add = (extra ?? []).filter((id) => !models.some((m) => m.id === id))
  if (add.length === 0) return models
  return [...models, ...add.map((id) => stubModel(id, id, OTHER))]
}

/** 177 U4: the providers that serve `local` models. */
export const localProviders = (models: ModelInfo[]): Set<string> => new Set(models.filter((m) => m.local).map((m) => m.provider))

/** The rows of one picker state (§3.1–§3.3, §3.5, 127 favorites); pure, so the levels are unit-testable without React. 194 D5: one ordering everywhere (no `localFirst`); local models keep their Local tag. */
export function viewFor(a: { models: ModelInfo[]; model: string | undefined; provider: string | null; query: string; none?: { label: string }; extra?: string[]; favorites?: string[] }): View {
  const models = withExtra(a.models, a.extra)
  const providers = providersOf(models)
  const favorites = a.favorites ?? []
  const current = a.model === undefined ? undefined : models.find((m) => m.id === a.model)
  const currentRow: Row[] = a.model === undefined ? [] : [{ kind: 'current', value: `current:${a.model}`, id: a.model, name: current?.name ?? a.model, provider: current?.provider }]
  const modelRow = (m: ModelInfo, current: boolean): Row => ({ kind: 'model', value: `model:${m.id}`, m, current, favorite: favorites.includes(m.id) })
  const search = searchTerms(a.query).length > 0
  if (search) {
    const ranked = rankModels(models, a.query).filter((m) => m.id !== a.model)
    const results: Row[] = ranked.map((m) => modelRow(m, false))
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

/** The selected model's effort levels: the chip row under its row. */
export type Effort = {
  levels: readonly string[]
  current: string | null | undefined
  /** `null` is the "Default" chip. */
  onSelect(level: string | null): void
  /** Display names by level (a harness's option names); the level itself otherwise. */
  labels?: Record<string, string>
  /** No "Default" chip: a level is always set (a harness's thought_level option). */
  required?: boolean
}

export type ModelPickerProps = {
  models: ModelInfo[]
  /** The current id (session, draft, or form value); `undefined` → no Current row. */
  model: string | undefined
  onSelect(id: string): void
  /** The choice is complete: an effort chip, or a model without effort levels (one with levels keeps the picker open on its chips). */
  onClose(): void
  /** The current model's effort levels, as chips under its row (no levels → no chips). */
  effort?: Effort
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

/** An effort chip; wraps and grows to a finger's size (44 px) on phones and coarse pointers. A cmdk chip shows the highlight as a ring. */
const CHIP = 'inline-flex h-7 items-center gap-1 rounded-full border border-border px-2.5 py-0 text-xs data-[selected=true]:ring-2 data-[selected=true]:ring-ring max-md:h-11 max-md:px-3.5 max-md:text-sm pointer-coarse:h-11 pointer-coarse:px-3.5 pointer-coarse:text-sm'
/** The plain-button chip (under the Current lead row). */
const CHIP_BUTTON = 'cursor-default outline-hidden select-none hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring'
/** The active level. */
const CHIP_ON = 'border-foreground bg-foreground text-background hover:bg-foreground/85 hover:text-background data-[selected=true]:bg-foreground/85 data-[selected=true]:text-background'

/** 177 U4: the small tag of a model (or provider) served by a local model server. */
export function LocalTag() {
  return <span className="shrink-0 rounded-sm border border-fg-tertiary/40 px-1 text-[10px] leading-4 text-fg-secondary">local</span>
}

/**
 * 215 C1/C4: the quiet model label every model trigger shares — the composer's toolbar (`ComposerModel`) and every form
 * field (`ModelCombobox`): ghost, no border, muted text tinted on hover and while open. The name truncates first; then
 * `· effort`, the local tag and the chevron, which never shrink. The trigger itself shrinks (unlike `Button`'s
 * `shrink-0`) and clips past its last glyph, so in a full row it gives way before any icon button and never pushes one
 * out. Every other prop (id, aria-label, disabled, size, the Radix trigger's state and ref) reaches the `Button`;
 * `className` merges last.
 */
export function QuietModelTrigger({ name, effort, local, className, ...props }: ComponentProps<typeof Button> & { name: string; effort?: string | null; local?: boolean }) {
  return (
    <Button
      variant="ghost"
      {...props}
      className={cn(
        'min-w-0 shrink justify-start gap-1 overflow-hidden rounded-md px-2 text-[13px] font-normal text-fg-tertiary hover:bg-layer-2 hover:text-fg-secondary has-[>svg]:px-2 dark:hover:bg-layer-2 data-[state=open]:bg-layer-2 data-[state=open]:text-fg-secondary',
        className,
      )}
    >
      <span className="min-w-0 truncate">{name}</span>
      {effort && <span className="shrink-0">· {effort}</span>}
      {local && <LocalTag />}
      <ChevronDown className="size-3.5 opacity-60" />
    </Button>
  )
}

export function ModelPicker({ models, model, onSelect, onClose, effort, none, extra, favorites, onToggleFavorite, className, listClassName }: ModelPickerProps) {
  const [query, setQuery] = useState('')
  const [provider, setProvider] = useState<string | null>(null)
  // A model with levels just picked: its chips get focus once the caller shows it as the model (a form may round-trip
  // first). Meanwhile the old model's chips hide, so a level always goes to the model just picked.
  const [chosen, setChosen] = useState<string | null>(null)
  const chipsRef = useRef<HTMLDivElement>(null)
  const awaiting = chosen !== null && chosen !== model
  useEffect(() => {
    if (chosen === null || chosen !== model) return
    const group = chipsRef.current
    // The caller offers no levels for it (a form without an effort setting): the pick alone was the choice.
    if (!group) return onClose()
    ;(group.querySelector<HTMLElement>('[aria-pressed="true"]') ?? group.querySelector<HTMLElement>('button'))?.focus()
  }, [chosen, model])
  const pick = (id: string) => {
    onSelect(id)
    if (models.find((m) => m.id === id)?.efforts.length) setChosen(id)
    else onClose()
  }
  const pickLevel = (lvl: string | null) => {
    effort!.onSelect(lvl)
    onClose()
  }
  // The plain-button chips: arrows step between them, Enter/Space stay theirs (cmdk's root would select its highlighted
  // row on Enter), Tab after a pick leaves = done. Escape reaches the popover/dialog, which closes.
  const chipKeys = (e: KeyboardEvent<HTMLDivElement>) => {
    const all = [...e.currentTarget.querySelectorAll<HTMLElement>('button')]
    const step = ({ ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 } as Record<string, number>)[e.key]
    if (step && all.length) {
      e.preventDefault()
      e.stopPropagation()
      all[(all.indexOf(document.activeElement as HTMLElement) + step + all.length) % all.length]!.focus()
    } else if (e.key === 'Enter' || e.key === ' ') e.stopPropagation()
    else if (e.key === 'Tab' && chosen !== null) {
      e.preventDefault()
      onClose()
    }
  }
  const view = viewFor({ models, model, provider, query, none, extra, favorites })
  const local = localProviders(models)
  const canGoBack = view.rows.some((r) => r.kind === 'back')
  // cmdk 1.1.1 always moves the highlight to the first `[cmdk-item]` in the DOM after a search change
  // (and a controlled `value` cannot veto that: it reports the move through `onValueChange` and keeps
  // it until the prop changes). The §3 start row — first provider / first model / first result — must
  // therefore be the first cmdk item, so the Current, none and back rows above it are plain buttons:
  // clickable and Tab-reachable, outside ↑/↓/Enter. Picking Current is a no-op anyway; back has Backspace.
  // The effort chips follow the row they sit under: plain buttons under Current (Tab, then Enter/Space), cmdk
  // items under a favorite or provider-list row (↑/↓ step through them, Enter applies), never the first cmdk item.
  // Right after a pick they are plain buttons wherever they sit, so the focus can sit on one (arrows, Enter/Space).
  const levels: (string | null)[] = effort && effort.levels.length > 0 && !awaiting ? [...(effort.required ? [] : [null]), ...effort.levels] : []
  const chips = (cmdk: boolean) => {
    const asItems = cmdk && chosen === null
    return (
      effort &&
      levels.length > 0 && (
        <div ref={chipsRef} role="group" aria-label="Effort" className="flex flex-wrap gap-1 pt-0.5 pr-2 pb-1.5 pl-4" onKeyDown={asItems ? undefined : chipKeys}>
          {levels.map((lvl) => {
            const on = lvl === null ? !effort.current : lvl === effort.current
            const value = lvl === null ? 'effort-default' : `effort:${lvl}`
            const body = (
              <>
                {on && <Check className="size-3.5 text-current" />}
                {lvl === null ? 'Default' : (effort.labels?.[lvl] ?? lvl)}
              </>
            )
            return asItems ? (
              <CommandItem key={value} value={value} className={cn(CHIP, on && CHIP_ON)} onSelect={() => pickLevel(lvl)}>
                {body}
              </CommandItem>
            ) : (
              <button key={value} type="button" data-value={value} aria-pressed={on} className={cn(CHIP, CHIP_BUTTON, on && CHIP_ON)} onClick={() => pickLevel(lvl)}>
                {body}
              </button>
            )
          })}
        </div>
      )
    )
  }
  const leadRow = (row: Row) => {
    switch (row.kind) {
      case 'current':
        return (
          <Fragment key={row.value}>
            <button type="button" className={LEAD_ROW} onClick={() => pick(row.id)}>
              <span className="truncate">{row.name}</span>
              {row.provider && local.has(row.provider) && <LocalTag />}
              {row.provider && <span className="ml-auto shrink-0 text-xs text-fg-tertiary">{row.provider}</span>}
              <Check className={row.provider ? undefined : 'ml-auto'} />
            </button>
            {chips(false)}
          </Fragment>
        )
      // "Node default" is a selection too: a form that pairs it with an effort (an agent's runtime) gets the chips under it.
      case 'none':
        return (
          <Fragment key={row.value}>
            <button type="button" className={LEAD_ROW} onClick={() => pick(none!.value)}>
              <span className="truncate">{row.label}</span>
              {row.current && <Check className="ml-auto" />}
            </button>
            {row.current && chips(false)}
          </Fragment>
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
            <span className="truncate">{BUILTIN_LABEL[row.id] ?? row.id}</span>
            {local.has(row.id) && <LocalTag />}
            <span className="ml-auto shrink-0 text-xs text-fg-tertiary">{row.count} models</span>
            {row.current && <Check className="size-4" />}
            <ChevronRight className="size-4 opacity-70" />
          </CommandItem>
        )
      case 'favorite':
        return (
          <Fragment key={row.value}>
            <CommandItem value={row.value} onSelect={() => pick(row.m.id)}>
              <span className="truncate">{row.m.name}</span>
              {row.m.local && <LocalTag />}
              <span className="ml-auto shrink-0 text-xs text-fg-tertiary">{[row.m.provider, contextLabel(row.m.context)].filter(Boolean).join(' · ')}</span>
              {row.current && <Check className="size-4" />}
              {star(row.m.id, true)}
            </CommandItem>
            {row.current && chips(true)}
          </Fragment>
        )
      case 'model':
        return (
          <Fragment key={row.value}>
            <CommandItem value={row.value} onSelect={() => pick(row.m.id)}>
              <span className="truncate">{row.m.name}</span>
              {row.m.local && <LocalTag />}
              <span className="ml-auto shrink-0 text-xs text-fg-tertiary">
                {view.mode === 'search' ? row.m.provider : [contextLabel(row.m.context), row.m.image ? 'vision' : ''].filter(Boolean).join(' · ')}
              </span>
              {row.current && <Check className="size-4" />}
              {star(row.m.id, row.favorite)}
            </CommandItem>
            {row.current && chips(true)}
          </Fragment>
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
        {view.mode === 'models' ? grouped('model', `${BUILTIN_LABEL[view.provider ?? ''] ?? view.provider} · ${view.rows.filter((r) => r.kind === 'model').length} models`) : view.mode === 'search' ? view.rows.map(item) : grouped('provider', 'Providers')}
      </CommandList>
    </Command>
  )
}
