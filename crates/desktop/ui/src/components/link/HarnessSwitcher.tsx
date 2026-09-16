import { useNavigate } from '@tanstack/react-router'
import { Check, ChevronDown, Plus, Settings2 } from 'lucide-react'
import { useStore } from 'zustand'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { LOCAL, type Presence, hasLocal, useHarnesses } from '@/link/harnesses'
import { useConnection } from '@/store/connection'
import { scope } from '@/store/harness-scope'

const DOT: Record<Presence, string> = {
  online: 'bg-success',
  connecting: 'bg-warning',
  offline: 'bg-fg-tertiary/40',
}

/** Paired harnesses plus the local daemon when this container has one. */
export const useHarnessCount = (): number => useHarnesses((s) => s.harnesses.length) + (hasLocal() ? 1 : 0)

/** The local daemon has no relay PRESENCE; its dot follows its socket state. */
function useLocalPresence(): Presence {
  const state = useStore(scope(LOCAL).connection, (s) => s.state)
  return state === 'open' ? 'online' : state === 'closed' ? 'offline' : 'connecting'
}

/** Presence dot: green online, amber connecting, muted offline. */
export function Dot({ presence }: { presence: Presence }) {
  return <span className={cn('size-2 shrink-0 rounded-full', DOT[presence])} />
}

/**
 * 31 §7.5 `● Studio Mac ▾`: every harness with its presence dot, "Add harness…",
 * "Manage…". Hidden when the local daemon is the only harness — today's desktop UI unchanged.
 */
export function HarnessSwitcher({ className }: { className?: string }) {
  const navigate = useNavigate()
  const harnesses = useHarnesses((s) => s.harnesses)
  const active = useHarnesses((s) => s.active)
  const presence = useHarnesses((s) => s.presence)
  const setActive = useHarnesses((s) => s.setActive)
  const info = useConnection((s) => s.info)
  const local = hasLocal()
  const localPresence = useLocalPresence()
  if (harnesses.length === 0 && local) return null

  const presenceOf = (id: string): Presence => (id === LOCAL ? localPresence : (presence[id] ?? 'offline'))
  const activeName = info?.name ?? (active === LOCAL ? 'This Mac' : (harnesses.find((h) => h.id === active)?.name ?? 'Harness'))
  const rows = [...(local ? [{ id: LOCAL, name: 'This Mac' }] : []), ...harnesses]

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className={cn('h-9 min-w-0 gap-2 rounded-lg px-2.5 font-medium text-fg-secondary', className)}>
          <Dot presence={presenceOf(active)} />
          <span className="min-w-0 truncate">{activeName}</span>
          <ChevronDown className="size-4 opacity-70" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        {rows.map((h) => (
          <DropdownMenuItem key={h.id} onSelect={() => setActive(h.id)}>
            <Dot presence={presenceOf(h.id)} />
            <span className="min-w-0 truncate">{h.id === LOCAL && active === LOCAL ? activeName : h.name}</span>
            {h.id === active && <Check className="ml-auto size-4" />}
          </DropdownMenuItem>
        ))}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void navigate({ to: '/link' })}>
          <Plus /> Add harness…
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void navigate({ to: '/settings/$tab', params: { tab: 'harnesses' }, search: {} })}>
          <Settings2 /> Manage…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
