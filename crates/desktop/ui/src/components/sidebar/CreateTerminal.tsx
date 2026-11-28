import { useNavigate } from '@tanstack/react-router'
import { Loader2, Terminal } from 'lucide-react'
import { type ReactNode, useState } from 'react'
import { age } from '@/components/tasks/helpers'
import { useMesh } from '@/components/tasks/queries'
import { openViewTile } from '@/components/tiles/open'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { tileLeaf, usePanes } from '@/lib/panes'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { MeshStatus } from '@/rpc/bindings/method.nodes.output'
import {
  freshShellId,
  terminalKey,
  terminalName,
  terminalPath,
  useUserTerminals,
  type UserTerminal,
} from '@/store/terminals'

const ROW =
  'flex min-h-8 w-full min-w-0 items-center gap-2 rounded-md px-2 py-1 text-left text-sm outline-hidden hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent disabled:pointer-events-none disabled:opacity-50 pointer-coarse:min-h-10'

export type TerminalNode = {
  /** Mesh id (`mesh_status.self_id` or a peer id); empty only before `mesh_status` loads. */
  id: string
  /** Node display name. */
  name: string
  self: boolean
  reachable: boolean
  status: string
}

export type EngineShellApi = {
  engine_shell(engine?: string | null, shell?: string | null): Promise<{ engine: string; shell: string }>
}

export type TerminalNavigate = (opts: {
  to: '/terminal/$engine/$shell'
  params: { engine: string; shell: string }
  search: { name: string }
}) => unknown

const errMessage = (e: unknown): string => (e instanceof Error ? e.message : String(e))

/**
 * Nodes shown in the Create terminal popover: this machine (always enabled), followed by every mesh peer
 * (reachable peers enabled, unreachable peers visible and disabled).
 */
export function terminalNodes(
  mesh: Pick<MeshStatus, 'self_id' | 'self_name' | 'peers'> | undefined,
  now = Date.now(),
): TerminalNode[] {
  const self: TerminalNode = {
    id: mesh?.self_id ?? '',
    name: mesh?.self_name?.trim() || 'This machine',
    self: true,
    reachable: true,
    status: 'this machine',
  }
  if (!mesh) return [self]
  const peers: TerminalNode[] = mesh.peers
    .filter((p) => p.id !== mesh.self_id)
    .map((p) => ({
      id: p.id,
      name: p.name?.trim() || p.id.slice(0, 8),
      self: false,
      reachable: p.reachable,
      status: !p.reachable
        ? p.last_seen_ms > 0
          ? `offline ${age(p.last_seen_ms, now)}`
          : 'offline'
        : p.path === 'relay'
          ? 'via relay'
          : p.path === 'lan'
            ? 'LAN'
            : '',
    }))
  return [self, ...peers]
}

/**
 * Open `terminal` in a new workspace pane on `/terminal/$engine/$shell` (`NodeTerminal`).
 * Never opens an OS popout window (works on iPad where `canPopout()` is false).
 * When no Trellis workspace is mounted (phone or non-chat page), navigates in-app to `/terminal/$engine/$shell`.
 */
export function openTerminalPane(terminal: UserTerminal, navigate?: TerminalNavigate): string | null {
  const path = terminalPath(terminal)
  const id = openViewTile({ path, title: terminal.name })
  if (id !== null) {
    const leaf = tileLeaf(id)
    usePanes.getState().select(leaf)
    return leaf
  }
  void navigate?.({
    to: '/terminal/$engine/$shell',
    params: { engine: terminal.engine, shell: terminal.shell },
    search: { name: terminal.name },
  })
  return null
}

/**
 * Create a user terminal on `node` via `api.engine_shell(engine, shell)`, store it in `useUserTerminals`,
 * and open a workspace pane on `/terminal/$engine/$shell`. Never calls `engine_terminal`, never creates a chat or job.
 */
export async function createUserTerminal(
  api: EngineShellApi,
  node: TerminalNode,
  navigate?: TerminalNavigate,
  makeShellId: () => string = freshShellId,
): Promise<UserTerminal> {
  const shell = makeShellId()
  const name = terminalName(node.name)
  const res = await api.engine_shell(node.id || null, shell)
  const terminal: UserTerminal = {
    engine: res?.engine || node.id,
    shell: res?.shell || shell,
    name,
    node: node.name,
  }
  useUserTerminals.getState().add(terminal)
  openTerminalPane(terminal, navigate)
  return terminal
}

/** Pure popover body: nodes to start a shell on, inline error (`role="alert"`), and terminals created this session. */
export function CreateTerminalMenu({
  nodes,
  terminals,
  pending,
  error,
  onCreate,
  onOpen,
}: {
  nodes: readonly TerminalNode[]
  terminals: readonly UserTerminal[]
  pending: string | null
  error: string | null
  onCreate(node: TerminalNode): void
  onOpen(terminal: UserTerminal): void
}) {
  return (
    <div className="flex flex-col gap-0.5" data-create-terminal-menu>
      <div className="px-2 py-1 text-xs font-medium text-fg-secondary">New terminal on</div>
      <ul>
        {nodes.map((node) => {
          const key = node.self ? 'self' : node.id
          const busy = pending === key
          return (
            <li key={key}>
              <button
                type="button"
                data-terminal-node={node.id || 'self'}
                disabled={!node.reachable || pending !== null}
                className={ROW}
                onClick={() => onCreate(node)}
              >
                {busy ? (
                  <Loader2 className="size-3 shrink-0 animate-spin text-fg-secondary" aria-hidden />
                ) : (
                  <span
                    className={cn(
                      'size-1.5 shrink-0 rounded-full',
                      node.reachable ? 'bg-success' : 'ring-1 ring-fg-tertiary ring-inset',
                    )}
                    aria-hidden
                  />
                )}
                <span className="min-w-0 flex-1 truncate">{node.name}</span>
                {node.status && (
                  <span
                    className={cn(
                      'shrink-0 text-[11px] text-fg-tertiary',
                      node.status === 'via relay' && 'rounded-full border border-border px-1.5',
                    )}
                  >
                    {node.status}
                  </span>
                )}
              </button>
            </li>
          )
        })}
      </ul>
      {error && (
        <div
          role="alert"
          className="mt-1 rounded-md border border-danger-border/40 bg-danger-bg px-2 py-1.5 font-mono text-xs break-words text-danger"
        >
          {error}
        </div>
      )}
      {terminals.length > 0 && (
        <>
          <div className="my-1 h-px bg-border" />
          <div className="px-2 py-1 text-xs font-medium text-fg-secondary">Terminals</div>
          <ul>
            {terminals.map((t) => (
              <li key={terminalKey(t)}>
                <button
                  type="button"
                  data-user-terminal={terminalKey(t)}
                  className={ROW}
                  onClick={() => onOpen(t)}
                >
                  <Terminal className="size-3.5 shrink-0 text-fg-secondary" aria-hidden />
                  <span className="min-w-0 flex-1 truncate">{t.name}</span>
                  <span className="shrink-0 text-[11px] text-fg-tertiary">{t.node}</span>
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  )
}

function CreateTerminalPopover({
  trigger,
  wrap = (t) => t,
}: {
  trigger: ReactNode
  wrap?(trigger: ReactNode): ReactNode
}) {
  const [open, setOpen] = useState(false)
  const [pending, setPending] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const api = useApi()
  const navigate = useNavigate()
  const { data: mesh } = useMesh()
  const terminals = useUserTerminals((s) => s.terminals)
  const nodes = terminalNodes(mesh)

  const handleOpenChange = (next: boolean) => {
    setOpen(next)
    if (!next) setError(null)
  }

  const handleCreate = (node: TerminalNode) => {
    if (!node.reachable || pending !== null) return
    const key = node.self ? 'self' : node.id
    setPending(key)
    setError(null)
    void createUserTerminal(api, node, navigate).then(
      () => {
        setOpen(false)
      },
      (e: unknown) => {
        setError(errMessage(e))
      },
    ).finally(() => {
      setPending(null)
    })
  }

  const handleOpen = (terminal: UserTerminal) => {
    openTerminalPane(terminal, navigate)
    setOpen(false)
  }

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      {wrap(<PopoverTrigger asChild>{trigger}</PopoverTrigger>)}
      <PopoverContent side="right" align="start" className="w-64 p-1">
        <CreateTerminalMenu
          nodes={nodes}
          terminals={terminals}
          pending={pending}
          error={error}
          onCreate={handleCreate}
          onOpen={handleOpen}
        />
      </PopoverContent>
    </Popover>
  )
}

/** Expanded sidebar Create terminal control, placed next to New chat. */
export function CreateTerminalButton({ className }: { className?: string }) {
  return (
    <CreateTerminalPopover
      trigger={
        <button
          type="button"
          aria-label="Create terminal"
          title="Create terminal"
          className={cn(
            'flex size-9 shrink-0 items-center justify-center rounded-[10px] text-sidebar-foreground outline-hidden hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 focus-visible:ring-sidebar-ring',
            className,
          )}
        >
          <Terminal className="size-4" aria-hidden />
        </button>
      }
    />
  )
}

/** Collapsed icon rail Create terminal control, placed next to New chat. */
export function CreateTerminalRail({ itemClass, wrap }: { itemClass: string; wrap(trigger: ReactNode): ReactNode }) {
  return (
    <CreateTerminalPopover
      wrap={wrap}
      trigger={
        <button type="button" aria-label="Create terminal" className={itemClass}>
          <Terminal className="size-[18px]" aria-hidden />
        </button>
      }
    />
  )
}
