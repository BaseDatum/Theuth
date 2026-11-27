// 227: one machine of the mesh as Settings shows it (the picker, "Set on all machines"): a presence dot, the name, and
// a quiet tag saying what it is or why it cannot be reached. Meant to be shared with Settings › Machines later.

import type { Machine } from '@/lib/mesh-machines'
import { cn } from '@/lib/utils'

export type Presence = 'online' | 'offline' | 'sealed'

export const presenceOf = (m: Machine): Presence => (!m.online ? 'offline' : m.sealed ? 'sealed' : 'online')

const DOT: Readonly<Record<Presence, string>> = { online: 'bg-success', offline: 'bg-fg-tertiary/40', sealed: 'bg-warning' }

/** The row's tag (mock: `local`, `hub`, `direct`, `relay`, `cloud`, `vault locked`, `offline`). */
export function machineTag(m: Machine): string {
  if (!m.online) return 'offline'
  if (m.sealed) return 'vault locked'
  if (m.kind === 'local') return m.hub ? 'local · hub' : 'local'
  if (m.hub) return 'hub'
  if (m.kind === 'cloud') return 'cloud'
  return m.path ?? 'peer'
}

export function MachineDot({ machine, className }: { machine: Machine; className?: string }) {
  const presence = presenceOf(machine)
  return <span data-presence={presence} aria-hidden className={cn('inline-block size-2 shrink-0 rounded-full', DOT[presence], className)} />
}

export function MachineRow({ machine, selected, tag = machineTag(machine), className }: { machine: Machine; selected?: boolean; tag?: string | null; className?: string }) {
  return (
    <span data-machine={machine.id} className={cn('flex min-w-0 flex-1 items-center gap-2', className)}>
      <MachineDot machine={machine} />
      <span className="min-w-0 flex-1 truncate">{machine.name}</span>
      {tag && <span className="shrink-0 text-[11px] text-fg-tertiary">{tag}{selected ? ' ✓' : ''}</span>}
    </span>
  )
}
