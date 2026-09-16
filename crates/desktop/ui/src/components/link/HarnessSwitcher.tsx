import { useNavigate } from '@tanstack/react-router'
import { Check, ChevronDown, Plus, RotateCw, Settings2 } from 'lucide-react'
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
import { LOCAL, LOCAL_NAME, type Presence, hasLocal, useHarnessTree, useHarnesses } from '@/link/harnesses'
import { retryPair } from '@/link/nodes'
import { useConnection } from '@/store/connection'
import { scope } from '@/store/harness-scope'

const DOT: Record<Presence, string> = {
  online: 'bg-success',
  connecting: 'bg-warning',
  offline: 'bg-fg-tertiary/40',
  refused: 'bg-danger',
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
 * 31 §7.5 `● Studio Mac ▾`: every harness with its presence dot, nodes nested one step
 * under their parent (32 §8.1), "Add harness…", "Manage…". Hidden when the local
 * daemon is the only harness — today's desktop UI unchanged. Nodes include their
 * parent: `build-box · via Studio Mac`.
 */
export function HarnessSwitcher({ className }: { className?: string }) {
  const navigate = useNavigate()
  const harnesses = useHarnesses((s) => s.harnesses)
  const rows = useHarnessTree()
  const active = useHarnesses((s) => s.active)
  const presence = useHarnesses((s) => s.presence)
  const pairError = useHarnesses((s) => s.pairError)
  const setActive = useHarnesses((s) => s.setActive)
  const info = useConnection((s) => s.info)
  const localPresence = useLocalPresence()
  if (harnesses.length === 0 && hasLocal()) return null

  const presenceOf = (id: string): Presence => (id === LOCAL ? localPresence : (presence[id] ?? 'offline'))
  const current = harnesses.find((h) => h.id === active)
  const activeName = info?.name ?? (active === LOCAL ? LOCAL_NAME : (current?.name ?? 'Harness'))
  const via = current?.parent && (current.parent === LOCAL ? LOCAL_NAME : harnesses.find((h) => h.id === current.parent)?.name)

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className={cn('h-9 min-w-0 gap-2 rounded-lg px-2.5 font-medium text-fg-secondary', className)}>
          <Dot presence={presenceOf(active)} />
          <span className="min-w-0 truncate">
            {activeName}
            {via && <span className="font-normal text-fg-tertiary"> · via {via}</span>}
          </span>
          <ChevronDown className="size-4 opacity-70" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        {rows.map(({ harness: h, depth }) => {
          const p = presenceOf(h.id)
          const failed = pairError[h.id]
          return (
            <DropdownMenuItem
              key={h.id}
              style={depth > 0 ? { paddingLeft: `${0.5 + depth}rem` } : undefined}
              onSelect={(e) => (failed ? e.preventDefault() : setActive(h.id))}
            >
              <Dot presence={p} />
              <span className="min-w-0 truncate">{h.id === LOCAL && active === LOCAL ? activeName : h.name}</span>
              {failed ? (
                <Button
                  variant="ghost"
                  size="sm"
                  title={failed}
                  className="ml-auto h-6 gap-1 px-1.5 text-xs text-warning"
                  onClick={(e) => {
                    e.stopPropagation()
                    retryPair(h.id)
                  }}
                >
                  pairing failed · retry <RotateCw className="size-3" />
                </Button>
              ) : h.id === active ? (
                <Check className="ml-auto size-4" />
              ) : (
                (p === 'offline' || p === 'refused') && <span className="ml-auto text-xs text-fg-tertiary">{p}</span>
              )}
            </DropdownMenuItem>
          )
        })}
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
