import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Bot, Brain, ChevronDown, ChevronLeft, ChevronRight, Cpu, Eye, RotateCcw } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { routed } from '@/lib/owner'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useDraftTarget, useDrafts } from '@/lib/drafts'
import { usePanes } from '@/lib/panes'
import { touch } from '@/lib/platform'
import { cn } from '@/lib/utils'
import { queryKeys, useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { on, useSessionField } from '@/store/sessions'
import type { ConfigOption } from "@/rpc/bindings/method.agent.output"
import type { ModelInfo } from "@/rpc/bindings/method.agent.output"
import { useAdvisor, useSetAdvisor } from './advisor'
import { useKnowledgeHarvester, useKnowledgeHarvesterDefault, useSetKnowledgeHarvester } from './knowledge-harvester'
import { MenuRow } from './HeaderMenu'
import { LocalTag, ModelPicker, stubModel } from './ModelPicker'
import { surfaceOf } from './slash'

/** `harness_options(session)` (75 §8); refetched on `HarnessModelChanged` and at every turn end (a queued change lands then). */
const harnessOptionsKey = (session: string) => ['harness-options', session] as const

/** The two options the harness picker shows (75 §8): the `model` select (the rows) and the `thought_level` one (the chips), when offered. */
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

/** 183 §12.2 / §5.6: the catalog of the engine that would run this chat — a remote session's owner, or the new chat's
 * machine — via `list_models(engine)`; this engine's own list otherwise (the shared `queryKeys.models` entry). */
export function useModels(session: string | null, enabled = true): ModelInfo[] {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const canQuery1_2 = useQuerySupport(["list_models"])
  const api = useApi()
  const { data: rows } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canQuery1 && (!!session) })
  const row = session ? rows?.find((r) => r.id === session) : undefined
  const draft = useDraftTarget((s) => s.engine)
  const engine = session ? (row && !row.here ? row.owner : null) : draft
  const { data = [] } = useQuery({ queryKey: engine ? [...queryKeys.models, engine] : queryKeys.models, queryFn: () => api.list_models(engine), enabled: canQuery1_2 && (enabled) })
  return data
}

type Props = {
  session: string | null
  /** 157 D1: this picker's pane is the page's own (a notebook's chat column) — `/model` opens it whatever `usePanes` focuses. */
  own?: boolean
  /** `false`: `/model` never opens this one (a second picker of the same draft, the new-chat page's under the header's). */
  slash?: boolean
}

/** The picker's state shared by the header button, the compact row and the dialog. */
export type Model = {
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
  /** Closes whichever surface showed the picker: the picker's `onClose` (an effort chip, or a model without levels). */
  done(): void
}

/** `done` closes whichever surface showed the picker; the picker calls it once the choice is complete. */
function useModel({ session }: Props, done: () => void): Model {
  const canQuery2 = useQuerySupport(["harness_options"])
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
  const models = useModels(session, !agent && !watching)
  const current = models.find((m) => m.id === model)
  const acp = !!agent && !!session && !watching
  const { data: harness, isError } = useQuery({
    queryKey: harnessOptionsKey(session ?? ''),
    queryFn: () => api.harness_options(session ?? ''),
    enabled: canQuery2 && (acp),
    retry: false,
  })
  const [pending, setPending] = useState<Record<string, string>>({})
  useEffect(() => {
    if (!acp) return
    const refresh = (e: { session: string }) => {
      if (e.session !== session) return
      void qc.invalidateQueries({ queryKey: harnessOptionsKey(session) })
    }
    const offs = [on("agent.events@1", 'HarnessModelChanged', refresh), on("agent.events@1", 'TurnFinished', (e) => (refresh(e), setPending({})))]
    return () => offs.forEach((off) => off())
  }, [acp, session, qc])
  // 148 R1: a pick applies to this chat only. In a session the engine switches that session (`set_model` with a
  // session never touches the default); in the draft it is kept locally until the first send creates the session.
  const setModel = async (id: string) => {
    if (!session) {
      // An effort the new model does not offer would reject the create: fall back to the default.
      const effort = pick.effort && models.find((m) => m.id === id)?.efforts.includes(pick.effort) ? pick.effort : null
      return useDrafts.getState().setPick({ model: id, effort })
    }
    try {
      await routed(session, () => api.set_model(session, id, true))
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const setEffort = async (level: string | null) => {
    if (!session) return useDrafts.getState().setPick({ effort: level })
    try {
      await routed(session, () => api.set_effort(session, level, true))
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const setHarnessOption = async (configId: string, value: string) => {
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

/**
 * 75 §8 through the standard picker: the harness's model options are the rows (provider = the value's `provider/`
 * prefix, else the harness) and its thought_level option the chips under the current one — every value verbatim, no
 * Default chip. A value queued for the turn's end reads `<name> · after this turn`. A harness with no model select gets
 * one row for whatever it runs, so its chips still have a home. Every row carries the thought_level values as its levels,
 * so a pick keeps the picker open on the chips (a queued model counts as the picked one).
 */
export function HarnessPicker({ m, className, listClassName }: { m: Model; className?: string; listClassName?: string }) {
  const { model, effort } = harnessGroups(m.harness ?? [])
  const agent = m.agent ?? 'harness'
  const named = (option: ConfigOption, v: { name: string; value: string }) => (v.value === m.pending[option.id] ? `${v.name} · after this turn` : v.name)
  const levels = effort ? effort.options.map((v) => v.value) : []
  const models = (model ? model.options.map((v) => stubModel(v.value, named(model, v), v.value.includes('/') ? v.value.split('/')[0]! : agent)) : [stubModel(agent, m.harnessModel ?? agent, agent)]).map((r) => ({ ...r, efforts: levels }))
  return (
    <ModelPicker
      models={models}
      model={model ? (m.pending[model.id] ?? model.current) : agent}
      onSelect={(v) => model && void m.setHarnessOption(model.id, v)}
      onClose={m.done}
      effort={
        effort && {
          levels,
          labels: Object.fromEntries(effort.options.map((v) => [v.value, named(effort, v)])),
          current: effort.current,
          required: true,
          onSelect: (v) => v !== null && void m.setHarnessOption(effort.id, v),
        }
      }
      className={className}
      listClassName={listClassName}
    />
  )
}

/** `agent · <current model>` for the header/row; a queued model shows as `→ value (after this turn)`. */
function harnessLabel(m: Model): string {
  const { model } = harnessGroups(m.harness ?? [])
  const current = harnessCurrent(model, model && m.pending[model.id]) ?? m.harnessModel
  return current ? `${m.agent} · ${current}` : (m.agent ?? '')
}

/**
 * Pure: does `/model` (`theuth:open`) address this picker? The focused pane's (a draft pane counts as focused when it
 * is the route's own), or (157 D1) one that is its page's own — a notebook's chat column, which no split pane focuses.
 */
export const slashTarget = (session: string | null, focused: string | null, own: boolean): boolean => own || session === null || focused === session

/** `/model` opens the focused pane's picker (`slashTarget`). */
function useOpenOnSlash({ session, own = false, slash = true }: Props, open: () => void) {
  const focused = usePanes((s) => slash && slashTarget(session, s.focused, own))
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
export function AdvisorCommand({ session, m, onBack, className, listClassName }: { session: string; m: Model; onBack(): void; className?: string; listClassName?: string }) {
  const status = useAdvisor(session)
  const set = useSetAdvisor(session)
  const pending = useDrafts((s) => s.settings.create?.advisor)
  const effort = session ? status?.effort : pending?.[1]
  const model = session ? (status && status.state !== 'off' ? (status.model ?? undefined) : undefined) : pending?.[0]
  const current = m.models.find((x) => x.id === model)
  const pick = (vars: { model: string | null; effort: string | null }) => {
    if (!session) useDrafts.getState().setCreate({ advisor: vars.model ? [vars.model, vars.effort] : null })
    else set.mutate(vars)
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
        onSelect={(id) => pick({ model: id === 'off' ? null : id, effort: id === model ? effort ?? null : null })}
        onClose={m.done}
        effort={current && current.efforts.length > 0 ? { levels: current.efforts, current: effort, onSelect: (lvl) => pick({ model: model ?? null, effort: lvl }) } : undefined}
        favorites={m.favorites}
        onToggleFavorite={(id) => void m.toggleFavorite(id)}
        className={className}
        listClassName={listClassName}
      />
    </div>
  )
}

/** `Advisor · <model name | Off> ›` under the picker; draft choices ride in `create_session`. */
function AdvisorRow({ session, models, onOpen, className }: { session: string; models: ModelInfo[]; onOpen(): void; className?: string }) {
  const status = useAdvisor(session)
  const pending = useDrafts((s) => s.settings.create?.advisor)
  const locked = useDrafts((s) => s.opening !== null || s.settings.create?.retention === 'private')
  if (session && !status) return null
  const model = session ? (status?.state === 'off' ? null : status?.model) : pending?.[0]
  const name = model ? models.find((x) => x.id === model)?.name ?? model : 'Off'
  return (
    <button type="button" disabled={!session && locked} className={cn(LEAD_ROW, 'border-t border-border-subtle disabled:opacity-50', className)} onClick={onOpen}>
      <Eye className="size-4 opacity-70" />
      <span className="truncate">Advisor · {name}</span>
      <ChevronRight className="ml-auto size-4 opacity-70" />
    </button>
  )
}

/**
 * The Knowledge harvester level: the model the knowledge harvest writes memories with. `Default · <name>` (Settings ›
 * Models › Helpers, else the chat's own model) leads; a pick is this chat's own (`set_session_knowledge_harvester`),
 * the lead row resets it. Chips only beside the chat's own pick. The draft keeps its pick for creation.
 */
export function KnowledgeHarvesterCommand({ session, m, onBack, className, listClassName }: { session: string; m: Model; onBack(): void; className?: string; listClassName?: string }) {
  const own = useKnowledgeHarvester(session || null)
  const fallback = useKnowledgeHarvesterDefault()
  const set = useSetKnowledgeHarvester(session)
  const pending = useDrafts((s) => s.settings.knowledgeHarvester)
  const model = session ? (own?.model ?? undefined) : pending?.[0]
  const effort = session ? own?.effort : pending?.[1]
  const current = m.models.find((x) => x.id === model)
  const pick = (vars: { model: string | null; effort: string | null }) => {
    if (!session) useDrafts.getState().setSettings({ knowledgeHarvester: vars.model ? [vars.model, vars.effort] : null })
    else set.mutate(vars)
  }
  return (
    <div className="flex min-h-0 flex-col">
      <button type="button" className={`${LEAD_ROW} border-b border-border-subtle`} onClick={onBack}>
        <ChevronLeft className="size-4 opacity-70" />
        Back
        <span className="ml-auto text-xs text-fg-tertiary">Knowledge harvester</span>
      </button>
      <ModelPicker
        models={m.models}
        model={model}
        none={{ value: 'default', label: `Default · ${knowledgeHarvesterDefaultName(fallback, m.models)}` }}
        extra={model && !current ? [model] : undefined}
        onSelect={(id) => pick({ model: id === 'default' ? null : id, effort: id === model ? (effort ?? null) : null })}
        onClose={m.done}
        effort={current && current.efforts.length > 0 ? { levels: current.efforts, current: effort, onSelect: (lvl) => pick({ model: model ?? null, effort: lvl }) } : undefined}
        favorites={m.favorites}
        onToggleFavorite={(id) => void m.toggleFavorite(id)}
        className={className}
        listClassName={listClassName}
      />
    </div>
  )
}

/** The default's shown name, the same in the row and the level: the Helpers pick, else the chat's own model. */
const knowledgeHarvesterDefaultName = (fallback: ModelInfo | null | undefined, models: ModelInfo[]): string =>
  fallback ? (models.find((x) => x.id === fallback.id)?.name ?? fallback.name ?? fallback.id) : 'chat model'

/** `Knowledge harvester · <name>` under the picker, `default` when the chat has no pick of its own. */
function KnowledgeHarvesterRow({ session, models, onOpen, className }: { session: string; models: ModelInfo[]; onOpen(): void; className?: string }) {
  const supported = useQuerySupport(['session_knowledge_harvester', 'set_session_knowledge_harvester', 'knowledge_harvester_model'])
  const own = useKnowledgeHarvester(session || null)
  const fallback = useKnowledgeHarvesterDefault()
  const pending = useDrafts((s) => s.settings.knowledgeHarvester)
  if (!supported || (session && !own)) return null
  const model = session ? own?.model : pending?.[0]
  const name = model ? (models.find((x) => x.id === model)?.name ?? model) : knowledgeHarvesterDefaultName(fallback, models)
  return (
    <button type="button" className={cn(LEAD_ROW, 'border-t border-border-subtle', className)} onClick={onOpen}>
      <Brain className="size-4 opacity-70" />
      <span className="truncate">Knowledge harvester · {name}</span>
      {!model && <span className="shrink-0 text-xs text-fg-tertiary">default</span>}
      <ChevronRight className="ml-auto size-4 opacity-70" />
    </button>
  )
}

/** The cmdk body: providers → models, word search, the current model's effort chips under its row (124 §3); the Advisor (125 §8.1) and Knowledge harvester rows below. */
function ModelCommand({ m, session, advisor, className, listClassName, footerClassName }: { m: Model; session: string | null; advisor: boolean; className?: string; listClassName?: string; footerClassName?: string }) {
  const [level, setLevel] = useState<'model' | 'advisor' | 'knowledgeHarvester'>(advisor ? 'advisor' : 'model')
  if (level === 'advisor') return <AdvisorCommand session={session ?? ''} m={m} onBack={() => setLevel('model')} className={className} listClassName={listClassName} />
  if (level === 'knowledgeHarvester') return <KnowledgeHarvesterCommand session={session ?? ''} m={m} onBack={() => setLevel('model')} className={className} listClassName={listClassName} />
  return (
    <>
      <ModelPicker
        models={m.models}
        model={m.model}
        onSelect={(id) => void m.setModel(id)}
        onClose={m.done}
        effort={m.current && m.current.efforts.length > 0 ? { levels: m.current.efforts, current: m.effort, onSelect: (lvl) => void m.setEffort(lvl) } : undefined}
        favorites={m.favorites}
        onToggleFavorite={(id) => void m.toggleFavorite(id)}
        className={className}
        listClassName={listClassName}
      />
      <AdvisorRow session={session ?? ''} models={m.models} onOpen={() => setLevel('advisor')} className={footerClassName} />
      <KnowledgeHarvesterRow session={session ?? ''} models={m.models} onOpen={() => setLevel('knowledgeHarvester')} className={footerClassName} />
    </>
  )
}

/** The picker in 127 §6's dialog (top of the band on touch): the compact header's, and the header's on touch (194 D1). */
function PickerDialogContent({ m, session, advisor }: { m: Model; session: string | null; advisor: boolean }) {
  const place = dialogPlacement(touch())
  const picker = cn('[&_[cmdk-input]]:h-12 **:data-[slot=command-input-wrapper]:h-12', place.picker)
  return (
    <DialogContent className={cn('max-w-sm overflow-hidden p-0', place.content)}>
      <DialogHeader className="sr-only">
        <DialogTitle>{m.agent ? 'Harness' : 'Model'}</DialogTitle>
        <DialogDescription>{m.agent ? "Pick from the harness's options" : 'Pick the model and effort'}</DialogDescription>
      </DialogHeader>
      {m.agent ? <HarnessPicker m={m} className={picker} listClassName={place.list} /> : <ModelCommand m={m} session={session} advisor={advisor} className={picker} listClassName={place.list} footerClassName={place.footer} />}
    </DialogContent>
  )
}

/**
 * ChatGPT's header slot: the model name with a cmdk-filtered list from
 * `list_models`, the selected model's effort levels as chips under its row. A fresh chat keeps its
 * model and effort pick in the UI until submit creates the session (148 R1: never the default).
 * An ACP child (75 §8) gets the same picker over its harness's own options (`HarnessPicker`);
 * with the process gone it shows `omp · <model>` as plain text.
 * `small` (157 D1): the 28 px, text-xs trigger of a column header (the notebook chat) instead of the 36 px headline.
 * 194 D1: on touch it opens the top-of-band dialog (as `ModelCombobox` does), not the anchored popover.
 */
export function ModelMenu({ small = false, ...props }: Props & { small?: boolean }) {
  const [open, setOpen] = useState(false)
  // 125 §8.2: opened from the pill → the Advisor level first; a normal open starts on the models level.
  const [advisor, setAdvisor] = useState(false)
  const m = useModel(props, () => setOpen(false))
  useOpenOnSlash(props, () => setOpen(true))
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
  const onOpenChange = (o: boolean) => (setOpen(o), !o && setAdvisor(false))
  const trigger = (
    <Button variant="ghost" data-tour={small ? undefined : 'chat-model'} className={small ? 'h-7 max-w-full gap-1 rounded-md px-2 text-xs font-medium text-fg-secondary' : 'h-9 max-w-full gap-1 rounded-lg px-2.5 text-lg font-medium text-fg-secondary'}>
      <span className="min-w-0 truncate">{m.agent ? harnessLabel(m) : m.name}</span>
      {effortLabel && <span className={cn('shrink-0 font-normal text-fg-tertiary', small ? 'text-[11px]' : 'text-sm')}>· {effortLabel}</span>}
      {!m.agent && m.current?.local && <LocalTag />}
      <ChevronDown className={small ? 'size-3.5 opacity-70' : 'size-4 opacity-70'} />
    </Button>
  )
  return (
    <>
      {touch() ? (
        <Dialog open={open} onOpenChange={onOpenChange}>
          <DialogTrigger asChild>{trigger}</DialogTrigger>
          <PickerDialogContent m={m} session={props.session} advisor={advisor} />
        </Dialog>
      ) : (
        <Popover open={open} onOpenChange={onOpenChange}>
          <PopoverTrigger asChild>{trigger}</PopoverTrigger>
          <PopoverContent align="start" className="w-80 p-0">
            {m.agent ? <HarnessPicker m={m} /> : <ModelCommand m={m} session={props.session} advisor={advisor} />}
          </PopoverContent>
        </Popover>
      )}
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
  const text = m.effort ? `${m.name} · ${m.effort}` : m.name
  return <MenuRow icon={Cpu} label="Model" detail={m.current?.local ? <span className="inline-flex items-center gap-1">{text}<LocalTag /></span> : text} onSelect={onOpen} />
}

/** The same picker as a dialog: the compact header has no model button to anchor a popover to. */
export function ModelDialog({ open, onOpenChange, ...props }: Props & { open: boolean; onOpenChange(o: boolean): void }) {
  const [advisor, setAdvisor] = useState(false)
  const m = useModel(props, () => onOpenChange(false))
  useOpenOnSlash(props, () => onOpenChange(true))
  useOpenAdvisor(props.session, () => (setAdvisor(true), onOpenChange(true)))
  if (m.watching || (m.agent && !m.harness)) return null
  return (
    <Dialog open={open} onOpenChange={(o) => (onOpenChange(o), !o && setAdvisor(false))}>
      <PickerDialogContent m={m} session={props.session} advisor={advisor} />
    </Dialog>
  )
}
