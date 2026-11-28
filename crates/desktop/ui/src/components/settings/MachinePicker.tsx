// 227: the machine picker at the top of Settings. Desktop and iPad: the top of the categories column; phone: a row
// under the Settings header that opens the same menu. Offline machines are listed, disabled.

import { ChevronDown } from 'lucide-react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { MachineDot, MachineRow } from '@/components/settings/MachineRow'
import { usePickedMachine, useSettingsMachines } from '@/components/settings/machine-target'
import type { Machine } from '@/lib/mesh-machines'
import { cn } from '@/lib/utils'

export function MachinePicker({ phone, className, selected }: { phone?: boolean; className?: string; selected?: Machine | null }) {
  const settings = useSettingsMachines()
  const pick = usePickedMachine((s) => s.pick)
  // One machine (or an engine that cannot forward): nothing to pick.
  if (!settings || settings.machines.length < 2) return null
  const target = selected ?? settings.target
  if (!target) return null
  const { machines } = settings
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        data-machine-picker={phone ? 'phone' : 'sidebar'}
        aria-label={`Settings of ${target.name}. Pick a machine`}
        className={cn(
          'flex w-full min-w-0 items-center gap-2 rounded-[10px] border border-border-subtle bg-layer-2 px-2.5 text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50',
          phone ? 'h-11' : 'h-9',
          className,
        )}
      >
        <MachineDot machine={target} />
        <span className="min-w-0 flex-1 truncate text-left font-medium">{target.name}</span>
        {target.hub && <span className="shrink-0 rounded-full border border-project-plum/35 px-1.5 text-[10px] leading-4 text-project-plum">hub</span>}
        <ChevronDown className="size-4 shrink-0 text-fg-tertiary" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-(--radix-dropdown-menu-trigger-width) min-w-56">
        {machines.map((m) => (
          <DropdownMenuItem key={m.id} disabled={!m.online} aria-current={m.id === target.id || undefined} onSelect={() => pick(m.id)}>
            <MachineRow machine={m} selected={m.id === target.id} />
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
