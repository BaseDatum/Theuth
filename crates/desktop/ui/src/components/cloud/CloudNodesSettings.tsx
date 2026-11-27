// 224: Settings › Machines › Cloud nodes — the hosted nodes on the user's own provider accounts, with their status,
// version and monthly price, and the ⋯ menu (Restart · Resize · Update now · Open terminal · View desktop · View logs · Destroy…).
// Provider errors from `cloud_list` show inline here; every other status goes to the status bar (`notify`).

import { useNavigate } from '@tanstack/react-router'
import { Loader2, MoreHorizontal, Plus, RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { Tag } from '@/components/costs/ui'
import { Terminal } from '@/components/chat/TerminalViewer'
import { Empty, Section } from '@/components/settings/shared'
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
import { useIsMobile } from '@/hooks/use-mobile'
import { notify } from '@/lib/notify'
import { cn } from '@/lib/utils'
import { Capability } from '@/rpc/capabilities'
import { CloudNodeWizard } from './CloudNodeWizard'
import { NodeDesktopPanel } from './NodeDesktop'
import { type Tone, destroySaving, diskUsd, localQuote, sizesOf, statusOf, usd } from './format'
import { CLOUD_REQUIRED, useCloudActions, useCloudEvents, useCloudList, useCloudLogs, useCloudProviders, useCloudSizes } from './queries'
import { PriceNote, ProviderBadge, SizeCards } from './steps'
import { type CloudNode, DEFAULT_VOLUME_GB, type NodeDesktop, type ProviderInfo, type ProviderKind, type TerminalOpen } from './types'

export const PROVIDER_LABEL: Record<ProviderKind, string> = { digitalocean: 'DigitalOcean', fly: 'Fly.io', kubernetes: 'Kubernetes', lightsail: 'AWS Lightsail' }

export type CloudDialog = 'destroy' | 'resize' | 'logs' | 'terminal' | 'desktop'

/** The section, shown only on an engine with `cloudnodes` (the rest of the Machines tab never depends on it). */
export function CloudNodesSettings({ initial }: { initial?: { dialog: CloudDialog; node: string; adding?: boolean } | { adding: true } }) {
  return (
    <Capability required={CLOUD_REQUIRED} hide>
      <CloudNodesSection initial={initial} />
    </Capability>
  )
}

const DOT: Record<Tone, string> = {
  ok: 'bg-success',
  bad: 'bg-danger',
  warn: 'bg-warning',
  busy: 'bg-primary animate-pulse',
  muted: 'bg-fg-tertiary/40',
}

function Status({ node }: { node: CloudNode }) {
  const s = statusOf(node)
  return (
    <span title={s.text} className={cn('inline-flex min-w-0 items-center gap-1.5 truncate text-[13px] whitespace-nowrap', s.tone === 'bad' && 'text-danger', s.tone === 'warn' && 'text-warning')}>
      <span className={cn('inline-block size-[7px] shrink-0 rounded-full', DOT[s.tone])} />
      {s.text}
    </span>
  )
}

/** `☁︎ DigitalOcean · London`, `☁︎ Kubernetes · rwx-dev`. */
export function placeChip(n: CloudNode): string {
  const where = n.provider === 'kubernetes' ? n.k8s?.context : (n.region_label ?? n.region)
  return `☁︎ ${PROVIDER_LABEL[n.provider]}${where ? ` · ${where}` : ''}`
}

export function CloudNodesSection({ initial }: { initial?: { dialog?: CloudDialog; node?: string; adding?: boolean } }) {
  useCloudEvents()
  const mobile = useIsMobile()
  const navigate = useNavigate()
  const list = useCloudList()
  const providers = useCloudProviders()
  const actions = useCloudActions()
  const [adding, setAdding] = useState(!!initial?.adding)
  const [open, setOpen] = useState<{ dialog: CloudDialog; node: string } | null>(initial?.dialog && initial.node ? { dialog: initial.dialog, node: initial.node } : null)
  const [terminal, setTerminal] = useState<(TerminalOpen & { name: string }) | null>(null)
  const [desktop, setDesktop] = useState<(NodeDesktop & { name: string }) | null>(null)

  const nodes = list.data?.nodes ?? []
  const target = open ? nodes.find((n) => n.name === open.node) : undefined
  const infoOf = (k: ProviderKind): ProviderInfo | undefined => providers.data?.find((p) => p.provider === k)

  const add = () => (mobile ? void navigate({ to: '/cloud/new', search: { entry: 'add-to-mesh' } }) : setAdding(true))

  const openTerminal = (name: string) =>
    actions.terminal.mutate(name, {
      onSuccess: (t) => {
        setTerminal({ ...t, name })
        setOpen({ dialog: 'terminal', node: name })
      },
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
          {nodes.map((n) => (
            <NodeRow
              key={n.name}
              node={n}
              onRestart={() => actions.restart.mutate(n.name, { onSuccess: () => notify(`Restarting ${n.name}`) })}
              onUpdate={() => actions.update.mutate(n.name, { onSuccess: () => notify(`Updating ${n.name}`) })}
              onTerminal={() => openTerminal(n.name)}
              onDesktop={() => openDesktop(n.name)}
              opening={(actions.terminal.isPending && actions.terminal.variables === n.name) || (actions.desktop.isPending && actions.desktop.variables === n.name)}
              onOpen={(dialog) => setOpen({ dialog, node: n.name })}
            />
          ))}
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
        <TerminalPanel
          t={terminal}
          mobile={mobile}
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
      {adding && <CloudNodeWizard entry="desktop-dialog" onClose={() => setAdding(false)} onDone={() => setAdding(false)} />}
    </Section>
  )
}

export function NodeRow({
  node,
  onRestart,
  onUpdate,
  onTerminal,
  onDesktop,
  opening,
  onOpen,
}: {
  node: CloudNode
  onRestart: () => void
  onUpdate: () => void
  onTerminal: () => void
  onDesktop: () => void
  opening: boolean
  onOpen: (d: CloudDialog) => void
}) {
  const orphan = node.state === 'orphan'
  const live = node.state === 'ready'
  const settled = live || node.state === 'failed' || node.state === 'volume_kept' || orphan
  return (
    <li data-cloud-node={node.name} className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-border-subtle py-3 last:border-0 md:grid md:grid-cols-[1.5rem_minmax(0,1fr)_8.5rem_8rem_4.5rem_4.75rem] md:gap-x-3">
      <ProviderBadge kind={node.provider} className="size-6 text-[10px]" />
      <div className="flex min-w-0 flex-1 items-center gap-x-2">
        <span className="shrink-0 text-sm font-semibold">{node.name}</span>
        <span title={placeChip(node)} className="min-w-0 truncate rounded-full bg-layer-2 px-2 py-0.5 text-[11px] whitespace-nowrap">
          {placeChip(node)}
        </span>
        {node.provider === 'fly' && <Tag tone="info">beta</Tag>}
      </div>
      <Status node={node} />
      <span className="text-xs whitespace-nowrap text-fg-secondary max-md:hidden">{node.version ? `${node.version}${node.auto_update ? ' · auto-update' : ''}` : '—'}</span>
      <span className="text-right text-xs whitespace-nowrap text-fg-secondary">{node.monthly_usd != null ? `${usd(node.monthly_usd)}/mo` : ''}</span>
      {orphan ? (
        <Button size="xs" variant="outline" className="justify-self-end text-danger" onClick={() => onOpen('destroy')}>
          Destroy…
        </Button>
      ) : (
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
            <DropdownMenuItem disabled={!live || node.provider === 'kubernetes'} onSelect={() => onOpen('resize')}>
              Resize…
            </DropdownMenuItem>
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
      )}
    </li>
  )
}

/** The destroy confirmation's words: the node, the monthly saving, and the disk choice (kept by default). */
export function DestroyBody({ node, perGb, keep, onKeep }: { node: CloudNode; perGb: number | null | undefined; keep: boolean; onKeep: (keep: boolean) => void }) {
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

function TerminalPanel({ t, mobile, onClose }: { t: TerminalOpen & { name: string }; mobile: boolean; onClose: () => void }) {
  const term = <Terminal session={t.session} job={t.job} engine={t.engine} label={t.name} onExit={onClose} className="min-h-0 flex-1" />
  if (mobile)
    return (
      <Sheet open onOpenChange={(o) => !o && onClose()}>
        <SheetContent side="bottom" className="h-[85dvh]">
          <SheetHeader>
            <SheetTitle>{t.name}</SheetTitle>
            <SheetDescription className="sr-only">A shell on the cloud node.</SheetDescription>
          </SheetHeader>
          {term}
        </SheetContent>
      </Sheet>
    )
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex h-[75dvh] flex-col sm:max-w-4xl">
        <DialogHeader>
          <DialogTitle>{t.name} · terminal</DialogTitle>
          <DialogDescription className="sr-only">A shell on the cloud node.</DialogDescription>
        </DialogHeader>
        {term}
      </DialogContent>
    </Dialog>
  )
}
