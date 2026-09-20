import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, ChevronDown } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Command,
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
import { surfaceOf } from './slash'

/**
 * ChatGPT's header slot: the model name with a cmdk-filtered list from
 * `list_models`, then the selected model's effort levels. A fresh chat keeps
 * effort in the UI until submit creates the session; model changes update the default.
 */
export function ModelMenu({
  session,
  draftEffort,
  onDraftEffort,
}: {
  session: string | null
  draftEffort?: string | null
  onDraftEffort?(effort: string | null): void
}) {
  const api = useApi()
  const qc = useQueryClient()
  const info = useConnection((s) => s.info)
  const sessionModel = useSessionField(session ?? '', 'model')
  const sessionEffort = useSessionField(session ?? '', 'effort')
  const model = sessionModel ?? info?.default_model
  const effort = session ? sessionEffort : draftEffort
  const { data: models = [] } = useQuery({ queryKey: queryKeys.models, queryFn: () => api.list_models() })
  const current = models.find((m) => m.id === model)
  const [open, setOpen] = useState(false)

  const focused = usePanes((s) => session === null || s.focused === session)
  useEffect(() => {
    if (!focused) return
    const on = (e: Event) => surfaceOf(e) === 'model' && setOpen(true)
    window.addEventListener('theuth:open', on)
    return () => window.removeEventListener('theuth:open', on)
  }, [focused])

  const setModel = async (id: string) => {
    setOpen(false)
    try {
      const m = await api.set_model(session, id)
      if (!session && info) useConnection.getState().setInfo({ ...info, default_model: m.id })
      void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }
  const setEffort = async (level: string | null) => {
    setOpen(false)
    try {
      if (session) await api.set_effort(session, level)
      else onDraftEffort?.(level)
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" className="h-9 max-w-full gap-1 rounded-lg px-2.5 text-lg font-medium text-fg-secondary">
          <span className="min-w-0 truncate">{current?.name ?? model ?? 'Model'}</span>
          {effort && <span className="shrink-0 text-sm font-normal text-fg-tertiary">· {effort}</span>}
          <ChevronDown className="size-4 opacity-70" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 p-0">
        <Command>
          <CommandInput placeholder="Search models" />
          <CommandList className="max-h-96">
            <CommandEmpty>No model matches.</CommandEmpty>
            <CommandGroup heading="Models">
              {models.map((m) => (
                <CommandItem key={m.id} value={`${m.name} ${m.id}`} onSelect={() => void setModel(m.id)}>
                  <span className="truncate">{m.name}</span>
                  <span className="ml-auto truncate text-xs text-fg-tertiary">{m.provider}</span>
                  {m.id === model && <Check className="size-4" />}
                </CommandItem>
              ))}
            </CommandGroup>
            {current && current.efforts.length > 0 && (
              <>
                <CommandSeparator />
                <CommandGroup heading="Effort">
                  {current.efforts.map((lvl) => (
                    <CommandItem key={lvl} value={`effort ${lvl}`} onSelect={() => void setEffort(lvl)}>
                      {lvl}
                      {lvl === effort && <Check className="ml-auto size-4" />}
                    </CommandItem>
                  ))}
                  <CommandItem value="effort clear" onSelect={() => void setEffort(null)}>
                    clear
                    {!effort && <Check className="ml-auto size-4" />}
                  </CommandItem>
                </CommandGroup>
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}
