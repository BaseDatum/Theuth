import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Bot, Check, ChevronDown, Cpu } from 'lucide-react'
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
import { useSessionField } from '@/store/sessions'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { MenuRow } from './HeaderMenu'
import { surfaceOf } from './slash'

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
  draftEffort?: string | null
  onDraftEffort?(effort: string | null): void
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
}

/** `done` runs after a choice (closes whichever surface showed the picker). */
function useModel({ session, draftEffort, onDraftEffort }: Props, done: () => void): Model {
  const api = useApi()
  const qc = useQueryClient()
  const info = useConnection((s) => s.info)
  const sessionModel = useSessionField(session ?? '', 'model')
  const sessionEffort = useSessionField(session ?? '', 'effort')
  const agent = useSessionField(session ?? '', 'agent')
  const harnessModel = useSessionField(session ?? '', 'harnessModel')
  const model = sessionModel ?? info?.default_model
  const effort = session ? sessionEffort : draftEffort
  const { data: models = [] } = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const current = models.find((m) => m.id === model)
  const setModel = async (id: string) => {
    done()
    try {
      const m = await api.set_model(session, id)
      if (!session && info) useConnection.getState().setInfo({ ...info, default_model: m.id })
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const setEffort = async (level: string | null) => {
    done()
    try {
      if (session) await api.set_effort(session, level)
      else onDraftEffort?.(level)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  return { agent, harnessModel, model, effort, models, current, setModel, setEffort, name: current?.name ?? model ?? 'Model' }
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
 * An ACP child (06 §8.2) shows `omp · anthropic/claude-opus-4-8` as plain text: the
 * harness owns its model, theuth cannot switch it.
 */
export function ModelMenu(props: Props) {
  const [open, setOpen] = useState(false)
  const m = useModel(props, () => setOpen(false))
  useOpenOnSlash(props.session, () => setOpen(true))
  if (m.agent) {
    return (
      <span className="flex h-9 max-w-full items-center gap-1 px-2.5 text-lg font-medium text-fg-secondary" title="External harness; its model is set on the harness side">
        <span className="min-w-0 truncate">{m.harnessModel ? `${m.agent} · ${m.harnessModel}` : m.agent}</span>
      </span>
    )
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" className="h-9 max-w-full gap-1 rounded-lg px-2.5 text-lg font-medium text-fg-secondary">
          <span className="min-w-0 truncate">{m.name}</span>
          {m.effort && <span className="shrink-0 text-sm font-normal text-fg-tertiary">· {m.effort}</span>}
          <ChevronDown className="size-4 opacity-70" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 p-0">
        <Command>
          <ModelCommand m={m} />
        </Command>
      </PopoverContent>
    </Popover>
  )
}

/** The compact menu's line: the model and effort (opens `ModelDialog`), or the harness's model as an indicator. */
export function ModelRow({ onOpen, ...props }: Props & { onOpen(): void }) {
  const m = useModel(props, onOpen)
  if (m.agent) return <MenuRow icon={Bot} label="Harness" detail={m.harnessModel ? `${m.agent} · ${m.harnessModel}` : m.agent} />
  return <MenuRow icon={Cpu} label="Model" detail={m.effort ? `${m.name} · ${m.effort}` : m.name} onSelect={onOpen} />
}

/** The same picker as a dialog: the compact header has no model button to anchor a popover to. */
export function ModelDialog({ open, onOpenChange, ...props }: Props & { open: boolean; onOpenChange(o: boolean): void }) {
  const m = useModel(props, () => onOpenChange(false))
  useOpenOnSlash(props.session, () => onOpenChange(true))
  if (m.agent) return null
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Model" description="Pick the model and effort" className="max-w-sm">
      <ModelCommand m={m} />
    </CommandDialog>
  )
}
