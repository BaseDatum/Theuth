import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Camera, Download, FolderOpen, MoreHorizontal, Monitor, Pause, Play, RotateCcw, Scaling, Square, Terminal, Trash2 } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { SandboxCreateSheet, SandboxResizeSheet, SandboxSharesSheet, SandboxSnapshots } from '@/components/settings/SandboxSheets'
import { useHandoffs, usePreflight, useSandboxActions, useSandboxLog, useSandboxes, unresolved } from '@/components/settings/sandbox-queries'
import { Dot, Empty, Section } from '@/components/settings/shared'
import { useMesh } from '@/components/tasks/queries'
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
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Switch } from '@/components/ui/switch'
import { canPopout, openPopout } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import type { Preflight } from "@/rpc/bindings/method.sandbox.output"
import type { Api } from '@/rpc/bindings/client'
import type { SandboxInfo } from "@/rpc/bindings/method.sandbox.output"
import { type ProgressLine, canStart, fmtBytes, fmtMib, stateLabel } from '@/store/sandboxes'

/**
 * Settings › Network › **Sandbox VMs** (79 §7): the preflight card, then one row per `backend: vm` record. **Add a
 * sandbox VM** is disabled while the host has blockers — the first one is its tooltip — so "your system does not meet
 * the requirements" is said in the button as well as the card.
 */
export function SandboxVms() {
  const preflight = usePreflight()
  const list = useSandboxes()
  const act = useSandboxActions()
  const [create, setCreate] = useState(false)
  const rows = (list.data ?? []).filter((s) => s.backend === 'vm')
  const blockers = preflight.data?.blockers ?? []
  const blocked = preflight.isPending || blockers.length > 0
  const why = preflight.isPending ? 'Checking this host…' : blockers[0]?.message
  return (
    <Section
      title="Sandbox VMs"
      actions={
        <span title={why} className="inline-flex">
          <Button size="sm" disabled={blocked} aria-label="Add a sandbox VM" onClick={() => setCreate(true)}>
            Add a sandbox VM
          </Button>
        </span>
      }
    >
      {preflight.isPending && <Empty>Checking this host…</Empty>}
      {preflight.isError && <Empty>Preflight failed: {preflight.error.message}</Empty>}
      {preflight.data && <SandboxPreflightCard preflight={preflight.data} onInstall={() => act.install.mutate(undefined)} installing={act.install.isPending} />}
      {list.isError && <Empty>{list.error.message}</Empty>}
      {list.data && rows.length === 0 && <Empty>No sandbox VM yet. One is a Linux desktop that joins your mesh and receives your vault, like any engine.</Empty>}
      {rows.map((s) => (
        <SandboxRow key={s.name} vm={s} preflight={preflight.data} />
      ))}
      {preflight.data && <SandboxCreateSheet open={create} onOpenChange={setCreate} preflight={preflight.data} taken={rows} create={act.create} />}
    </Section>
  )
}

const CODE_RE = /^[\w./~-]+(\s|$)|^(sudo|brew|curl|apt|usermod)\b/

/**
 * Host readiness (79 §5.1). Green: the runtime, virtualization, the host's cores / memory / free disk and the derived
 * defaults. Red: one sentence per blocker (`message`), its `fix` as an instruction line, and **Set up runtime** when a
 * blocker is `installable` or the embedded runtime is not extracted yet (the first create does it too). Amber: warnings,
 * which never disable anything.
 */
export function SandboxPreflightCard({ preflight: p, onInstall, installing }: { preflight: Preflight; onInstall: () => void; installing: boolean }) {
  const log = useSandboxLog('runtime')
  const ok = p.blockers.length === 0
  const pending = p.runtime.state === 'embedded' && !p.runtime.extracted
  const installable = pending || p.blockers.some((b) => b.installable)
  const runtime =
    p.runtime.state === 'embedded'
      ? pending
        ? `microsandbox ${p.runtime.version} embedded — set up on first use (no download)`
        : `microsandbox ${p.runtime.version} (embedded, in ${p.runtime.home})`
      : p.runtime.state === 'installed'
        ? `microsandbox ${p.runtime.version} (libkrunfw ${p.runtime.libkrunfw})`
        : p.runtime.state === 'missing'
          ? 'no runtime in this build'
          : `runtime incomplete: ${p.runtime.why}`
  const virt = p.virt.state === 'ok' ? `virtualization via ${p.virt.via}` : `no virtualization: ${p.virt.why}`
  return (
    <div className={cn('my-2 flex flex-col gap-2 rounded-xl border px-4 py-3 text-[13px]', ok ? 'border-success-border bg-success-bg' : 'border-danger-border bg-danger-bg')} data-testid="sandbox-preflight">
      {ok ? (
        <p className="text-success">
          <span className="font-medium">This host can run sandbox VMs.</span> {runtime} · {virt}.
        </p>
      ) : (
        <>
          <p className="font-medium text-danger">This host does not meet the requirements for a sandbox VM yet:</p>
          <ul className="flex flex-col gap-1.5">
            {p.blockers.map((b) => (
              <li key={b.code} className="text-danger">
                {b.message}
                {b.fix && (CODE_RE.test(b.fix) ? <code className="mt-0.5 block rounded bg-layer-2 px-1.5 py-0.5 font-mono text-xs text-foreground">{b.fix}</code> : <span className="block text-fg-secondary">{b.fix}</span>)}
              </li>
            ))}
          </ul>
        </>
      )}
      <p className="text-fg-secondary">
        {p.host.os} {p.host.arch} · {p.host.cores} cores · {fmtBytes(p.host.mem_bytes)} · {fmtBytes(p.host.disk_free_bytes)} free · defaults {p.defaults.cpus} CPUs (up to {p.defaults.max_cpus}) · {fmtMib(p.defaults.memory_mib)} (up to {fmtMib(p.defaults.max_memory_mib)}) · {fmtMib(p.defaults.root_disk_mib)} disk
      </p>
      {p.warnings.length > 0 && (
        <ul className="flex flex-col gap-1 text-warning">
          {p.warnings.map((w) => (
            <li key={w.code}>
              {w.message}
              {w.fix && <span className="text-fg-secondary"> — {w.fix}</span>}
            </li>
          ))}
        </ul>
      )}
      {installable && (
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" disabled={installing} onClick={onInstall}>
            <Download /> {installing ? 'Setting up…' : 'Set up runtime'}
          </Button>
          <span className="text-xs text-fg-tertiary">writes the embedded msb + libkrunfw into its home; no root</span>
        </div>
      )}
      {(installing || log.length > 0) && <StepLog lines={log} />}
    </div>
  )
}

/** The bring-up / install transcript: `step · line`, pinned to the bottom (the launch transcript's `<pre>`, `StartupScreen.tsx`). */
export function StepLog({ lines }: { lines: ProgressLine[] }) {
  const pre = useRef<HTMLPreElement>(null)
  useEffect(() => {
    pre.current?.scrollTo(0, pre.current.scrollHeight)
  }, [lines.length])
  return (
    <pre ref={pre} className="max-h-48 overflow-auto rounded-md bg-layer-2 p-3 font-mono text-xs leading-5 whitespace-pre-wrap break-all text-fg-secondary">
      {lines.length === 0 ? 'waiting for the first line…' : lines.map((l, i) => `${l.step.padEnd(10)} ${l.line}${l.pct !== null ? ` (${l.pct}%)` : ''}${i < lines.length - 1 ? '\n' : ''}`)}
    </pre>
  )
}

/** 79 §5.5 / smoke check 7: `memory_limit_bytes` is the hypervisor's allocation, which live resizes move; the guest's `MemTotal` does not follow. */
export const MEMORY_ALLOCATED_TIP = "hypervisor allocation; the guest's MemTotal stays at its boot size"

/** **Terminal** (79 §7): F2's `sandbox_terminal` starts a login shell as a PTY job; 55's pop-out shows it. */
export async function openVmTerminal(api: Pick<Api, 'sandbox_terminal'>, name: string): Promise<void> {
  try {
    const { session, job } = await api.sandbox_terminal(name)
    await openPopout({ kind: 'pty', session, job })
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e))
  }
}

function Gauge({ label, pct, text, title }: { label: string; pct: number | null; text: string; title?: string }) {
  const p = pct === null ? 0 : Math.max(0, Math.min(100, pct))
  return (
    <div className="flex min-w-24 flex-1 flex-col gap-0.5" role="meter" aria-label={label} aria-valuenow={pct ?? undefined} aria-valuemin={0} aria-valuemax={100} title={title}>
      <div className="flex justify-between text-[11px] text-fg-tertiary">
        <span>{label}</span>
        <span className="tabular-nums">{text}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-layer-3">
        <div className={cn('h-full rounded-full', p > 90 ? 'bg-danger' : p > 75 ? 'bg-warning' : 'bg-success')} style={{ width: `${p}%` }} />
      </div>
    </div>
  )
}

const BADGE: Record<SandboxInfo['state']['state'], string> = {
  creating: 'bg-info-bg text-info border-info-border',
  starting: 'bg-info-bg text-info border-info-border',
  ready: 'bg-layer-2 text-fg-secondary border-border-subtle',
  running: 'bg-success-bg text-success border-success-border',
  paused: 'bg-warning-bg text-warning border-warning-border',
  stopped: 'bg-layer-2 text-fg-secondary border-border-subtle',
  crashed: 'bg-danger-bg text-danger border-danger-border',
  gone: 'bg-danger-bg text-danger border-danger-border',
  failed: 'bg-danger-bg text-danger border-danger-border',
  destroying: 'bg-warning-bg text-warning border-warning-border',
}

/**
 * One VM (79 §7): name · state badge · image tag; three gauges from `vm.metrics` (refreshed by every `Changed`); shares
 * as chips; the mesh peer's reachability and running sessions; pending markers; the step log while creating; step +
 * error + Retry / Destroy when failed. Actions: Open desktop, Start/Stop, and a menu with Terminal, Resize, Shared
 * folders, Snapshots, Pause/Resume, Destroy.
 */
export function SandboxRow({ vm: s, preflight }: { vm: SandboxInfo; preflight: Preflight | undefined }) {
  const canQuery1 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const act = useSandboxActions()
  const mesh = useMesh()
  const sessions = useQuery({ enabled: canQuery1, queryKey: queryKeys.sessions, queryFn: () => api.list_sessions() })
  const handoffs = useHandoffs(!!s.engine)
  const log = useSandboxLog(s.name)
  const [sheet, setSheet] = useState<'resize' | 'shares' | 'snapshots' | 'destroy' | null>(null)
  const [keep, setKeep] = useState(true)
  const v = s.vm
  const st = s.state
  const m = v?.metrics ?? null
  const peer = s.engine ? mesh.data?.peers.find((p) => p.id === s.engine) : undefined
  const running = s.engine ? (sessions.data ?? []).filter((r) => r.owner === s.engine && r.running).length : 0
  const handoff = unresolved(handoffs.data, s.engine)
  const busy = Object.values(act).some((a) => a.isPending)
  const image = v ? (v.image.kind === 'oci' ? v.image.ref.split('/').pop() : `bootstrap · ${v.image.base.split('/').pop()}`) : s.backend
  const desktop = v?.desktop?.enabled === true && st.state === 'running' && canPopout()
  const pending = v ? [v.pending.shares && 'shares', v.pending.ports && 'ports', v.pending.resources && 'resources'].filter((x): x is string => !!x) : []
  return (
    <div className="flex flex-col gap-2 border-b border-border-subtle py-3 last:border-0" data-testid={`sandbox-row-${s.name}`}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-medium">{s.name}</span>
        <span className={cn('rounded-full border px-2 py-0.5 text-[11px]', BADGE[st.state])}>{stateLabel(st)}</span>
        <span className="truncate font-mono text-xs text-fg-tertiary" title={v?.image.kind === 'oci' ? v.image.ref : undefined}>
          {image}
        </span>
        {s.engine && (
          <span className="flex items-center gap-1 text-xs text-fg-secondary">
            <Dot on={peer?.reachable ?? false} /> {peer ? (peer.reachable ? 'online' : 'offline') : 'not paired'}
            {running > 0 && ` · ${running} running`}
          </span>
        )}
        {pending.length > 0 && <span className="rounded-full bg-warning-bg px-2 py-0.5 text-[11px] text-warning">restart to apply {pending.join(', ')}</span>}
        {handoff && <span className="rounded-full bg-info-bg px-2 py-0.5 text-[11px] text-info">{handoff.agent ?? handoff.session.slice(-6)} needs you</span>}
        <span className="ml-auto flex items-center gap-1">
          {st.state === 'failed' ? (
            <>
              <Button size="sm" variant="outline" disabled={busy} onClick={() => act.retry.mutate(s.name)}>
                <RotateCcw /> Retry
              </Button>
              <Button size="sm" variant="outline" className="text-danger" disabled={busy} onClick={() => setSheet('destroy')}>
                <Trash2 /> Destroy
              </Button>
            </>
          ) : (
            <>
              {desktop && (
                <Button size="sm" variant="outline" onClick={() => void openPopout({ kind: 'vm', name: s.name })}>
                  <Monitor /> Open desktop
                </Button>
              )}
              {st.state === 'running' && (
                <Button size="sm" variant="ghost" disabled={busy} aria-label={`Stop ${s.name}`} onClick={() => act.stop.mutate(s.name)}>
                  <Square /> Stop
                </Button>
              )}
              {canStart(st) && (
                <Button size="sm" variant="ghost" disabled={busy} aria-label={`Start ${s.name}`} onClick={() => act.start.mutate(s.name)}>
                  <Play /> Start
                </Button>
              )}
              {st.state === 'paused' && (
                <Button size="sm" variant="ghost" disabled={busy} onClick={() => act.resume.mutate(s.name)}>
                  <Play /> Resume
                </Button>
              )}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button size="icon-sm" variant="ghost" aria-label={`More actions for ${s.name}`}>
                    <MoreHorizontal />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem disabled={st.state !== 'running' || !canPopout()} onSelect={() => void openVmTerminal(api, s.name)}>
                    <Terminal /> Terminal
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => setSheet('resize')}>
                    <Scaling /> Resize…
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => setSheet('shares')}>
                    <FolderOpen /> Shared folders…
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => setSheet('snapshots')}>
                    <Camera /> Snapshots…
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  {st.state === 'running' && (
                    <DropdownMenuItem onSelect={() => act.pause.mutate(s.name)}>
                      <Pause /> Pause
                    </DropdownMenuItem>
                  )}
                  <DropdownMenuItem className="text-danger" onSelect={() => setSheet('destroy')}>
                    <Trash2 /> Destroy…
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </>
          )}
        </span>
      </div>
      {v && (
        <div className="flex flex-wrap gap-4">
          <Gauge label="CPU" pct={m ? m.cpu_percent : null} text={m ? `${Math.round(m.cpu_percent)} % of ${v.resources.cpus}` : `${v.resources.cpus} CPUs`} />
          <Gauge
            label="Memory"
            title={MEMORY_ALLOCATED_TIP}
            pct={m && m.memory_limit_bytes > 0 ? (m.memory_bytes / m.memory_limit_bytes) * 100 : null}
            text={m ? `${fmtBytes(m.memory_bytes)} of ${fmtBytes(m.memory_limit_bytes)} allocated` : `${fmtMib(v.resources.memory_mib)} allocated`}
          />
          <Gauge
            label="Disk"
            pct={m && m.disk_used_bytes + m.disk_free_bytes > 0 ? (m.disk_used_bytes / (m.disk_used_bytes + m.disk_free_bytes)) * 100 : null}
            text={m ? `${fmtBytes(m.disk_used_bytes)} / ${fmtBytes(m.disk_used_bytes + m.disk_free_bytes)}` : fmtMib(v.resources.root_disk_mib)}
          />
        </div>
      )}
      {v && v.shares.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {v.shares.map((sh) => (
            <span key={sh.guest} className="rounded-full bg-layer-2 px-2 py-0.5 font-mono text-[11px] text-fg-secondary" title={`${sh.host} → ${sh.guest}`}>
              {sh.guest}
              {sh.readonly && ' · ro'}
            </span>
          ))}
        </div>
      )}
      {st.state === 'failed' && (
        <p className="text-[13px] text-danger">
          Failed at <span className="font-medium">{st.step}</span>: {st.error}
        </p>
      )}
      {/* F2 streams `Progress { step: "start" }` while `Start` recreates: the log stays visible from the click (still `stopped`) through `starting`. */}
      {(st.state === 'creating' || st.state === 'failed' || st.state === 'starting' || act.start.isPending) && log.length > 0 && <StepLog lines={log} />}
      {v && preflight && <SandboxResizeSheet open={sheet === 'resize'} onOpenChange={(o) => setSheet(o ? 'resize' : null)} vm={s} preflight={preflight} resize={act.resize} />}
      {v && <SandboxSharesSheet open={sheet === 'shares'} onOpenChange={(o) => setSheet(o ? 'shares' : null)} vm={s} setShares={act.setShares} stop={act.stop} start={act.start} />}
      {v && <SandboxSnapshots open={sheet === 'snapshots'} onOpenChange={(o) => setSheet(o ? 'snapshots' : null)} vm={s} snapshot={act.snapshot} restore={act.restore} />}
      <AlertDialog open={sheet === 'destroy'} onOpenChange={(o) => setSheet(o ? 'destroy' : null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Destroy {s.name}?</AlertDialogTitle>
            <AlertDialogDescription>The VM, its disk and its mesh membership are removed. Snapshots are kept unless you say otherwise.</AlertDialogDescription>
          </AlertDialogHeader>
          <label className="flex items-center gap-2 text-sm">
            <Switch checked={keep} onCheckedChange={setKeep} aria-label="Keep snapshots" /> Keep snapshots
          </label>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="bg-destructive text-primary-foreground hover:bg-destructive/90" onClick={() => act.destroy.mutate({ name: s.name, keep })}>
              Destroy
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
