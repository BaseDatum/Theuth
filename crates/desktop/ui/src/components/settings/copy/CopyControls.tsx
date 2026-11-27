// 227 pieces 3 and 4 on a copyable field: an amber "differs on N" when other machines hold another value (its popover
// lists each machine's, with "Use this everywhere"), and "Set on all machines…". Nothing outside the Settings page, on
// a tab that is not copyable, or with one machine.

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { MachineRow } from '@/components/settings/MachineRow'
import { type SettingsMachines, useSettingsMachines } from '@/components/settings/machine-target'
import type { CopyField } from '@/components/settings/copy/set-on-all'
import type { Machine } from '@/lib/mesh-machines'
import { cn } from '@/lib/utils'
import { receiveResult } from '@/rpc/bindings/receive'
import { LOCAL } from '@/link/engines'

/** Setters "Set on all machines" never offers: a forwarded `plugin_approve` always answers `needs_press` (plugins' `HumanPress` refuses every forwarded call). */
const NEVER_COPIED: readonly string[] = ['plugin_approve']

/** Whether `field` gets copy controls here. */
export function useCopyable(field: CopyField | undefined): SettingsMachines | null {
  const settings = useSettingsMachines()
  if (!field || !settings?.tab?.copyable || !settings.target || settings.machines.length < 2) return null
  const plan = field.plan(field.value)
  if (plan.kind === 'calls' && plan.calls.some((c) => NEVER_COPIED.includes(c.method))) return null
  return settings
}

export const differsKey = (engines: readonly string[], key: string) => ['settings-values', key, engines] as const

/** One machine's value of a field (`undefined`: the getter failed there or the machine did not answer). */
export type MachineValue<T> = { machine: Machine; value: T | undefined }

/**
 * Each reachable machine's value of `field`: the mesh members' from `settings_values` on the mesh client, this device's
 * engine's from its own client (its getter called directly), the edited machine's from the tab.
 */
export function useMachineValues<T>(field: CopyField<T>, settings: SettingsMachines | null): MachineValue<T>[] | null {
  const read = field.read
  const reachable = (settings?.machines ?? []).filter((m) => m.online && !m.sealed)
  const local = reachable.find((m) => m.client === LOCAL && !m.forwarded)
  const engines = reachable.filter((m) => m !== local).map((m) => m.id)
  const values = useQuery(
    {
      enabled: !!settings?.api && !!read && engines.length > 0 && reachable.length > 1,
      queryKey: differsKey(engines, read?.key ?? ''),
      queryFn: () => settings!.api!.settings_values(engines, [read!.key]),
      staleTime: 30_000,
      retry: false,
    },
    settings?.qc,
  )
  const localValue = useQuery(
    {
      enabled: !!settings?.local && !!read && !!local && reachable.length > 1,
      queryKey: [...differsKey([LOCAL], read?.key ?? '')],
      queryFn: () => (settings!.local as unknown as Record<string, () => Promise<unknown>>)[read!.key]!(),
      staleTime: 30_000,
      retry: false,
    },
    settings?.qc,
  )
  if (!settings || !read || (engines.length > 0 && !values.data) || (local && settings.local && !localValue.isFetched)) return null
  const answers = values.data ?? {}
  return reachable.map((machine) => {
    if (machine.id === settings.target?.id) return { machine, value: field.value }
    // The local getter's answer is already adapted by its `Api`; a `settings_values` cell is raw wire JSON.
    const raw = machine === local ? localValue.data : answers[machine.id]?.[read.key]
    if (raw === undefined || raw === null) return { machine, value: undefined }
    return { machine, value: read.pick(machine === local ? raw : receiveResult(read.key, raw)) }
  })
}

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)

/** The machines (other than the edited one) whose value differs from it. */
export function differing<T>(values: readonly MachineValue<T>[], target: string, value: T): MachineValue<T>[] {
  return values.filter((v) => v.machine.id !== target && v.value !== undefined && !same(v.value, value))
}

export function CopyControls<T>({ field, className }: { field: CopyField<T>; className?: string }) {
  const settings = useCopyable(field as CopyField)
  return settings?.target ? <Controls field={field} settings={settings} target={settings.target} className={className} /> : null
}

function Controls<T>({ field, settings, target, className }: { field: CopyField<T>; settings: SettingsMachines; target: Machine; className?: string }) {
  const values = useMachineValues(field, settings)
  const [open, setOpen] = useState(false)
  const differs = values ? differing(values, target.id, field.value) : []
  const setOnAll = (value: T) => {
    setOpen(false)
    settings.open({ field: field as CopyField, value, tab: settings.tab!.id, needs: settings.tab!.needs })
  }
  return (
    <span className={cn('inline-flex flex-wrap items-center gap-2', className)} data-copy-controls={field.id}>
      {differs.length > 0 && values && (
        <Popover open={open} onOpenChange={setOpen}>
          <PopoverTrigger asChild>
            <button type="button" data-differs={differs.length} className="text-[11px] whitespace-nowrap text-warning underline-offset-2 hover:underline">
              differs on {differs.length}
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-[min(360px,calc(100vw-2rem))] space-y-1 p-2">
            <p className="px-1 pb-1 text-[12px] text-fg-tertiary">{field.label} on each machine</p>
            {values.map(({ machine, value }) => (
              <div key={machine.id} data-compare={machine.id} className="flex items-center gap-2 rounded-[8px] px-1 py-1 text-sm">
                <MachineRow machine={machine} tag={value === undefined ? 'unknown' : field.format(value)} />
                {value !== undefined && (
                  <Button size="sm" variant="ghost" className="h-7 shrink-0 px-2 text-[12px]" onClick={() => setOnAll(value as T)}>
                    Use this everywhere
                  </Button>
                )}
              </div>
            ))}
          </PopoverContent>
        </Popover>
      )}
      <Button size="sm" variant="outline" className="h-7 px-2 text-[12px] whitespace-nowrap" onClick={() => setOnAll(field.value)}>
        Set on all machines…
      </Button>
    </span>
  )
}
