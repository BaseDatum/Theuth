import { open as pickFolders } from '@tauri-apps/plugin-dialog'
import { Camera, ChevronRight, FolderPlus, RotateCcw, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Field } from '@/components/agents/RuntimeAccessFields'
import type { Action } from '@/components/settings/shared'
import { age } from '@/components/tasks/helpers'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Switch } from '@/components/ui/switch'
import { isTauri } from '@/lib/tauri'
import type { ImageRef } from "@/rpc/bindings/method.sandbox.output"
import type { Mount } from "@/rpc/bindings/method.sandbox.output"
import type { NetPolicy } from "@/rpc/bindings/method.sandbox.output"
import type { Pending } from "@/rpc/bindings/method.sandbox.output"
import type { Preflight } from "@/rpc/bindings/method.sandbox.output"
import type { ResizeOutcome } from "@/rpc/bindings/method.sandbox.output"
import type { ResourceChange } from "@/rpc/bindings/method.sandbox.output"
import type { Resources } from "@/rpc/bindings/method.sandbox.output"
import type { SandboxCreate } from '@/rpc/bindings/method.sandbox.input'
import type { SandboxInfo } from "@/rpc/bindings/method.sandbox.output"
import type { VmSnapshotInfo } from "@/rpc/bindings/method.sandbox.output"
import { fmtBytes, fmtMib, guestPath, limits, needsRestart, nextVmName, resizeGuard } from '@/store/sandboxes'

type SheetProps = { open: boolean; onOpenChange: (open: boolean) => void }

/** A labelled native range with its value beside it; `onCommit` fires on release (pointer up / key up), for live resizes. */
function Range({ id, label, value, min, max, step = 1, format, onChange, onCommit, hint }: { id: string; label: string; value: number; min: number; max: number; step?: number; format: (v: number) => string; onChange: (v: number) => void; onCommit?: (v: number) => void; hint?: string }) {
  return (
    <Field label={`${label} · ${format(value)}`} id={id} hint={hint ?? `${format(min)} – ${format(max)}`}>
      <input
        id={id}
        type="range"
        className="w-full accent-primary"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        onPointerUp={(e) => onCommit?.(Number(e.currentTarget.value))}
        onKeyUp={(e) => onCommit?.(Number(e.currentTarget.value))}
      />
    </Field>
  )
}

/** Shared folders (79 decision 7): host path → `/shared/<basename>`, a read-only switch per row; Add folder is the OS picker, a text field off Tauri. */
function SharesEditor({ shares, onChange }: { shares: Mount[]; onChange: (s: Mount[]) => void }) {
  const [typed, setTyped] = useState('')
  const add = (hosts: string[]) => {
    const fresh = hosts.map((h) => h.trim()).filter((h) => h && !shares.some((s) => s.host === h))
    if (fresh.length) onChange([...shares, ...fresh.map((host) => ({ host, guest: guestPath(host), readonly: false }))])
  }
  const pick = async () => {
    const picked = await pickFolders({ directory: true, multiple: true })
    if (picked) add(Array.isArray(picked) ? picked : [picked])
  }
  return (
    <div className="flex flex-col gap-2">
      {shares.length === 0 && <p className="text-xs text-fg-tertiary">No folder is shared. The VM sees only what you add here.</p>}
      {shares.map((s, i) => (
        <div key={s.host} className="flex items-center gap-2 text-[13px] max-lg:flex-wrap">
          <span className="min-w-0 flex-1 truncate font-mono text-xs" title={s.host}>
            {s.host}
          </span>
          <span className="shrink-0 font-mono text-xs text-fg-tertiary max-lg:max-w-full max-lg:wrap-anywhere">→ {s.guest}</span>
          <label className="flex shrink-0 items-center gap-1 text-xs text-fg-secondary">
            <Switch checked={s.readonly} aria-label={`${s.host} read-only`} onCheckedChange={(ro) => onChange(shares.map((x, j) => (j === i ? { ...x, readonly: ro } : x)))} /> read-only
          </label>
          <Button size="icon-xs" variant="ghost" aria-label={`Remove ${s.host}`} onClick={() => onChange(shares.filter((_, j) => j !== i))}>
            <Trash2 />
          </Button>
        </div>
      ))}
      {isTauri() ? (
        <Button size="sm" variant="outline" className="self-start" onClick={() => void pick()}>
          <FolderPlus /> Add folder
        </Button>
      ) : (
        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            add([typed])
            setTyped('')
          }}
        >
          <Input className="h-8 font-mono text-base md:text-xs" placeholder="/absolute/host/path" value={typed} onChange={(e) => setTyped(e.target.value)} aria-label="Folder to share" />
          <Button size="sm" variant="outline" type="submit" disabled={!typed.trim()}>
            <FolderPlus /> Add folder
          </Button>
        </form>
      )}
    </div>
  )
}

const NETWORKS: readonly [NetPolicy, string][] = [
  ['public_host', 'public, host'],
  ['public_host_private', 'public, host, private'],
  ['public_only', 'public only'],
]

/** The `SandboxCreate` the sheet sends for its draft: only what differs from the derived defaults is set. */
export function toCreate(d: { name: string; res: Resources; shares: Mount[]; desktop: boolean; network: NetPolicy; image: ImageRef | undefined; autostart: boolean }, defaults: Resources): SandboxCreate {
  const diff = <K extends keyof Resources>(k: K) => (d.res[k] === defaults[k] ? undefined : d.res[k])
  return {
    name: d.name.trim(),
    cpus: diff('cpus'),
    memory_mib: diff('memory_mib'),
    root_disk_mib: diff('root_disk_mib'),
    max_cpus: diff('max_cpus'),
    max_memory_mib: diff('max_memory_mib'),
    shares: d.shares,
    desktop: d.desktop ? undefined : false,
    network: d.network === 'public_host' ? undefined : d.network,
    image: d.image,
    autostart: d.autostart ? undefined : false,
  }
}

const NAME_RE = /^[a-z0-9][a-z0-9-]{0,31}$/
const STOCK_BASE = 'ghcr.io/basedatum/theuth-vm-base:trixie'

/** **Add a sandbox VM** (79 §7): name, three sliders within the host's limits, shares, desktop, network, an Advanced disclosure. */
export function SandboxCreateSheet({ open, onOpenChange, preflight, taken, create }: SheetProps & { preflight: Preflight; taken: readonly Pick<SandboxInfo, 'name'>[]; create: Action<SandboxCreate, SandboxInfo> }) {
  const lim = limits(preflight)
  const [name, setName] = useState('')
  const [res, setRes] = useState<Resources>(preflight.defaults)
  const [shares, setShares] = useState<Mount[]>([])
  const [desktop, setDesktop] = useState(true)
  const [network, setNetwork] = useState<NetPolicy>('public_host')
  const [imageMode, setImageMode] = useState<'default' | 'oci' | 'bootstrap'>('default')
  const [ref, setRef] = useState('')
  const [autostart, setAutostart] = useState(true)
  // Re-derive on every open: the list may have grown, the defaults may have moved.
  useEffect(() => {
    if (!open) return
    setName(nextVmName(taken))
    setRes(preflight.defaults)
    setShares([])
    setDesktop(true)
    setNetwork('public_host')
    setImageMode('default')
    setRef('')
    setAutostart(true)
  }, [open, taken, preflight.defaults])
  const nameErr = !NAME_RE.test(name.trim()) ? 'lowercase letters, digits and dashes, 32 at most' : taken.some((t) => t.name === name.trim()) ? 'a VM of that name exists' : undefined
  // A: `script: ""` means the bundled `bootstrap.sh` (`<data_dir>/sandbox/bootstrap/`); the UI has no script editor.
  const image: ImageRef | undefined = imageMode === 'oci' && ref.trim() ? { kind: 'oci', ref: ref.trim() } : imageMode === 'bootstrap' ? { kind: 'bootstrap', base: STOCK_BASE, script: '' } : undefined
  const set = <K extends keyof Resources>(k: K) => (v: number) => setRes((r) => ({ ...r, [k]: v }))
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="flex w-full flex-col gap-0 overflow-y-auto sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Add a sandbox VM</SheetTitle>
          <SheetDescription>This VM joins your mesh and receives your vault, like any engine.</SheetDescription>
        </SheetHeader>
        <form
          className="flex flex-col gap-4 px-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (nameErr) return
            create.mutate(toCreate({ name, res, shares, desktop, network, image, autostart }, preflight.defaults), { onSuccess: () => onOpenChange(false) })
          }}
        >
          <Field label="Name" id="vm-name" error={nameErr} hint="also its hostname and mesh name">
            <Input id="vm-name" className="h-8 font-mono" value={name} onChange={(e) => setName(e.target.value)} autoComplete="off" />
          </Field>
          <Range id="vm-cpus" label="CPUs" value={res.cpus} min={lim.cpus[0]} max={Math.min(lim.cpus[1], res.max_cpus)} format={(v) => `${v}`} onChange={set('cpus')} />
          <Range id="vm-mem" label="Memory" value={res.memory_mib} min={lim.memory_mib[0]} max={Math.min(lim.memory_mib[1], res.max_memory_mib)} step={512} format={fmtMib} onChange={set('memory_mib')} />
          <Range id="vm-disk" label="Disk" value={res.root_disk_mib} min={lim.root_disk_mib[0]} max={lim.root_disk_mib[1]} step={1024} format={fmtMib} onChange={set('root_disk_mib')} hint="sparse: grows on use, can grow later, never shrinks" />
          <Field label="Shared folders" id="vm-shares">
            <SharesEditor shares={shares} onChange={setShares} />
          </Field>
          <label className="flex items-center justify-between text-[13px]">
            <span>
              Desktop
              <span className="block text-xs text-fg-tertiary">XFCE on a VNC display; the agent's `computer_*` tools and your pop-out share it</span>
            </span>
            <Switch checked={desktop} onCheckedChange={setDesktop} aria-label="Desktop" />
          </label>
          <Field label="Network" id="vm-net" hint={network === 'public_host_private' ? 'private: the VM can reach your LAN — a NAS, a printer, other machines. Off by default because isolation from the LAN is part of what a sandbox is for.' : network === 'public_only' ? 'no host services (host.microsandbox.internal)' : 'internet + services on this Mac (host.microsandbox.internal); not your LAN'}>
            <Select value={network} onValueChange={(v) => setNetwork(v as NetPolicy)}>
              <SelectTrigger id="vm-net" size="sm" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {NETWORKS.map(([v, l]) => (
                  <SelectItem key={v} value={v}>
                    {l}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
          <Collapsible>
            <CollapsibleTrigger className="flex items-center gap-1 text-[13px] text-fg-secondary [&[data-state=open]>svg]:rotate-90">
              <ChevronRight className="size-3.5 transition-transform" /> Advanced
            </CollapsibleTrigger>
            <CollapsibleContent className="flex flex-col gap-4 pt-3">
              <Range id="vm-max-cpus" label="CPU ceiling" value={res.max_cpus} min={res.cpus} max={lim.cpus[1]} format={(v) => `${v}`} onChange={set('max_cpus')} hint="the most a live resize can reach without a restart" />
              <Range id="vm-max-mem" label="Memory ceiling" value={res.max_memory_mib} min={res.memory_mib} max={lim.memory_mib[1]} step={512} format={fmtMib} onChange={set('max_memory_mib')} hint="address space, not memory: keep it high" />
              <Field label="Image" id="vm-image">
                <Select value={imageMode} onValueChange={(v) => setImageMode(v as typeof imageMode)}>
                  <SelectTrigger id="vm-image" size="sm" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="default">the published theuth image</SelectItem>
                    <SelectItem value="oci">another image ref…</SelectItem>
                    <SelectItem value="bootstrap">bootstrap stock Debian at first boot (minutes)</SelectItem>
                  </SelectContent>
                </Select>
              </Field>
              {imageMode === 'oci' && <Input className="h-8 font-mono text-base md:text-xs" placeholder="ghcr.io/basedatum/theuth-vm:2.1.2" value={ref} onChange={(e) => setRef(e.target.value)} aria-label="Image ref" />}
              <label className="flex items-center justify-between text-[13px]">
                <span>
                  Autostart
                  <span className="block text-xs text-fg-tertiary">start with this engine</span>
                </span>
                <Switch checked={autostart} onCheckedChange={setAutostart} aria-label="Autostart" />
              </label>
            </CollapsibleContent>
          </Collapsible>
          <SheetFooter className="px-0">
            <Button type="submit" disabled={!!nameErr || create.isPending}>
              {create.isPending ? 'Creating…' : 'Create'}
            </Button>
          </SheetFooter>
        </form>
      </SheetContent>
    </Sheet>
  )
}

/** **Resize** (79 §5.5): live within the ceilings on release; a ceiling change says "restart to apply"; the server's guard is applied before the call. */
export function SandboxResizeSheet({ open, onOpenChange, vm, preflight, resize }: SheetProps & { vm: SandboxInfo; preflight: Preflight; resize: Action<{ name: string; change: ResourceChange }, ResizeOutcome> }) {
  const v = vm.vm
  const lim = limits(preflight)
  const [res, setRes] = useState<Resources>(v?.resources ?? preflight.defaults)
  const [note, setNote] = useState<string | null>(null)
  useEffect(() => {
    if (open && v) setRes(v.resources)
  }, [open, v])
  if (!v) return null
  const current = v.resources
  const guard = resizeGuard(res, current, v.metrics)
  const restart = needsRestart(res, current)
  const commit = (next: Resources) => {
    const bad = resizeGuard(next, current, v.metrics)
    if (bad) {
      setNote(bad)
      return
    }
    const change: ResourceChange = {
      cpus: next.cpus !== current.cpus ? next.cpus : null,
      memory_mib: next.memory_mib !== current.memory_mib ? next.memory_mib : null,
      root_disk_mib: next.root_disk_mib !== current.root_disk_mib ? next.root_disk_mib : null,
      max_cpus: next.max_cpus !== current.max_cpus ? next.max_cpus : null,
      max_memory_mib: next.max_memory_mib !== current.max_memory_mib ? next.max_memory_mib : null,
    }
    if (Object.values(change).every((x) => x === null)) return
    resize.mutate(
      { name: vm.name, change },
      {
        onSuccess: (out) => {
          if (out.status.state === 'guest_refused') setNote(`The guest refused: ${out.status.why}`)
          else if (out.restart_required) setNote('Ceiling recorded — restart to apply.')
          else setNote(out.status.state === 'converging' ? 'Applying… the guest is onlining the change.' : null)
        },
      },
    )
  }
  const set = <K extends keyof Resources>(k: K) => (x: number) => setRes((r) => ({ ...r, [k]: x }))
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="flex w-full flex-col gap-0 overflow-y-auto sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Resize {vm.name}</SheetTitle>
          <SheetDescription>CPU and memory change live within the ceilings; the disk only grows. Release a slider to apply.</SheetDescription>
        </SheetHeader>
        <div className="flex flex-col gap-4 px-4">
          <Range id="rs-cpus" label="CPUs" value={res.cpus} min={1} max={current.max_cpus} format={(x) => `${x}`} onChange={set('cpus')} onCommit={(x) => commit({ ...res, cpus: x })} />
          <Range id="rs-mem" label="Memory" value={res.memory_mib} min={lim.memory_mib[0]} max={current.max_memory_mib} step={512} format={fmtMib} onChange={set('memory_mib')} onCommit={(x) => commit({ ...res, memory_mib: x })} hint={`a live change moves the hypervisor allocation; the guest's MemTotal stays at its boot size${v.metrics ? ` · the guest uses ${fmtBytes(v.metrics.memory_bytes)}; the floor is that plus 512 MiB` : ''}`} />
          <Range id="rs-disk" label="Disk" value={res.root_disk_mib} min={current.root_disk_mib} max={Math.max(current.root_disk_mib, lim.root_disk_mib[1])} step={1024} format={fmtMib} onChange={set('root_disk_mib')} onCommit={(x) => commit({ ...res, root_disk_mib: x })} hint="grow-only" />
          <Collapsible>
            <CollapsibleTrigger className="flex items-center gap-1 text-[13px] text-fg-secondary [&[data-state=open]>svg]:rotate-90">
              <ChevronRight className="size-3.5 transition-transform" /> Ceilings (restart to apply)
            </CollapsibleTrigger>
            <CollapsibleContent className="flex flex-col gap-4 pt-3">
              <Range id="rs-max-cpus" label="CPU ceiling" value={res.max_cpus} min={res.cpus} max={lim.cpus[1]} format={(x) => `${x}`} onChange={set('max_cpus')} onCommit={(x) => commit({ ...res, max_cpus: x })} />
              <Range id="rs-max-mem" label="Memory ceiling" value={res.max_memory_mib} min={res.memory_mib} max={lim.memory_mib[1]} step={512} format={fmtMib} onChange={set('max_memory_mib')} onCommit={(x) => commit({ ...res, max_memory_mib: x })} />
            </CollapsibleContent>
          </Collapsible>
          {(guard || note || restart || v.pending.resources) && (
            <p className={guard ? 'text-[13px] text-danger' : 'text-[13px] text-warning'} role="status">
              {guard ?? note ?? 'Restart to apply.'}
            </p>
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}

/** **Shared folders** on an existing VM: the same editor; Save records them (`pending`), **Restart now** applies them. */
export function SandboxSharesSheet({ open, onOpenChange, vm, setShares, stop, start }: SheetProps & { vm: SandboxInfo; setShares: Action<{ name: string; shares: Mount[] }, Pending>; stop: Action<string, SandboxInfo>; start: Action<string, SandboxInfo> }) {
  const v = vm.vm
  const [shares, setLocal] = useState<Mount[]>(v?.shares ?? [])
  const [restart, setRestart] = useState(false)
  useEffect(() => {
    if (open && v) setLocal(v.shares)
  }, [open, v])
  if (!v) return null
  const dirty = JSON.stringify(shares) !== JSON.stringify(v.shares)
  const pending = restart || v.pending.shares
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="flex w-full flex-col gap-0 overflow-y-auto sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Shared folders of {vm.name}</SheetTitle>
          <SheetDescription>Bind mounts, applied by the next start. Files keep their host ownership; nothing is copied.</SheetDescription>
        </SheetHeader>
        <div className="flex flex-col gap-4 px-4">
          <SharesEditor shares={shares} onChange={setLocal} />
          <div className="flex items-center gap-2">
            <Button size="sm" disabled={!dirty || setShares.isPending} onClick={() => setShares.mutate({ name: vm.name, shares }, { onSuccess: (p) => setRestart(p.restart_required) })}>
              Save
            </Button>
            {pending && vm.state.state === 'running' && (
              <Button
                size="sm"
                variant="outline"
                disabled={stop.isPending || start.isPending}
                onClick={() => stop.mutate(vm.name, { onSuccess: () => start.mutate(vm.name, { onSuccess: () => setRestart(false) }) })}
              >
                <RotateCcw /> Restart now
              </Button>
            )}
            {pending && <span className="text-xs text-warning">restart to apply</span>}
          </div>
        </div>
      </SheetContent>
    </Sheet>
  )
}

/** **Snapshots** (79 §5.6): the list with labels, sizes and the `auto` badge; a restore confirms in its own label. */
export function SandboxSnapshots({ open, onOpenChange, vm, snapshot, restore }: SheetProps & { vm: SandboxInfo; snapshot: Action<{ name: string; label: string; full: boolean }, VmSnapshotInfo>; restore: Action<{ name: string; id: string }, SandboxInfo> }) {
  const [label, setLabel] = useState('')
  const [full, setFull] = useState(false)
  const snaps = vm.vm?.snapshots ?? []
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="flex w-full flex-col gap-0 overflow-y-auto sm:max-w-md">
        <SheetHeader>
          <SheetTitle>Snapshots of {vm.name}</SheetTitle>
          <SheetDescription>Disk snapshots boot fresh on restore; a full one keeps RAM and running processes. Named snapshots are never pruned.</SheetDescription>
        </SheetHeader>
        <div className="flex flex-col gap-4 px-4">
          <form
            className="flex flex-wrap items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              if (!label.trim()) return
              snapshot.mutate(
                { name: vm.name, label: label.trim(), full },
                {
                  onSuccess: (s) => {
                    setLabel('')
                    toast.success(`Snapshot ${s.id} taken`)
                  },
                },
              )
            }}
          >
            <label className="flex items-center gap-1 text-xs text-fg-secondary">
              <Switch checked={full} onCheckedChange={setFull} aria-label="Full snapshot" /> full
            </label>
            <Input className="h-8 flex-1 font-mono text-base md:text-xs" placeholder="label, e.g. clean" value={label} onChange={(e) => setLabel(e.target.value)} aria-label="Snapshot label" />
            <Button size="sm" type="submit" disabled={!label.trim() || snapshot.isPending}>
              <Camera /> Snapshot
            </Button>
          </form>
          {snaps.length === 0 && <p className="text-xs text-fg-tertiary">No snapshot yet.</p>}
          <ul className="flex flex-col divide-y divide-border-subtle">
            {snaps.map((s) => (
              <li key={s.id} className="flex flex-col gap-1 py-2 text-[13px]">
                <div className="flex items-center gap-2">
                  <span className="font-mono">{s.id}</span>
                  {s.auto && <span className="rounded-full bg-layer-2 px-1.5 text-[11px] text-fg-secondary">auto</span>}
                  {s.full && <span className="rounded-full bg-layer-2 px-1.5 text-[11px] text-fg-secondary">full</span>}
                  <span className="ml-auto text-xs text-fg-tertiary tabular-nums">
                    {s.bytes !== null ? fmtBytes(s.bytes) : '—'} · {age(s.at_ms)} ago
                  </span>
                </div>
                <Button size="xs" variant="outline" className="self-start text-danger" disabled={restore.isPending} onClick={() => restore.mutate({ name: vm.name, id: s.id }, { onSuccess: () => onOpenChange(false) })}>
                  Restore {s.id} — current disk is discarded
                </Button>
              </li>
            ))}
          </ul>
        </div>
      </SheetContent>
    </Sheet>
  )
}
