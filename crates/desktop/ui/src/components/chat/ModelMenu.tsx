import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Bot, Check, ChevronDown, Cpu, RotateCcw } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { usePanes } from '@/lib/panes'
import { queryKeys, useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { on, useSessionField } from '@/store/sessions'
import type { ConfigOption } from '@/rpc/bindings/ConfigOption'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { MenuRow } from './HeaderMenu'
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

/** The cmdk group every model picker shows: name, provider, a check on the current one. */
export function ModelItems({ models, model, onSelect }: { models: ModelInfo[]; model: string | undefined; onSelect(id: string): void }) {
  return (
    <CommandGroup heading="Models">
      {models.map((m) => (
        <CommandItem key={m.id} value={`${m.name} ${m.id}`} onSelect={() => onSelect(m.id)}>
          <span className="truncate">{m.name}</span>
          <span className="ml-auto truncate text-xs text-fg-tertiary">{m.provider}</span>
          {m.id === model && <Check className="size-4" />}
        </CommandItem>
      ))}
    </CommandGroup>
  )
}

type Props = {
  session: string | null
}

/** The picker's state shared by the header button, the compact row and the dialog. */
type Model = {
  agent: string | null | undefined
  harnessModel: string | null | undefined
  model: string | undefined
  effort: string | null | undefined
  models: ModelInfo[]
  current: ModelInfo | undefined
  /** The shown name: the model's, else its id. */
  name: string
  setModel(id: string): Promise<void>
  setEffort(level: string | null): Promise<void>
  /** An ACP child's harness options (75 §8); `undefined` until loaded, `null` when the process is gone. */
  harness: ConfigOption[] | null | undefined
  /** Option id → value queued for the turn's end (`set_harness_option` answered `pending`). */
  pending: Record<string, string>
  setHarnessOption(configId: string, value: string): Promise<void>
  /** `resume_child` (75 §10): set on an idle ACP child; restarts its harness in the stored session context. */
  resume: (() => Promise<void>) | null
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
  const model = sessionModel ?? info?.default_model
  // 83 §3: the draft shows the engine's live default effort (`DefaultEffortChanged` keeps it current).
  const effort = session ? sessionEffort : info?.default_effort
  const { data: models = [] } = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models(), enabled: !agent })
  const current = models.find((m) => m.id === model)
  const acp = !!agent && !!session
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
  // 82 §2: a pick here is manual — the engine makes it the default too and `DefaultModelChanged`
  // updates the draft (`dispatch`), on every device attached to this engine.
  const setModel = async (id: string) => {
    done()
    try {
      await api.set_model(session, id, true)
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  // 83 §2: a pick here is manual — it becomes the default for new chats too; in the draft that is all it sets.
  const setEffort = async (level: string | null) => {
    done()
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
      await api.resume_child(session)
      void qc.invalidateQueries({ queryKey: harnessOptionsKey(session) })
      toast('harness resumed in its previous context')
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return {
    agent,
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
  }
}

/** One harness option as a cmdk group (75 §8): the harness's values verbatim, the current one checked, a queued one marked. */
function HarnessGroup({ heading, option, pending, onSelect }: { heading: string; option: ConfigOption; pending: string | undefined; onSelect(value: string): void }) {
  return (
    <CommandGroup heading={heading}>
      {option.options.map((v) => (
        <CommandItem key={v.value} value={`${heading} ${v.name} ${v.value}`} onSelect={() => onSelect(v.value)}>
          <span className="truncate">{v.name}</span>
          {v.value !== v.name && <span className="ml-auto truncate text-xs text-fg-tertiary">{v.value}</span>}
          {v.value === pending ? <span className="text-xs text-fg-tertiary">after this turn</span> : v.value === option.current && <Check className="size-4" />}
        </CommandItem>
      ))}
    </CommandGroup>
  )
}

/** The cmdk body for an ACP child: the harness's model and effort/thinking options (75 §8). */
function HarnessCommand({ m }: { m: Model }) {
  const { model, effort } = harnessGroups(m.harness ?? [])
  return (
    <>
      <CommandInput placeholder="Search harness options" />
      <CommandList className="max-h-96">
        <CommandEmpty>No option matches.</CommandEmpty>
        {model && <HarnessGroup heading="Model" option={model} pending={m.pending[model.id]} onSelect={(v) => void m.setHarnessOption(model.id, v)} />}
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

/** The cmdk body: search, models, then the current model's effort levels. */
function ModelCommand({ m }: { m: Model }) {
  return (
    <>
      <CommandInput placeholder="Search models" />
      <CommandList className="max-h-96">
        <CommandEmpty>No model matches.</CommandEmpty>
        <ModelItems models={m.models} model={m.model} onSelect={(id) => void m.setModel(id)} />
        {m.current && m.current.efforts.length > 0 && (
          <>
            <CommandSeparator />
            <CommandGroup heading="Effort">
              {m.current.efforts.map((lvl) => (
                <CommandItem key={lvl} value={`effort ${lvl}`} onSelect={() => void m.setEffort(lvl)}>
                  {lvl}
                  {lvl === m.effort && <Check className="ml-auto size-4" />}
                </CommandItem>
              ))}
              <CommandItem value="effort clear" onSelect={() => void m.setEffort(null)}>
                clear
                {!m.effort && <Check className="ml-auto size-4" />}
              </CommandItem>
            </CommandGroup>
          </>
        )}
      </CommandList>
    </>
  )
}

/**
 * ChatGPT's header slot: the model name with a cmdk-filtered list from
 * `list_models`, then the selected model's effort levels. A fresh chat keeps
 * effort in the UI until submit creates the session; model changes update the default.
 * An ACP child (75 §8) gets the same picker over its harness's own options —
 * a Model group and, when the harness offers one, an Effort group; with the
 * process gone it shows `omp · <model>` as plain text.
 */
export function ModelMenu(props: Props) {
  const [open, setOpen] = useState(false)
  const m = useModel(props, () => setOpen(false))
  useOpenOnSlash(props.session, () => setOpen(true))
  const resume = m.resume && (
    <Button variant="ghost" size="sm" className="h-7 shrink-0 gap-1 px-2 text-xs text-fg-tertiary" title="restart the harness process in this session's stored context (session/load)" onClick={() => void m.resume?.()}>
      <RotateCcw className="size-3.5" /> Resume
    </Button>
  )
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
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button variant="ghost" className="h-9 max-w-full gap-1 rounded-lg px-2.5 text-lg font-medium text-fg-secondary">
            <span className="min-w-0 truncate">{m.agent ? harnessLabel(m) : m.name}</span>
            {effortLabel && <span className="shrink-0 text-sm font-normal text-fg-tertiary">· {effortLabel}</span>}
            <ChevronDown className="size-4 opacity-70" />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-80 p-0">
          <Command>{m.agent ? <HarnessCommand m={m} /> : <ModelCommand m={m} />}</Command>
        </PopoverContent>
      </Popover>
      {resume}
    </>
  )
}

/** The compact menu's line: the model and effort (opens `ModelDialog`); for an ACP child the harness's model, a picker while its process runs. */
export function ModelRow({ onOpen, ...props }: Props & { onOpen(): void }) {
  const m = useModel(props, onOpen)
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
  const m = useModel(props, () => onOpenChange(false))
  useOpenOnSlash(props.session, () => onOpenChange(true))
  if (m.agent && !m.harness) return null
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title={m.agent ? 'Harness' : 'Model'} description={m.agent ? "Pick from the harness's options" : 'Pick the model and effort'} className="max-w-sm">
      {m.agent ? <HarnessCommand m={m} /> : <ModelCommand m={m} />}
    </CommandDialog>
  )
}
