import { useNavigate } from '@tanstack/react-router'
import { Check, ChevronDown, Plus, RotateCw, Server, Settings2 } from 'lucide-react'
import { useStore } from 'zustand'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useLinkFailure } from '@/components/link/LinkFailure'
import { cn } from '@/lib/utils'
import { type Engine, LOCAL, LOCAL_NAME, type Presence, hasLocal, useEngineTree, useEngines } from '@/link/engines'
import { retryPair } from '@/link/nodes'
import { useConnection } from '@/store/connection'
import { scope } from '@/store/engine-scope'

const DOT: Record<Presence, string> = {
  online: 'bg-success',
  connecting: 'bg-warning',
  offline: 'bg-fg-tertiary/40',
  refused: 'bg-danger',
}

/** Paired engines plus the local daemon when this container has one. */
export const useEngineCount = (): number => useEngines((s) => s.engines.length) + (hasLocal() ? 1 : 0)

/** The local daemon has no relay PRESENCE; its dot follows its socket state. */
function useLocalPresence(): Presence {
  const state = useStore(scope(LOCAL).connection, (s) => s.state)
  return state === 'open' ? 'online' : state === 'closed' ? 'offline' : 'connecting'
}

/** Presence dot: green online, amber connecting, muted offline. */
export function Dot({ presence }: { presence: Presence }) {
  return <span className={cn('size-2 shrink-0 rounded-full', DOT[presence])} />
}

/** A mesh row's label: the mesh's name once its hub told us, the pairing name before. */
export const engineLabel = (h: Engine): string => (h.kind === 'mesh' && h.meshName) || h.name

/**
 * The chip beside a mesh row (69 decision 6): a hub failover is `PRESENCE 0` / `CLOSE 4|5` and the
 * device redials the same id, so it is "reconnecting…", never an error; `refused` means every hub
 * forgot this device — pair again.
 */
export const meshChip = (p: Presence): string | null => (p === 'online' ? null : p === 'refused' ? 'revoked — pair again' : 'reconnecting…')

/** `2 of 3 nodes online` for a mesh row; `null` before its hub answered `mesh_status`. */
export function useMembersLine(id: string): string | null {
  const m = useEngines((s) => s.members[id])
  return m ? `${m.online} of ${m.total} node${m.total === 1 ? '' : 's'} online` : null
}

/**
 * 31 §7.5 `● Studio Mac ▾`: every engine with its presence dot, nodes nested one step
 * under their parent (32 §8.1), "Add engine…", "Manage…". Hidden when the local
 * daemon is the only engine — today's desktop UI unchanged. Nodes include their
 * parent: `build-box · via Studio Mac`.
 */
export function EngineSwitcher({ className, compact = false }: { className?: string; compact?: boolean }) {
  const navigate = useNavigate()
  const engines = useEngines((s) => s.engines)
  const rows = useEngineTree()
  const active = useEngines((s) => s.active)
  const presence = useEngines((s) => s.presence)
  const pairError = useEngines((s) => s.pairError)
  const setActive = useEngines((s) => s.setActive)
  const info = useConnection((s) => s.info)
  const localPresence = useLocalPresence()
  if (engines.length === 0 && hasLocal()) return null

  const presenceOf = (id: string): Presence => (id === LOCAL ? localPresence : (presence[id] ?? 'offline'))
  const current = engines.find((h) => h.id === active)
  const activeName = current?.kind === 'mesh' ? engineLabel(current) : (info?.name ?? (active === LOCAL ? LOCAL_NAME : (current?.name ?? 'Node')))
  const via = current?.parent && (current.parent === LOCAL ? LOCAL_NAME : engines.find((h) => h.id === current.parent)?.name)
  const chip = current?.kind === 'mesh' ? meshChip(presenceOf(active)) : null

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        {compact ? (
          // The collapsed sidebar rail: a server glyph with the presence dot in its corner; the name is the label.
          <Button
            variant="ghost"
            size="icon"
            className={cn('relative rounded-lg text-fg-secondary', className)}
            aria-label={`Machine: ${activeName}${via ? ` · via ${via}` : ''}${chip ? ` · ${chip}` : ''}`}
            title={activeName}
          >
            <Server className="size-[18px]" />
            <span className="absolute top-1.5 right-1.5 flex rounded-full ring-2 ring-sidebar">
              <Dot presence={presenceOf(active)} />
            </span>
          </Button>
        ) : (
          <Button variant="ghost" className={cn('h-9 min-w-0 gap-2 rounded-lg px-2.5 font-medium text-fg-secondary', className)}>
            <Dot presence={presenceOf(active)} />
            <span className="min-w-0 truncate">
              {activeName}
              {via && <span className="font-normal text-fg-tertiary"> · via {via}</span>}
              {chip && <span className="font-normal text-fg-tertiary"> · {chip}</span>}
            </span>
            <ChevronDown className="size-4 opacity-70" />
          </Button>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side={compact ? 'right' : undefined} className="w-64">
        {rows.map(({ engine: h, depth }) => {
          const p = presenceOf(h.id)
          const failed = pairError[h.id]
          return (
            <DropdownMenuItem
              key={h.id}
              style={depth > 0 ? { paddingLeft: `${0.5 + depth}rem` } : undefined}
              onSelect={(e) => (failed ? e.preventDefault() : setActive(h.id))}
            >
              <Dot presence={p} />
              <span className="min-w-0 truncate">{h.id === LOCAL && active === LOCAL ? activeName : engineLabel(h)}</span>
              {h.kind === 'mesh' && <Members id={h.id} presence={p} />}
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
                <Check className={cn('size-4', h.kind === 'mesh' ? 'ml-1' : 'ml-auto')} />
              ) : (
                h.kind !== 'mesh' && (p === 'offline' || p === 'refused') && <Trail id={h.id} presence={p} />
              )}
            </DropdownMenuItem>
          )
        })}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void navigate({ to: '/link' })}>
          <Plus /> Pair with a mesh…
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void navigate({ to: '/settings/$tab', params: { tab: 'machine' }, search: {} })}>
          <Settings2 /> Manage…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** The mesh row's trailing text: the failover chip while not online, else `n of m nodes online`. */
function Members({ id, presence }: { id: string; presence: Presence }) {
  const line = useMembersLine(id)
  const text = meshChip(presence) ?? line
  return text ? <span className="ml-auto truncate pl-2 text-xs text-fg-tertiary">{text}</span> : null
}

/** An engine row's trailing text while not online: the named failure's title (LINK-MSG), else the presence word. */
function Trail({ id, presence }: { id: string; presence: Presence }) {
  const f = useLinkFailure(id)
  return (
    <span className="ml-auto max-w-[50%] truncate pl-2 text-xs text-fg-tertiary" title={f ? `${f.title}. ${f.detail}` : undefined}>
      {f?.title ?? presence}
    </span>
  )
}
