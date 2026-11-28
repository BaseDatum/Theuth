// 224: Settings › Machines › Cloud nodes — the hosted nodes on the user's own provider accounts, with their status,
// version and monthly price, and the ⋯ menu (Restart · Resize · Update now · Open terminal · View desktop · View logs · Destroy…).
// Provider errors from `cloud_list` show inline here; every other status goes to the status bar (`notify`).

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Cloud, Loader2, MoreHorizontal, Plus, RefreshCw } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Terminal } from '@/components/chat/TerminalViewer'
import { Empty, Section, useAction } from '@/components/settings/shared'
import { keys as meshKeys } from '@/components/tasks/queries'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { useVaultApi } from '@/components/vault/api'
import { PasswordForm } from '@/components/vault/PasswordForm'
import { useIsMobile } from '@/hooks/use-mobile'
import { notify } from '@/lib/notify'
import { isPeerBootstrapping, usePeerSync } from '@/lib/peer-sync'
import { canPopout, openPopout } from '@/lib/popout'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { MeshStatus } from '@/rpc/bindings/method.nodes.output'
import { Capability, useQuerySupport } from '@/rpc/capabilities'
import { CloudNodeWizard, specFromNode } from './CloudNodeWizard'
import { NodeDesktopPanel } from './NodeDesktop'
import { type LivePeer, type Tone, destroySaving, diskUsd, localQuote, sizesOf, statusOf, usd, vaultRow } from './format'
import { isNewerUpdate, usePeerUpdateApply, usePeerUpdates } from './nodeUpdate'
import { CLOUD_REQUIRED, cloudKeys, useCloudActions, useCloudEvents, useCloudList, useCloudLogs, useCloudProviders, useCloudSizes, useVaultSupport } from './queries'
import { PriceNote, ProviderBadge, SizeCards, VAULT_HINT } from './steps'
import { type CloudNode, DEFAULT_VOLUME_GB, type NodeDesktop, type ProviderInfo, type ProviderKind, type TerminalOpen } from './types'

export const PROVIDER_LABEL: Record<ProviderKind, string> = { digitalocean: 'DigitalOcean', fly: 'Fly.io', kubernetes: 'Kubernetes', ssh: 'SSH', lightsail: 'AWS Lightsail' }

export type CloudDialog = 'destroy' | 'resize' | 'logs' | 'terminal' | 'desktop' | 'vault' | 'progress'

export type NodeUpdateControl = {
  version: string
  applied: boolean
  pending: boolean
  onApply: () => void
}

/** The section, shown only on an engine with `cloudnodes` (the rest of the Machines tab never depends on it). */
export function CloudNodesSettings({
  initial,
  progress,
  mesh,
}: {
  initial?: { dialog: CloudDialog; node: string; adding?: boolean } | { adding: true }
  progress?: string | null
  mesh?: MeshStatus
}) {
  return (
    <Capability required={CLOUD_REQUIRED} hide>
      <CloudNodesSection initial={initial} progress={progress} mesh={mesh} />
    </Capability>
  )
}

const DOT: Record<Tone, string> = {
  ok: 'bg-success',
  bad: 'bg-danger',
  warn: 'bg-warning',
  busy: 'bg-primary animate-pulse',
  sync: 'bg-warning animate-pulse',
  muted: 'bg-fg-tertiary/40',
}

function Status({ node, bootstrapping = false, livePeer }: { node: CloudNode; bootstrapping?: boolean; livePeer?: LivePeer | null }) {
  // `sealed` is honoured only where the engine has the sealed vault (feat/sealed-vault).
  const vault = useVaultSupport()
  const s = statusOf({ ...node, sealed: vault && node.sealed }, bootstrapping, livePeer)
  return (
    <span
      data-syncing={s.tone === 'sync' ? '' : undefined}
      title={node.error ?? s.hint ?? s.text}
      className={cn(
        'inline-flex min-w-0 items-center gap-1.5 truncate text-[13px] whitespace-nowrap',
        s.tone === 'bad' && 'text-danger',
        (s.tone === 'warn' || s.tone === 'sync') && 'text-warning',
        s.tone === 'muted' && 'text-fg-secondary',
      )}
    >
      <span className={cn('inline-block size-[7px] shrink-0 rounded-full', DOT[s.tone])} />
      <span className="truncate">{s.text}</span>
    </span>
  )
}

/** `DigitalOcean · London`, `Kubernetes · rwx-dev`, `SSH · prod-box`. */
export function placeChip(n: CloudNode): string {
  const where = n.provider === 'kubernetes' ? n.k8s?.context : n.provider === 'ssh' ? n.ssh?.host : (n.region_label ?? n.region)
  return `${PROVIDER_LABEL[n.provider]}${where ? ` · ${where}` : ''}`
}

/**
 * Desktop / browser: open the standalone node shell in its own OS popout window (`popout-terminal-<engine>-<shell>`).
 * iOS (`!canPopout()`): fall back to a bottom sheet using the same shell subscription (still no chat, no job).
 */
export function dispatchTerminalOpen(
  t: TerminalOpen,
  name: string,
  openSheet: (target: TerminalOpen & { name: string }) => void,
): void {
  if (canPopout()) {
    void openPopout({ kind: 'node-terminal', engine: t.engine, shell: t.shell, name })
    return
  }
  openSheet({ ...t, name })
}

export function CloudNodesSection({ initial, progress, mesh }: { initial?: { dialog?: CloudDialog; node?: string; adding?: boolean }; progress?: string | null; mesh?: MeshStatus }) {
  useCloudEvents()
  const mobile = useIsMobile()
  const navigate = useNavigate()
  const api = useApi()
  const canMesh = useQuerySupport(['mesh_status'])
  const meshQ = useQuery<MeshStatus>({
    enabled: !mesh && canMesh && typeof api.mesh_status === 'function',
    queryKey: meshKeys.mesh,
    queryFn: () => api.mesh_status(),
  })
  const meshData = mesh ?? meshQ.data
  const peersById = useMemo(() => {
    if (!meshData) return undefined
    const map = new Map<string, LivePeer>()
    for (const p of meshData.peers) map.set(p.id, p)
    if (meshData.self_id && !map.has(meshData.self_id)) {
      map.set(meshData.self_id, { reachable: true, path: 'offline', rtt_ms: null, last_seen_ms: 0 })
    }
    return map
  }, [meshData])

  const list = useCloudList()
  const { byPeer } = usePeerSync()
  const providers = useCloudProviders()
  const actions = useCloudActions()
  const [adding, setAdding] = useState(!!initial?.adding)
  const [open, setOpen] = useState<{ dialog: CloudDialog; node: string } | null>(
    initial?.dialog && initial.node ? { dialog: initial.dialog, node: initial.node } : progress ? { dialog: 'progress', node: progress } : null,
  )
  const [terminal, setTerminal] = useState<(TerminalOpen & { name: string }) | null>(null)
  const [desktop, setDesktop] = useState<(NodeDesktop & { name: string }) | null>(null)

  useEffect(() => {
    if (!progress) return
    setAdding(false)
    setOpen({ dialog: 'progress', node: progress })
  }, [progress])

  const nodes = list.data?.nodes ?? []
  const onlinePeerIds = useMemo(
    () =>
      nodes
        .filter((n) => {
          if (n.state !== 'ready' || !n.peer_id || n.peer_id === meshData?.self_id) return false
          if (peersById) return peersById.get(n.peer_id)?.reachable === true
          return n.reachable !== false && n.path !== 'offline'
        })
        .map((n) => n.peer_id as string),
    [nodes, peersById, meshData?.self_id],
  )
  const peerUpdates = usePeerUpdates(onlinePeerIds)
  const peerUpdateApply = usePeerUpdateApply()

  const target = open ? nodes.find((n) => n.name === open.node) : undefined
  const infoOf = (k: ProviderKind): ProviderInfo | undefined => providers.data?.find((p) => p.provider === k)

  const add = () => {
    if (mobile) void navigate({ to: '/cloud/new', search: { entry: 'add-to-mesh' } })
    else {
      setOpen(null)
      setAdding(true)
    }
  }
  const openDialog = (dialog: CloudDialog, name: string) => {
    if (dialog === 'progress' && mobile) {
      void navigate({ to: '/cloud/new', search: { entry: 'add-to-mesh', node: name } })
      return
    }
    setAdding(false)
    setOpen({ dialog, node: name })
  }
  const closeProgress = () => {
    setOpen(null)
    if (progress) void navigate({ to: '/settings/$tab', params: { tab: 'machines' }, search: {}, replace: true })
  }

  const openTerminal = (name: string) =>
    actions.terminal.mutate(name, {
      onSuccess: (t) =>
        dispatchTerminalOpen(t, name, (sheet) => {
          setTerminal(sheet)
          setOpen({ dialog: 'terminal', node: name })
        }),
    })
  const openDesktop = (name: string) =>
    actions.desktop.mutate(name, {
      onSuccess: (d) => {
        setDesktop({ ...d, name })
        setOpen({ dialog: 'desktop', node: name })
      },
    })

  return (
    <Section
      title="Cloud nodes"
      actions={
        <Button size="sm" onClick={add}>
          <Plus />
          Add cloud node
        </Button>
      }
    >
      {list.data?.errors.map((e) => (
        <p key={e.provider} role="alert" className="mb-2 rounded-lg border border-danger-border/50 bg-danger-bg px-3 py-2 text-xs text-danger">
          {PROVIDER_LABEL[e.provider]}: {e.message}
        </p>
      ))}
      {list.isPending ? (
        <Empty>Loading…</Empty>
      ) : list.isError ? (
        <p className="py-4 text-sm text-danger">{list.error.message}</p>
      ) : nodes.length === 0 ? (
        <Empty>No cloud nodes yet. One keeps working when this machine sleeps, and gives your phone a full-power engine.</Empty>
      ) : (
        <ul>
          {nodes.map((n) => {
            const livePeer: LivePeer | null | undefined = peersById ? (n.peer_id ? (peersById.get(n.peer_id) ?? null) : null) : undefined
            const rawUpdate = n.state === 'ready' && n.peer_id ? peerUpdates.get(n.peer_id) : null
            const available = isNewerUpdate(rawUpdate, n.version) ? rawUpdate : null
            const updateControl: NodeUpdateControl | null =
              available && n.peer_id
                ? {
                    version: available.version,
                    applied: available.applied,
                    pending: peerUpdateApply.isPending(n.peer_id),
                    onApply: () => peerUpdateApply.apply(n.peer_id!, n.name),
                  }
                : null
            const updateError = n.peer_id ? peerUpdateApply.errorOf(n.peer_id) : null
            return (
              <NodeRow
                key={n.name}
                node={n}
                livePeer={livePeer}
                bootstrapping={!!n.peer_id && isPeerBootstrapping(byPeer.get(n.peer_id))}
                update={updateControl}
                updateError={updateError}
                onRestart={() => actions.restart.mutate(n.name, { onSuccess: () => notify(`Restarting ${n.name}`) })}
                onUpdate={() => actions.update.mutate(n.name, { onSuccess: () => notify(`Updating ${n.name}`) })}
                onTerminal={() => openTerminal(n.name)}
                onDesktop={() => openDesktop(n.name)}
                opening={(actions.terminal.isPending && actions.terminal.variables === n.name) || (actions.desktop.isPending && actions.desktop.variables === n.name)}
                onOpen={(dialog) => openDialog(dialog, n.name)}
              />
            )
          })}
        </ul>
      )}

      {open?.dialog === 'destroy' && target && (
        <DestroyDialog
          node={target}
          perGb={infoOf(target.provider)?.volume_usd_per_gb}
          busy={actions.destroy.isPending}
          onCancel={() => setOpen(null)}
          onConfirm={(keep) =>
            actions.destroy.mutate(
              { name: target.name, keep },
              {
                onSuccess: (removed) => {
                  setOpen(null)
                  notify.success(`${target.name} destroyed`, { description: removed.join(' · ') || undefined })
                },
              },
            )
          }
        />
      )}
      {open?.dialog === 'resize' && target && (
        <ResizeDialog
          node={target}
          provider={infoOf(target.provider)}
          busy={actions.resize.isPending}
          onCancel={() => setOpen(null)}
          onConfirm={(memory_gb) =>
            actions.resize.mutate(
              { name: target.name, memory_gb },
              {
                onSuccess: () => {
                  setOpen(null)
                  notify(`Resizing ${target.name} to ${memory_gb} GB`)
                },
              },
            )
          }
        />
      )}
      {open?.dialog === 'logs' && target && <LogsDialog name={target.name} onClose={() => setOpen(null)} />}
      {open?.dialog === 'terminal' && terminal && (
        <TerminalSheet
          t={terminal}
          onClose={() => {
            setOpen(null)
            setTerminal(null)
          }}
        />
      )}
      {open?.dialog === 'desktop' && desktop && (
        <NodeDesktopPanel
          name={desktop.name}
          access={desktop}
          mobile={mobile}
          onClose={() => {
            setOpen(null)
            setDesktop(null)
          }}
        />
      )}
      {open?.dialog === 'vault' && target?.peer_id && <VaultPasswordDialog name={target.name} engine={target.peer_id} onClose={() => setOpen(null)} />}
      {open?.dialog === 'progress' && (
        <CloudNodeWizard
          key={open.node}
          entry="desktop-dialog"
          initial={{ step: 'progress', node: open.node, ...(target ? { spec: specFromNode(target) } : {}) }}
          onClose={closeProgress}
          onDone={closeProgress}
        />
      )}
      {adding && <CloudNodeWizard entry="desktop-dialog" onClose={() => setAdding(false)} onDone={() => setAdding(false)} />}
    </Section>
  )
}

export function NodeRow({
  node,
  livePeer,
  bootstrapping = false,
  update = null,
  updateError = null,
  onRestart,
  onUpdate,
  onTerminal,
  onDesktop,
  opening,
  onOpen,
}: {
  node: CloudNode
  livePeer?: LivePeer | null
  bootstrapping?: boolean
  update?: NodeUpdateControl | null
  updateError?: string | null
  onRestart: () => void
  onUpdate: () => void
  onTerminal: () => void
  onDesktop: () => void
  opening: boolean
  onOpen: (d: CloudDialog) => void
}) {
  const orphan = node.state === 'orphan'
  const creating = node.state === 'creating'
  const failed = node.state === 'failed'
  const live = node.state === 'ready'
  const settled = live || failed || node.state === 'volume_kept' || orphan
  const vault = vaultRow(node)
  const menu = (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" className="justify-self-end" aria-label={`${node.name} actions`}>
          {opening ? <Loader2 className="animate-spin" /> : <MoreHorizontal />}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem disabled={!live} onSelect={onRestart}>
          Restart
        </DropdownMenuItem>
        {node.provider !== 'ssh' && (
          <DropdownMenuItem disabled={!live || node.provider === 'kubernetes'} onSelect={() => onOpen('resize')}>
            Resize…
          </DropdownMenuItem>
        )}
        <DropdownMenuItem disabled={!live} onSelect={onUpdate}>
          Update now
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled={!live} onSelect={onTerminal}>
          Open terminal
        </DropdownMenuItem>
        {node.desktop && (
          <DropdownMenuItem disabled={!live} onSelect={onDesktop}>
            View desktop
          </DropdownMenuItem>
        )}
        <DropdownMenuItem disabled={!live} onSelect={() => onOpen('logs')}>
          View logs
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem variant="destructive" disabled={!settled} onSelect={() => onOpen('destroy')}>
          Destroy…
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
  return (
    <li data-cloud-node={node.name} className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-border-subtle py-3 last:border-0 md:grid md:grid-cols-[1.5rem_minmax(0,1fr)_10.5rem_8rem_4.5rem_minmax(4.75rem,auto)] md:gap-x-3">
      <ProviderBadge kind={node.provider} className="size-6 text-[10px]" />
      <div className="flex min-w-0 flex-1 items-center gap-x-2">
        <span className="shrink-0 text-sm font-semibold">{node.name}</span>
        <span title={placeChip(node)} className="inline-flex min-w-0 items-center gap-1 rounded-full bg-layer-2 px-2 py-0.5 text-[11px] whitespace-nowrap">
          <Cloud className="size-3 shrink-0 text-fg-secondary" aria-hidden />
          <span className="truncate">{placeChip(node)}</span>
        </span>
      </div>
      <Status node={node} livePeer={livePeer} bootstrapping={bootstrapping} />
      <span className="text-xs whitespace-nowrap text-fg-secondary max-md:hidden">{node.version ? `${node.version}${node.auto_update ? ' · auto-update' : ''}` : '—'}</span>
      <span className="text-right text-xs whitespace-nowrap text-fg-secondary">{node.monthly_usd != null ? `${usd(node.monthly_usd)}/mo` : ''}</span>
      {orphan ? (
        <Button size="xs" variant="outline" className="justify-self-end text-danger" onClick={() => onOpen('destroy')}>
          Destroy…
        </Button>
      ) : creating ? (
        <Button size="xs" variant="outline" className="justify-self-end" onClick={() => onOpen('progress')}>
          View status
        </Button>
      ) : failed ? (
        <div className="flex items-center justify-end gap-1 justify-self-end">
          <Button size="xs" variant="outline" onClick={() => onOpen('progress')}>
            View status
          </Button>
          {menu}
        </div>
      ) : update ? (
        <div className="flex items-center justify-end gap-1 justify-self-end">
          <Button
            size="xs"
            variant="outline"
            data-update-node={node.name}
            aria-label={`Update ${node.name}`}
            disabled={update.pending}
            onClick={update.onApply}
          >
            {update.pending && <Loader2 className="animate-spin" />}
            {update.pending ? 'Updating…' : `Update ${update.version}`}
          </Button>
          {menu}
        </div>
      ) : (
        menu
      )}
      {failed && node.error && (
        <p
          data-node-error=""
          role="alert"
          className="w-full min-w-0 font-mono text-xs whitespace-pre-wrap break-words text-danger md:col-span-5 md:col-start-2"
        >
          {node.error}
        </p>
      )}
      {updateError && (
        <p
          data-update-error={node.peer_id ?? node.name}
          role="alert"
          className="w-full min-w-0 font-mono text-xs whitespace-pre-wrap break-words text-danger md:col-span-5 md:col-start-2"
        >
          {updateError}
        </p>
      )}
      {vault && (
        <div data-vault={node.vault} className="flex w-full min-w-0 items-center gap-2 text-xs md:col-span-5 md:col-start-2">
          <span className={cn('min-w-0 truncate', vault.tone === 'warn' ? 'text-warning' : 'text-fg-secondary')}>{vault.text}</span>
          {vault.action && (
            <Button size="xs" variant={vault.tone === 'warn' ? 'outline' : 'link'} className={cn(vault.tone !== 'warn' && 'h-auto px-0 text-xs')} onClick={() => onOpen('vault')}>
              Set master password
            </Button>
          )}
        </div>
      )}
    </li>
  )
}

/**
 * 225: "Set master password" for a node's vault: Settings › Vault's own form, sent to the node over the mesh
 * (`vault_add_password(engine, …)`). The list then re-reads the node's vault and drops the line.
 */
export function VaultPasswordDialog({ name, engine, onClose }: { name: string; engine: string; onClose: () => void }) {
  const api = useVaultApi()
  const add = useAction((pw: string) => api.addPassword(engine, pw), { keys: [cloudKeys.list], onSuccess: () => notify.success(`${name}'s vault has a master password`) })
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Master password for {name}</DialogTitle>
          <DialogDescription>{VAULT_HINT}</DialogDescription>
        </DialogHeader>
        <PasswordForm mode="add" name={name} onSubmit={(next) => add.mutateAsync(next)} onDone={onClose} onCancel={onClose} />
      </DialogContent>
    </Dialog>
  )
}

/** The destroy confirmation's words: the node, the monthly saving, and the disk choice (kept by default). */
export function DestroyBody({ node, perGb, keep, onKeep }: { node: CloudNode; perGb: number | null | undefined; keep: boolean; onKeep: (keep: boolean) => void }) {
  if (node.provider === 'ssh') {
    const host = node.ssh?.host ?? 'the server'
    return (
      <>
        <AlertDialogHeader>
          <AlertDialogTitle>Destroy {node.name}?</AlertDialogTitle>
          <AlertDialogDescription>
            {keep
              ? `This stops and uninstalls the theuth service on ${host} and unpairs it from your mesh. Your data in ~/.theuth is kept.`
              : `This stops and uninstalls the theuth service on ${host}, removes ~/.theuth, and unpairs it from your mesh.`}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" className="mt-0.5 size-4 accent-[var(--primary)]" checked={keep} onChange={(e) => onKeep(e.currentTarget.checked)} />
          <span>Keep ~/.theuth data on {host}</span>
        </label>
      </>
    )
  }
  const saving = destroySaving(node, keep, perGb)
  const disk = diskUsd(node, perGb)
  const gb = node.volume_gb ?? DEFAULT_VOLUME_GB
  const kept = node.state === 'volume_kept'
  return (
    <>
      <AlertDialogHeader>
        <AlertDialogTitle>Destroy {node.name}?</AlertDialogTitle>
        <AlertDialogDescription>
          {kept
            ? `This deletes its ${gb} GB disk${disk !== null ? ` (${usd(disk)}/mo)` : ''} and everything on it.`
            : saving !== null
              ? `You'll save ${usd(saving)}/mo.${keep ? '' : ' The disk and everything on it is deleted.'}`
              : `The node is removed from ${PROVIDER_LABEL[node.provider]}.`}
        </AlertDialogDescription>
      </AlertDialogHeader>
      {!kept && (
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" className="mt-0.5 size-4 accent-[var(--primary)]" checked={keep} onChange={(e) => onKeep(e.currentTarget.checked)} />
          <span>
            Keep the {gb} GB disk{disk !== null ? ` (${usd(disk)}/mo)` : ''} so you can bring it back
          </span>
        </label>
      )}
    </>
  )
}

export function DestroyDialog({ node, perGb, busy, onCancel, onConfirm }: { node: CloudNode; perGb: number | null | undefined; busy: boolean; onCancel: () => void; onConfirm: (keep: boolean) => void }) {
  const [keep, setKeep] = useState(node.state !== 'volume_kept')
  return (
    <AlertDialog open onOpenChange={(o) => !o && onCancel()}>
      <AlertDialogContent>
        <DestroyBody node={node} perGb={perGb} keep={keep} onKeep={setKeep} />
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={busy}
            onClick={(e) => {
              e.preventDefault()
              onConfirm(node.state === 'volume_kept' ? false : keep)
            }}
          >
            {busy && <Loader2 className="animate-spin" />}
            Destroy
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

export function ResizeDialog({ node, provider, busy, onCancel, onConfirm }: { node: CloudNode; provider: ProviderInfo | undefined; busy: boolean; onCancel: () => void; onConfirm: (gb: number) => void }) {
  const [gb, setGb] = useState(node.memory_gb ?? 2)
  // Live sizes for the node's provider and region; unpriced (and still choosable) when the provider gives no prices.
  const live = useCloudSizes(node.provider, node.region)
  const sizes = sizesOf(live.data) ?? (live.isError ? provider?.sizes.map((s) => ({ ...s, monthly_usd: null })) : undefined)
  const perGb = provider?.volume_usd_per_gb ?? null
  const price = localQuote(node.provider, sizes, perGb, gb, node.volume_gb ?? DEFAULT_VOLUME_GB)?.total_usd ?? null
  return (
    <Dialog open onOpenChange={(o) => !o && onCancel()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Resize {node.name}</DialogTitle>
          <DialogDescription>It restarts on the new size, about a minute. The disk and everything on it stay.</DialogDescription>
        </DialogHeader>
        <SizeCards kind={node.provider} sizes={sizes} perGb={perGb} value={gb} desktop={node.desktop} volume_gb={node.volume_gb} current={node.memory_gb} onChange={setGb} />
        {sizes && price === null && node.provider !== 'kubernetes' && <PriceNote kind={node.provider} className="text-xs text-fg-secondary" />}
        <DialogFooter>
          <Button variant="outline" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button disabled={busy || gb === node.memory_gb} onClick={() => onConfirm(gb)}>
            {busy && <Loader2 className="animate-spin" />}
            {price !== null ? `Resize · ${usd(price)}/mo` : 'Resize'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function LogsDialog({ name, onClose }: { name: string; onClose: () => void }) {
  const logs = useCloudLogs(name)
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex max-h-[85dvh] flex-col sm:max-w-3xl">
        <DialogHeader className="flex-row items-center gap-2">
          <DialogTitle>{name} · logs</DialogTitle>
          <DialogDescription className="sr-only">The tail of the node engine's own log, redacted by the node.</DialogDescription>
          <Button variant="ghost" size="icon-sm" className="mr-6 ml-auto" aria-label="Refresh" disabled={logs.isFetching} onClick={() => void logs.refetch()}>
            <RefreshCw className={cn(logs.isFetching && 'animate-spin')} />
          </Button>
        </DialogHeader>
        <pre className="min-h-48 flex-1 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap">
          {logs.isPending ? 'Loading…' : logs.isError ? logs.error.message : logs.data.lines.length ? logs.data.lines.join('\n') : 'No log lines yet.'}
        </pre>
      </DialogContent>
    </Dialog>
  )
}

/** iOS-only fallback (`!canPopout()`): bottom sheet subscribed to the standalone shell (no chat, no job). */
function TerminalSheet({ t, onClose }: { t: TerminalOpen & { name: string }; onClose: () => void }) {
  const api = useApi()
  const closedRef = useRef(false)
  const end = () => {
    if (closedRef.current) return
    closedRef.current = true
    void api.shell_close(t.shell, t.engine).catch(() => undefined)
  }
  const close = () => {
    end()
    onClose()
  }
  useEffect(() => () => end(), [t.shell, t.engine])
  return (
    <Sheet open onOpenChange={(o) => !o && close()}>
      <SheetContent side="bottom" className="flex h-[85dvh] flex-col">
        <SheetHeader>
          <SheetTitle>{t.name}</SheetTitle>
          <SheetDescription className="sr-only">A shell on the cloud node.</SheetDescription>
        </SheetHeader>
        <Terminal source={{ kind: 'shell', shell: t.shell, engine: t.engine }} label={t.name} onExit={close} className="min-h-0 flex-1" />
      </SheetContent>
    </Sheet>
  )
}
