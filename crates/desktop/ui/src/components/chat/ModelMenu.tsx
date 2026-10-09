import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Bot, Check, ChevronDown, ChevronLeft, ChevronRight, Cpu, Eye, RotateCcw } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Command, CommandDialog, CommandEmpty, type CommandFilter, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { defaultFilter } from 'cmdk'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useDrafts } from '@/lib/drafts'
import { usePanes } from '@/lib/panes'
import { touch } from '@/lib/platform'
import { cn } from '@/lib/utils'
import { queryKeys, useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { on, useSessionField } from '@/store/sessions'
import type { ConfigOption } from '@/rpc/bindings/ConfigOption'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { useAdvisor, useSetAdvisor } from './advisor'
import { MenuRow } from './HeaderMenu'
import { ModelPicker } from './ModelPicker'
import { surfaceOf } from './slash'

/** `harness_options(session)` (75 §8); refetched on `HarnessModelChanged` and at every turn end (a queued change lands then). */
const harnessOptionsKey = (session: string) => ['harness-options', session] as const

/** The two option groups the harness picker shows (75 §8): the `model` select and the `thought_level` one, when offered. */
export function harnessGroups(options: readonly ConfigOption[]): { model?: ConfigOption; effort?: ConfigOption } {
  return {
    model: options.find((o) => o.category === 'model') ?? options.find((o) => /model/i.test(o.id) || /model/i.test(o.name)),
    effort: options.find((o) => o.category === 'thought_level'),
  }
}

/** The label of `option`'s current value (its name, else the raw value); `pending` overrides it as `→ value (after this turn)`. */
export function harnessCurrent(option: ConfigOption | undefined, pending: string | undefined): string | undefined {
  if (!option) return undefined
  if (pending !== undefined) return `→ ${pending} (after this turn)`
  return option.options.find((v) => v.value === option.current)?.name ?? option.current
}

/** The cmdk value of a harness option row (`HarnessGroup`); `pinnedFilter` compares against it. */
export function harnessItemValue(heading: string, v: { name: string; value: string }): string {
  return `${heading} ${v.name} ${v.value}`
}

/** `items` with the one `is` picks first, the rest in their given order; unchanged when none matches. */
function pinFirst<T>(items: T[], is: (item: T) => boolean): T[] {
  const i = items.findIndex(is)
  return i <= 0 ? items : [...items.slice(i, i + 1), ...items.slice(0, i), ...items.slice(i + 1)]
}

const norm = (s: string) => s.trim().toLowerCase()

/** cmdk's `defaultFilter`, except the row whose value is `pinned` always scores 1: it stays first and no search hides it. */
export function pinnedFilter(pinned: string | undefined): CommandFilter {
  const p = pinned === undefined ? undefined : norm(pinned)
  return (value, search, keywords) => (p !== undefined && norm(value) === p ? 1 : defaultFilter(value, search, keywords))
}

/**
 * 127 §6: a phone's keyboard hides the lower half of a centred dialog (the search input focuses on open), so on
 * touch the dialog hugs the top of the visible band and its list fills the rest; the `--visible-band` variable is
 * the visual-viewport height `lib/viewport.ts` maintains (page scroll undone, so a top-anchored `fixed` element sits
 * at the top of the band). Desktop / iPad with a hardware keyboard stay centred.
 * 125 F2: the `Command` root is `h-full`, which inside the band-capped column claimed the whole band and pushed the
 * Advisor row under the keyboard; `picker` lets it shrink and `footer` pins the row as the band's last row.
 */
export function dialogPlacement(touch: boolean): { content: string; list: string; picker: string; footer: string } {
  if (!touch) return { content: '', list: '', picker: '', footer: '' }
  return {
    content: 'top-[max(0.5rem,env(safe-area-inset-top))] translate-y-0 flex max-h-[calc(var(--visible-band)-1rem)] flex-col',
    list: 'max-h-none min-h-0 flex-1',
    picker: 'h-auto min-h-0 flex-1',
    footer: 'shrink-0',
  }
}

/** 127 §4: the engine-wide favorites and the toggle every picker surface shares (header, dialog, forms). */
export function useFavorites(): { favorites: string[]; toggleFavorite(id: string): Promise<void> } {
  const api = useApi()
  const info = useConnection((s) => s.info)
  const favorites = info?.favorite_models ?? []
  const toggleFavorite = async (id: string) => {
    try {
      const models = await api.set_model_favorite(id, !favorites.includes(id))
      const conn = useConnection.getState()
      if (conn.info) conn.setInfo({ ...conn.info, favorite_models: models })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return { favorites, toggleFavorite }
}

type Props = {
  session: string | null
}

/** The picker's state shared by the header button, the compact row and the dialog. */
type Model = {
  agent: string | null | undefined
  /** 141: a watch-only harness subagent — the label is the harness's model (or `—`), nothing opens. */
  watching: boolean
  harnessModel: string | null | undefined
  model: string | undefined
  effort: string | null | undefined
  models: ModelInfo[]
  current: ModelInfo | undefined
  /** The shown name: the model's, else its id. */
  name: string
  setModel(id: string): Promise<void>
  setEffort(level: string | null): Promise<void>
  favorites: string[]
  /** Star / unstar `id` (127); the menu stays open, the list re-renders from the reply and the event. */
  toggleFavorite(id: string): Promise<void>
  /** An ACP child's harness options (75 §8); `undefined` until loaded, `null` when the process is gone. */
  harness: ConfigOption[] | null | undefined
  /** Option id → value queued for the turn's end (`set_harness_option` answered `pending`). */
  pending: Record<string, string>
  setHarnessOption(configId: string, value: string): Promise<void>
  /** `resume_child` (75 §10): set on an idle ACP child; restarts its harness in the stored session context. */
  resume: (() => Promise<void>) | null
  /** Closes whichever surface showed the picker (the `useModel` argument), for picks made outside `setModel`/`setEffort`. */
  done(): void
}

/** `done` runs after a choice (closes whichever surface showed the picker). */
function useModel({ session }: Props, done: () => void): Model {
  const api = useApi()
  const qc = useQueryClient()
  const info = useConnection((s) => s.info)
  const sessionModel = useSessionField(session ?? '', 'model')
  const sessionEffort = useSessionField(session ?? '', 'effort')
  const agent = useSessionField(session ?? '', 'agent')
  const harnessModel = useSessionField(session ?? '', 'harnessModel')
  const running = useSessionField(session ?? '', 'running')
  // 141 §9.2(d): a watch-only child is the harness's — its model is shown, never picked; no `harness_options`/`resume_child` calls.
  const watching = useSessionField(session ?? '', 'kind') === 'observed'
  // 148 R1: the draft shows its own pick, else the engine's live defaults (`DefaultModelChanged` / `DefaultEffortChanged` keep them current).
  const pick = useDrafts((s) => s.pick)
  const model = (session ? sessionModel : pick.model) ?? info?.default_model
  const effort = session ? sessionEffort : (pick.effort ?? info?.default_effort)
  const { data: models = [] } = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models(), enabled: !agent && !watching })
  const current = models.find((m) => m.id === model)
  const acp = !!agent && !!session && !watching
  const { data: harness, isError } = useQuery({
    queryKey: harnessOptionsKey(session ?? ''),
    queryFn: () => api.harness_options(session ?? ''),
    enabled: acp,
    retry: false,
  })
  const [pending, setPending] = useState<Record<string, string>>({})
  useEffect(() => {
    if (!acp) return
    const refresh = (e: { session: string }) => {
      if (e.session !== session) return
      void qc.invalidateQueries({ queryKey: harnessOptionsKey(session) })
    }
    const offs = [on('HarnessModelChanged', refresh), on('TurnFinished', (e) => (refresh(e), setPending({})))]
    return () => offs.forEach((off) => off())
  }, [acp, session, qc])
  // 148 R1: a pick applies to this chat only. In a session the engine switches that session (`set_model` with a
  // session never touches the default); in the draft it is kept locally until the first send creates the session.
  const setModel = async (id: string) => {
    done()
    if (!session) {
      // An effort the new model does not offer would reject the create: fall back to the default.
      const effort = pick.effort && models.find((m) => m.id === id)?.efforts.includes(pick.effort) ? pick.effort : null
      return useDrafts.getState().setPick({ model: id, effort })
    }
    try {
      await api.set_model(session, id, true)
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const setEffort = async (level: string | null) => {
    done()
    if (!session) return useDrafts.getState().setPick({ effort: level })
    try {
      await api.set_effort(session, level, true)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const setHarnessOption = async (configId: string, value: string) => {
    done()
    if (!session) return
    try {
      const r = await api.set_harness_option(session, configId, value)
      qc.setQueryData(harnessOptionsKey(session), r.options)
      if (r.pending) {
        setPending((p) => ({ ...p, [configId]: value }))
        toast(`${configId} → ${value} after this turn`)
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const resume = async () => {
    if (!session) return
    try {
      const r = await api.resume_child(session)
      void qc.invalidateQueries({ queryKey: harnessOptionsKey(session) })
      // 146 D8: the engine says whether there was interrupted work to continue.
      toast(r === 'prompted' ? 'resuming the interrupted task' : 'reconnected; the harness was idle, nothing to continue')
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const { favorites, toggleFavorite } = useFavorites()
  return {
    favorites,
    toggleFavorite,
    agent,
    watching,
    harnessModel,
    model,
    effort,
    models,
    current,
    setModel,
    setEffort,
    name: current?.name ?? model ?? 'Model',
    harness: isError ? null : harness,
    pending,
    setHarnessOption,
    resume: acp && !running ? resume : null,
    done,
  }
}

/** One harness option as a cmdk group (75 §8): the harness's values verbatim, the current one checked (first, for the Model group), a queued one marked. */
function HarnessGroup({ heading, option, pending, pin, onSelect }: { heading: string; option: ConfigOption; pending: string | undefined; pin?: boolean; onSelect(value: string): void }) {
  const options = pin ? pinFirst(option.options, (v) => v.value === option.current) : option.options
  return (
    <CommandGroup heading={heading}>
      {options.map((v) => (
        <CommandItem key={v.value} value={harnessItemValue(heading, v)} forceMount={(pin && v.value === option.current) || undefined} onSelect={() => onSelect(v.value)}>
          <span className="truncate">{v.name}</span>
          {v.value !== v.name && <span className="ml-auto truncate text-xs text-fg-tertiary">{v.value}</span>}
          {v.value === pending ? <span className="text-xs text-fg-tertiary">after this turn</span> : v.value === option.current && <Check className="size-4" />}
        </CommandItem>
      ))}
    </CommandGroup>
  )
}

/** The cmdk body for an ACP child: the harness's model and effort/thinking options (75 §8). */
function HarnessCommand({ m, listClassName }: { m: Model; listClassName?: string }) {
  const { model, effort } = harnessGroups(m.harness ?? [])
  return (
    <>
      <CommandInput placeholder="Search harness options" />
      <CommandList className={cn('max-h-96', listClassName)}>
        <CommandEmpty>No option matches.</CommandEmpty>
        {model && <HarnessGroup heading="Model" option={model} pending={m.pending[model.id]} pin onSelect={(v) => void m.setHarnessOption(model.id, v)} />}
        {model && effort && <CommandSeparator />}
        {effort && <HarnessGroup heading="Effort" option={effort} pending={m.pending[effort.id]} onSelect={(v) => void m.setHarnessOption(effort.id, v)} />}
      </CommandList>
    </>
  )
}

/** `agent · <current model>` for the header/row; a queued model shows as `→ value (after this turn)`. */
function harnessLabel(m: Model): string {
  const { model } = harnessGroups(m.harness ?? [])
  const current = harnessCurrent(model, model && m.pending[model.id]) ?? m.harnessModel
  return current ? `${m.agent} · ${current}` : (m.agent ?? '')
}

/** The cmdk value `pinnedFilter` keeps on top: an ACP child's current harness model option. */
function pinnedValue(m: Model): string | undefined {
  const { model } = harnessGroups(m.harness ?? [])
  const current = model?.options.find((v) => v.value === model.current)
  return current && harnessItemValue('Model', current)
}

/** `/model` opens the focused pane's picker (a draft pane counts as focused when it is the route's own). */
function useOpenOnSlash(session: string | null, open: () => void) {
  const focused = usePanes((s) => session === null || s.focused === session)
  const latest = useRef(open)
  latest.current = open
  useEffect(() => {
    if (!focused) return
    const on = (e: Event) => surfaceOf(e) === 'model' && latest.current()
    window.addEventListener('theuth:open', on)
    return () => window.removeEventListener('theuth:open', on)
  }, [focused])
}

/** 125 §8.2: the pill's `Change model…` opens this session's picker on the Advisor level. */
export const openAdvisorPicker = (session: string) => window.dispatchEvent(new CustomEvent('theuth:open-advisor', { detail: { session } }))

function useOpenAdvisor(session: string | null, open: () => void) {
  const latest = useRef(open)
  latest.current = open
  useEffect(() => {
    if (!session) return
    const on = (e: Event) => (e as CustomEvent<{ session: string }>).detail.session === session && latest.current()
    window.addEventListener('theuth:open-advisor', on)
    return () => window.removeEventListener('theuth:open-advisor', on)
  }, [session])
}

/** The plain-button look of 124's lead rows, for the rows outside the picker's own `Command`. */
const LEAD_ROW = 'flex w-full cursor-default items-center gap-2 px-3 py-2 text-sm outline-hidden select-none hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent'

/**
 * 125 §8.1: the Advisor level — 124's picker over the same catalog with `Off` as the lead row and the advisor model's
 * effort levels; a pick calls `set_advisor` (never `set_model`, so the session's own default is untouched).
 */
function AdvisorCommand({ session, m, onBack, className, listClassName }: { session: string; m: Model; onBack(): void; className?: string; listClassName?: string }) {
  const status = useAdvisor(session)
  const set = useSetAdvisor(session)
  const model = status && status.state !== 'off' ? (status.model ?? undefined) : undefined
  const current = m.models.find((x) => x.id === model)
  const pick = (vars: { model: string | null; effort: string | null }) => {
    m.done()
    set.mutate(vars)
  }
  return (
    <div className="flex min-h-0 flex-col">
      <button type="button" className={`${LEAD_ROW} border-b border-border-subtle`} onClick={onBack}>
        <ChevronLeft className="size-4 opacity-70" />
        Back
        <span className="ml-auto text-xs text-fg-tertiary">Advisor</span>
      </button>
      <ModelPicker
        models={m.models}
        model={model}
        none={{ value: 'off', label: 'Off' }}
        extra={model && !current ? [model] : undefined}
        onSelect={(id) => pick({ model: id === 'off' ? null : id, effort: status?.effort ?? null })}
        effort={current && current.efforts.length > 0 ? { levels: current.efforts, current: status?.effort, onSelect: (lvl) => pick({ model: model ?? null, effort: lvl }) } : undefined}
        favorites={m.favorites}
        onToggleFavorite={(id) => void m.toggleFavorite(id)}
        className={className}
        listClassName={listClassName}
      />
    </div>
  )
}

/** `Advisor · <model name | Off> ›` under the picker; nothing for the draft (a new chat starts with no advisor, 125 ruling 2). */
function AdvisorRow({ session, models, onOpen, className }: { session: string; models: ModelInfo[]; onOpen(): void; className?: string }) {
  const status = useAdvisor(session)
  if (!status) return null
  const name = status.state === 'off' || !status.model ? 'Off' : (models.find((x) => x.id === status.model)?.name ?? status.model)
  return (
    <button type="button" className={cn(LEAD_ROW, 'border-t border-border-subtle', className)} onClick={onOpen}>
      <Eye className="size-4 opacity-70" />
      <span className="truncate">Advisor · {name}</span>
      <ChevronRight className="ml-auto size-4 opacity-70" />
    </button>
  )
}

/** The cmdk body: providers → models, word search, then the current model's effort levels (124 §3); the Advisor row below (125 §8.1). */
function ModelCommand({ m, session, advisor, className, listClassName, footerClassName }: { m: Model; session: string | null; advisor: boolean; className?: string; listClassName?: string; footerClassName?: string }) {
  const [level, setLevel] = useState<'model' | 'advisor'>(advisor ? 'advisor' : 'model')
  if (level === 'advisor' && session) return <AdvisorCommand session={session} m={m} onBack={() => setLevel('model')} className={className} listClassName={listClassName} />
  return (
    <>
      <ModelPicker
        models={m.models}
        model={m.model}
        onSelect={(id) => void m.setModel(id)}
        effort={m.current && m.current.efforts.length > 0 ? { levels: m.current.efforts, current: m.effort, onSelect: (lvl) => void m.setEffort(lvl) } : undefined}
        favorites={m.favorites}
        onToggleFavorite={(id) => void m.toggleFavorite(id)}
        className={className}
        listClassName={listClassName}
      />
      {session && <AdvisorRow session={session} models={m.models} onOpen={() => setLevel('advisor')} className={footerClassName} />}
    </>
  )
}

/**
 * ChatGPT's header slot: the model name with a cmdk-filtered list from
 * `list_models`, then the selected model's effort levels. A fresh chat keeps its
 * model and effort pick in the UI until submit creates the session (148 R1: never the default).
 * An ACP child (75 §8) gets the same picker over its harness's own options —
 * a Model group and, when the harness offers one, an Effort group; with the
 * process gone it shows `omp · <model>` as plain text.
 */
export function ModelMenu(props: Props) {
  const [open, setOpen] = useState(false)
  // 125 §8.2: opened from the pill → the Advisor level first; a normal open starts on the models level.
  const [advisor, setAdvisor] = useState(false)
  const m = useModel(props, () => setOpen(false))
  useOpenOnSlash(props.session, () => setOpen(true))
  useOpenAdvisor(props.session, () => (setAdvisor(true), setOpen(true)))
  const resume = m.resume && (
    <Button variant="ghost" size="sm" className="h-7 shrink-0 gap-1 px-2 text-xs text-fg-tertiary" title="restart the harness process in this session's stored context (session/load)" onClick={() => void m.resume?.()}>
      <RotateCcw className="size-3.5" /> Resume
    </Button>
  )
  if (m.watching) {
    return (
      <span className="flex h-9 max-w-full items-center px-2.5 text-lg font-medium text-fg-secondary" title="the harness runs this subagent; its model is not yours to pick" aria-disabled>
        <span className="min-w-0 truncate">{m.harnessModel ?? '—'}</span>
      </span>
    )
  }
  if (m.agent && !m.harness) {
    return (
      <span className="flex h-9 max-w-full items-center gap-1 px-2.5 text-lg font-medium text-fg-secondary" title={m.harness === null ? 'harness process not running' : 'External harness'}>
        <span className="min-w-0 truncate">{harnessLabel(m)}</span>
        {resume}
      </span>
    )
  }
  const { effort: harnessEffort } = harnessGroups(m.harness ?? [])
  const effortLabel = m.agent ? harnessCurrent(harnessEffort, harnessEffort && m.pending[harnessEffort.id]) : m.effort
  return (
    <>
      <Popover open={open} onOpenChange={(o) => (setOpen(o), !o && setAdvisor(false))}>
        <PopoverTrigger asChild>
          <Button variant="ghost" className="h-9 max-w-full gap-1 rounded-lg px-2.5 text-lg font-medium text-fg-secondary">
            <span className="min-w-0 truncate">{m.agent ? harnessLabel(m) : m.name}</span>
            {effortLabel && <span className="shrink-0 text-sm font-normal text-fg-tertiary">· {effortLabel}</span>}
            <ChevronDown className="size-4 opacity-70" />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-80 p-0">
          {m.agent ? (
            <Command filter={pinnedFilter(pinnedValue(m))}>
              <HarnessCommand m={m} />
            </Command>
          ) : (
            <ModelCommand m={m} session={props.session} advisor={advisor} />
          )}
        </PopoverContent>
      </Popover>
      {resume}
    </>
  )
}

/** The compact menu's line: the model and effort (opens `ModelDialog`); for an ACP child the harness's model, a picker while its process runs. */
export function ModelRow({ onOpen, ...props }: Props & { onOpen(): void }) {
  const m = useModel(props, onOpen)
  if (m.watching) return <MenuRow icon={Eye} label="Model" detail={m.harnessModel ?? '—'} />
  if (m.agent) {
    const { effort } = harnessGroups(m.harness ?? [])
    const lvl = harnessCurrent(effort, effort && m.pending[effort.id])
    const detail = lvl ? `${harnessLabel(m)} · ${lvl}` : harnessLabel(m)
    return <MenuRow icon={Bot} label="Harness" detail={detail} onSelect={m.harness ? onOpen : undefined} />
  }
  return <MenuRow icon={Cpu} label="Model" detail={m.effort ? `${m.name} · ${m.effort}` : m.name} onSelect={onOpen} />
}

/** The same picker as a dialog: the compact header has no model button to anchor a popover to. */
export function ModelDialog({ open, onOpenChange, ...props }: Props & { open: boolean; onOpenChange(o: boolean): void }) {
  const [advisor, setAdvisor] = useState(false)
  const m = useModel(props, () => onOpenChange(false))
  useOpenOnSlash(props.session, () => onOpenChange(true))
  useOpenAdvisor(props.session, () => (setAdvisor(true), onOpenChange(true)))
  if (m.watching || (m.agent && !m.harness)) return null
  const place = dialogPlacement(touch())
  if (m.agent) {
    return (
      <CommandDialog open={open} onOpenChange={onOpenChange} filter={pinnedFilter(pinnedValue(m))} title="Harness" description="Pick from the harness's options" className={cn('max-w-sm', place.content)}>
        <HarnessCommand m={m} listClassName={place.list} />
      </CommandDialog>
    )
  }
  return (
    <Dialog open={open} onOpenChange={(o) => (onOpenChange(o), !o && setAdvisor(false))}>
      <DialogHeader className="sr-only">
        <DialogTitle>Model</DialogTitle>
        <DialogDescription>Pick the model and effort</DialogDescription>
      </DialogHeader>
      <DialogContent className={cn('max-w-sm overflow-hidden p-0', place.content)}>
        <ModelCommand m={m} session={props.session} advisor={advisor} className={cn('[&_[cmdk-input]]:h-12 **:data-[slot=command-input-wrapper]:h-12', place.picker)} listClassName={place.list} footerClassName={place.footer} />
      </DialogContent>
    </Dialog>
  )
}
