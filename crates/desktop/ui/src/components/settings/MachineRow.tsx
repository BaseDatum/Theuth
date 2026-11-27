// 227: one machine of the mesh as Settings shows it (the picker, "Set on all machines"): a presence dot, the name, and
// a quiet tag saying what it is or why it cannot be reached. Meant to be shared with Settings › Machines later.

import type { Machine } from '@/lib/mesh-machines'
import { BOOTSTRAPPING_HINT, BOOTSTRAPPING_LABEL } from '@/lib/peer-sync'
import { cn } from '@/lib/utils'

export type Presence = 'online' | 'offline' | 'sealed' | 'syncing'

export const presenceOf = (m: Machine, syncing = false): Presence =>
  !m.online ? 'offline' : m.sealed ? 'sealed' : syncing ? 'syncing' : 'online'

const DOT: Readonly<Record<Presence, string>> = {
  online: 'bg-success',
  offline: 'bg-fg-tertiary/40',
  sealed: 'bg-warning',
  syncing: 'bg-warning animate-pulse',
}

/** The row's tag (mock: `local`, `hub`, `direct`, `relay`, `cloud`, `vault locked`, `Syncing mesh history…`, `offline`). */
export function machineTag(m: Machine, syncing = false): string {
  if (!m.online) return 'offline'
  if (m.sealed) return 'vault locked'
  if (syncing) return BOOTSTRAPPING_LABEL
  if (m.kind === 'local') return m.note ?? (m.hub ? 'local · hub' : 'local')
  if (m.hub) return 'hub'
  if (m.kind === 'cloud') return 'cloud'
  return m.path ?? 'peer'
}

export function MachineDot({ machine, syncing = false, className }: { machine: Machine; syncing?: boolean; className?: string }) {
  const presence = presenceOf(machine, syncing)
  return <span data-presence={presence} aria-hidden className={cn('inline-block size-2 shrink-0 rounded-full', DOT[presence], className)} />
}

export function MachineRow({
  machine,
  selected,
  syncing = false,
  tag = machineTag(machine, syncing),
  className,
}: {
  machine: Machine
  selected?: boolean
  syncing?: boolean
  tag?: string | null
  className?: string
}) {
  return (
    <span
      data-machine={machine.id}
      title={syncing ? BOOTSTRAPPING_HINT : undefined}
      className={cn('flex min-w-0 flex-1 items-center gap-2', className)}
    >
      <MachineDot machine={machine} syncing={syncing} />
      <span className="min-w-0 flex-1 truncate">{machine.name}</span>
      {tag && <span className={cn('shrink-0 text-[11px]', syncing ? 'text-warning' : 'text-fg-tertiary')}>{tag}{selected ? ' ✓' : ''}</span>}
    </span>
  )
}
