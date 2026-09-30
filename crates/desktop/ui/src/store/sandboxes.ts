// 79 §7 / contract §2.11: the sandbox VM list lives in the `['sandboxes']` react-query cache and is patched in place by
// `SandboxEvent` (`Changed` upserts, `Removed` deletes); `Progress` lines go to a per-name log capped at `LOG_CAP`.
// Pure functions here; `applySandboxEvent` is the one place that touches the cache (`lib/rpc.ts` calls it).

import type { QueryClient } from '@tanstack/react-query'
import type { Preflight } from '@/rpc/bindings/Preflight'
import type { Resources } from '@/rpc/bindings/Resources'
import type { SandboxEvent } from '@/rpc/bindings/SandboxEvent'
import type { SandboxInfo } from '@/rpc/bindings/SandboxInfo'
import type { SandboxState } from '@/rpc/bindings/SandboxState'
import type { VmMetrics } from '@/rpc/bindings/VmMetrics'

export const SANDBOXES_KEY = ['sandboxes'] as const
export const sandboxLogKey = (name: string) => ['sandboxes', 'log', name] as const
export const PREFLIGHT_KEY = ['sandboxes', 'preflight'] as const
export const HANDOFFS_KEY = ['handoffs'] as const
export const LOG_CAP = 500

export type ProgressLine = { step: string; line: string; pct: number | null }

type Progress = Extract<SandboxEvent, { type: 'Progress' }>

/** `Changed`: replace the row of the same name, else append (a fresh `creating` row lands at the end). */
export function upsert(rows: readonly SandboxInfo[] | undefined, s: SandboxInfo): SandboxInfo[] {
  const list = rows ?? []
  return list.some((r) => r.name === s.name) ? list.map((r) => (r.name === s.name ? s : r)) : [...list, s]
}

export function remove(rows: readonly SandboxInfo[] | undefined, name: string): SandboxInfo[] {
  return (rows ?? []).filter((r) => r.name !== name)
}

/** Append one line; the oldest fall off past `LOG_CAP`. */
export function appendLog(log: readonly ProgressLine[] | undefined, e: Pick<Progress, 'step' | 'line' | 'pct'>): ProgressLine[] {
  const next = [...(log ?? []), { step: e.step, line: e.line, pct: e.pct }]
  return next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next
}

/** Fold a `Changed` / `Removed` / `Progress` into the caches. `OpenDesktop` and `Takeover` are window concerns (`lib/rpc.ts`, `VmDesktop`). */
export function applySandboxEvent(qc: QueryClient, e: SandboxEvent): void {
  switch (e.type) {
    case 'Changed':
      qc.setQueryData<SandboxInfo[]>(SANDBOXES_KEY, (rows) => upsert(rows, e.sandbox))
      return
    case 'Removed':
      qc.setQueryData<SandboxInfo[]>(SANDBOXES_KEY, (rows) => remove(rows, e.name))
      qc.removeQueries({ queryKey: sandboxLogKey(e.name) })
      return
    case 'Progress':
      qc.setQueryData<ProgressLine[]>(sandboxLogKey(e.name), (log) => appendLog(log, e))
      return
    default:
      return
  }
}

// ---- derived facts the sheets and rows share ----

export const MIB = 1024 * 1024
export const GIB = 1024 * MIB

/** `3.2 GiB` / `512 MiB`; one decimal above 10 units none. */
export function fmtBytes(n: number): string {
  if (n >= GIB) return `${trim(n / GIB)} GiB`
  if (n >= MIB) return `${trim(n / MIB)} MiB`
  return `${Math.round(n / 1024)} KiB`
}
const trim = (x: number) => (x >= 10 ? Math.round(x).toString() : x.toFixed(1).replace(/\.0$/, ''))
export const fmtMib = (mib: number) => fmtBytes(mib * MIB)

/** `vm-1`, `vm-2`, …: the first free number. */
export function nextVmName(rows: readonly Pick<SandboxInfo, 'name'>[]): string {
  const taken = new Set(rows.map((r) => r.name))
  for (let i = 1; ; i++) if (!taken.has(`vm-${i}`)) return `vm-${i}`
}

/** Slider bounds from the host (79 decision 6): cpus ≤ cores, memory ≤ host/2, disk ≥ 8 GiB and ≤ what is free. */
export function limits(p: Preflight): { cpus: [number, number]; memory_mib: [number, number]; root_disk_mib: [number, number] } {
  return {
    cpus: [1, Math.max(1, p.host.cores)],
    memory_mib: [1024, Math.max(1024, Math.floor(p.host.mem_bytes / 2 / MIB))],
    root_disk_mib: [8192, Math.max(8192, Math.floor(p.host.disk_free_bytes / MIB))],
  }
}

export const RESIZE_FLOOR_MIB = 512

/**
 * 79 §5.5: the server refuses a memory value under the guest's use + 512 MiB, a CPU count under 1, and a smaller disk;
 * the UI refuses the same before calling. `null` = allowed.
 */
export function resizeGuard(next: Resources, current: Resources, metrics: VmMetrics | null): string | null {
  if (next.cpus < 1) return 'A VM needs at least 1 CPU.'
  if (metrics && next.memory_mib * MIB < metrics.memory_bytes + RESIZE_FLOOR_MIB * MIB) {
    return `The guest is using ${fmtBytes(metrics.memory_bytes)}; memory cannot go below that plus ${RESIZE_FLOOR_MIB} MiB (${fmtMib(Math.ceil(metrics.memory_bytes / MIB) + RESIZE_FLOOR_MIB)}).`
  }
  if (next.root_disk_mib < current.root_disk_mib) return 'A root disk can only grow; shrinking is refused.'
  return null
}

/** Which of a resize's fields need a restart: the ceilings (79 §5.5). */
export function needsRestart(next: Resources, current: Resources): boolean {
  return next.max_cpus !== current.max_cpus || next.max_memory_mib !== current.max_memory_mib
}

/** `SandboxState` → badge text; `creating`/`failed` carry their step. */
export function stateLabel(s: SandboxState): string {
  switch (s.state) {
    case 'creating':
      return `creating · ${s.step}`
    case 'failed':
      return `failed at ${s.step}`
    default:
      return s.state
  }
}

export const canStart = (s: SandboxState) => s.state === 'ready' || s.state === 'stopped' || s.state === 'crashed'

/** `/shared/<basename>` (79 decision 7). */
export function guestPath(host: string): string {
  const base = host.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || 'root'
  return `/shared/${base}`
}
