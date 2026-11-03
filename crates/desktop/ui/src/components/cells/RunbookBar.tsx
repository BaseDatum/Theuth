// Owner: U1
import { MoreHorizontal, Play } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isApple } from '@/lib/chord'
import { cn } from '@/lib/utils'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { CellState } from '@/rpc/bindings/CellState'
import type { KernelState } from '@/rpc/bindings/KernelState'
import { ended, exitText } from './store'
import { labelOf, titleOf } from './view'

export type RunbookBarProps = {
  /** The reply's cells, in order, with their state. */
  cells: { ref: CellRef; state: CellState | null; spec: CellSpec }[] // 171:U1-ADD `spec`: danger and labels name the boundary
  kernel: KernelState | null
  onRunAll(): void
  onOpenAsNote(): void
  /** 171:U1-ADD: false while the reply is still being written (`CellFence`'s "ready when the reply ends"). */
  runnable?: boolean
}

type Step = { spec: CellSpec; state: CellState | null }

/** Why Run all would stop before this cell (§7.2), seen in advance; `null` = it runs. An `after` gate on a cell earlier in this pass arms during the pass. */
function boundary(step: Step, before: Step[]): string | null {
  const o = step.spec.options
  const s = step.state
  if (o.danger) return 'danger'
  if (s?.problems.some((p) => p.severity === 'error')) return 'invalid'
  if (s?.problems.some((p) => p.code === 'cwd_not_found')) return 'folder not found'
  if (s?.chips.some((c) => c.stops_run_all)) return 'lint'
  if (o.after && !s?.armed && !before.some((b) => b.spec.options.name === o.after)) return 'not armed'
  return null
}

/** The latest Run-all pass that stopped: its failing step (0-based) and why. */
function stopped(steps: Step[]): { at: number; reason: string } | null {
  const runs = steps.map((s) => s.state?.latest ?? null)
  const passes = runs.filter((r) => r?.pass).sort((a, b) => (b?.queued_ms ?? 0) - (a?.queued_ms ?? 0))
  const pass = passes[0]?.pass
  if (!pass || runs.some((r) => r?.pass === pass && !ended(r))) return null
  const at = runs.findIndex((r) => r?.pass === pass && (r.state !== 'done' || r.expect.some((e) => !e.met)))
  if (at < 0) return null
  const r = runs[at]!
  const missed = r.expect.filter((e) => !e.met).map((e) => `expect “${e.item.value}” missed`)
  return { at, reason: [exitText(r), ...missed].join(' · ') }
}

/** 171 §7.2, §11.3: the bar's label — what Run all will do, or where the last pass stopped. */
export function runAllLabel(steps: Step[]): { label: string; done: number; stop: { at: number; reason: string } | null } {
  const live = steps.filter((s) => !s.state?.superseded_by)
  const done = live.filter((s) => s.state?.latest?.state === 'done').length
  const stop = stopped(live)
  if (stop) return { label: `Run all · stopped at a failed step (${stop.at + 1}/${live.length})`, done, stop }
  const at = live.findIndex((s, i) => boundary(s, live.slice(0, i)) !== null)
  const next = live[at]
  return { label: next ? `Run all · stops before ${titleOf(next.spec, at + 1)}` : 'Run all', done, stop: null }
}

const kernelText = (k: KernelState) => `${k.id.spec.type === 'shell' ? k.id.spec.kind : k.id.spec.type} · ${k.phase === 'ready' ? 'idle' : k.phase}`

/** 171 §11.3: Run all, progress segments and the kernel state above a reply with two or more cells. */
export function RunbookBar({ cells, kernel, onRunAll, onOpenAsNote, runnable = true }: RunbookBarProps) {
  const [why, setWhy] = useState(false)
  const { label, done, stop } = runAllLabel(cells)
  const busy = cells.some((c) => c.state?.latest && !ended(c.state.latest))
  const disabled = !runnable || busy
  const mod = isApple() ? '⌘' : 'Ctrl'
  return (
    <div
      role="toolbar"
      aria-label="Runbook"
      tabIndex={-1}
      className="sticky top-0 z-10 my-3 flex flex-wrap items-center gap-2 rounded-xl border border-border bg-layer-1/95 px-3 py-2 backdrop-blur pointer-coarse:flex-col pointer-coarse:items-stretch"
      onKeyDown={(e) => {
        // §11.7: Mod+Shift+Enter runs all while the bar is focused.
        if (e.key === 'Enter' && e.shiftKey && (isApple() ? e.metaKey : e.ctrlKey) && !disabled) {
          e.preventDefault()
          onRunAll()
        }
      }}
    >
      <Button
        size="sm"
        className="h-8 gap-1 pointer-coarse:h-12"
        aria-disabled={disabled || undefined}
        aria-description={!runnable ? 'ready when the reply ends' : busy ? 'a run is in progress' : undefined}
        title={`${label} (${mod}⇧↵)`}
        onClick={() => !disabled && onRunAll()}
      >
        <Play className="size-3.5" aria-hidden />
        {label}
      </Button>
      {stop && (
        <Button variant="link" size="sm" className="h-6 px-0 text-xs" aria-expanded={why} onClick={() => setWhy(!why)}>
          why?
        </Button>
      )}
      <div className="flex items-center gap-2">
        <div className="flex gap-1" aria-hidden>
          {cells.map((c, i) => {
            const st = c.state?.latest?.state
            const failed = (st && st !== 'done' && ended(c.state?.latest)) || c.state?.latest?.expect.some((e) => !e.met)
            return (
              <span
                key={i}
                title={labelOf(c.spec)}
                className={cn(
                  'h-1.5 w-6 rounded-full bg-layer-3',
                  st === 'done' && !failed && 'bg-success',
                  failed && 'bg-danger',
                  (st === 'running' || st === 'queued') && 'bg-fg-accent motion-safe:animate-pulse',
                  !st && c.spec.options.danger && 'border border-danger-border bg-transparent',
                )}
              />
            )
          })}
        </div>
        <span className="font-mono text-xs text-fg-secondary tabular-nums">
          {done}/{cells.length}
        </span>
        {kernel && <span className="text-xs text-fg-secondary">{kernelText(kernel)}</span>}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="sm" className="ml-auto size-8 p-0 pointer-coarse:size-12" aria-label="Runbook menu">
              <MoreHorizontal className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={onOpenAsNote}>Open as note</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {why && stop && <p className="basis-full text-xs text-danger">{`step ${stop.at + 1}: ${stop.reason}`}</p>}
    </div>
  )
}
