// Owner: U1
import { MoreHorizontal, Play } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isApple } from '@/lib/chord'
import { useDisclosure } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import type { CellRef } from "@/rpc/bindings/method.cells.output"
import type { Cell as CellSpec } from '@/rpc/bindings/auxiliary.surface.output'
import type { CellState } from "@/rpc/bindings/method.cells.output"
import type { KernelState } from "@/rpc/bindings/method.cells.output"
import type { PassEnd } from "@/rpc/bindings/method.cells.output"
import { digest } from './grammar'
import { liveKernels } from './KernelStrip'
import { ended, refKey } from './store'
import { labelOf, titleOf } from './view'

/** One step of the bar. `seen` (171F B8): the engine's digest of a followed replacement, which Run all sends as is. */
export type Step = { ref: CellRef; state: CellState | null; spec: CellSpec; seen?: string }

export type RunbookBarProps = {
  /** The reply's cells, in order, with their state, replacements followed (`followSuperseded`). */
  cells: Step[] // 171:U1-ADD `spec`: danger and labels name the boundary
  /** 171F B26: the scope's kernels; the bar shows only those its cells use (`kernelLabel`). */
  kernels: KernelState[]
  /** 171F B11: the scope's last Run all pass (`lastPass`); where it stopped, when that is one of these cells. */
  pass: PassEnd | null
  /** 171F B13: the `disclosureKey` of the bar's "why?" line. */
  whyKey: string
  onRunAll(): void
  onOpenAsNote(): void
  /** 171:U1-ADD: false while the reply is still being written (`CellFence`'s "ready when the reply ends"). */
  runnable?: boolean
}

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

/**
 * 171F B8: a step a newer reply replaced stands for its replacement once the replacement's state is loaded (`next[i]`,
 * the state of `superseded_by`): its ref, state and spec (`specOf`, else the old spec). Until then it stays as it is
 * and Run all skips it.
 */
export function followSuperseded(steps: Step[], next: (CellState | null)[], specOf: (ref: CellRef) => CellSpec | null): Step[] {
  return steps.map((s, i) => {
    const n = next[i]
    return s.state?.superseded_by && n ? { ref: n.cell, state: n, spec: specOf(n.cell) ?? s.spec, seen: n.digest } : s
  })
}

/** Run all's `(ref, seen)` pairs: a followed replacement as the engine presented it, the reply's own cells with the digest of their text. */
export const runAllPairs = (steps: Step[], bodies: string[]) =>
  Promise.all(steps.map(async (s, i): Promise<[CellRef, string]> => [s.ref, s.seen ?? (await digest(bodies[i] ?? ''))]))

/**
 * 171 §7.2, §11.3: the bar's label — what Run all will do, or where the scope's last pass stopped (171F B11: the
 * engine's `pass`, never guessed from runs). `done`/`total` count the live steps (replacements followed).
 */
export function runAllLabel(steps: Step[], pass: PassEnd | null = null): { label: string; done: number; total: number; stop: { at: number; reason: string } | null } {
  const live = steps.filter((s) => !s.state?.superseded_by)
  const done = live.filter((s) => s.state?.latest?.state === 'done').length
  const total = live.length
  if (total === 0) return { label: 'Replaced by a newer version ↓', done, total, stop: null }
  const stop = pass?.stopped
  const stoppedAt = stop ? live.findIndex((s) => refKey(s.ref) === refKey(stop.cell)) : -1
  if (stop && stoppedAt >= 0) {
    const where = stop.before ? `stopped before ${titleOf((live[stoppedAt] as Step).spec, stoppedAt + 1)}` : 'stopped at a failed step'
    return { label: `Run all · ${where} (${stoppedAt + 1}/${total})`, done, total, stop: { at: stoppedAt, reason: stop.why } }
  }
  const at = live.findIndex((s, i) => boundary(s, live.slice(0, i)) !== null)
  const next = live[at]
  return { label: next ? `Run all · stops before ${titleOf(next.spec, at + 1)}` : 'Run all', done, total, stop: null }
}

/** The program family of a kernel spec: the shell's kind, `python` or `monty`. */
const familyOf = (k: KernelState['id']['spec']) => (k.type === 'shell' ? k.kind : k.type)

/**
 * 171F B26: `bash · idle`, `bash + python · busy` — the kernels the bar's cells use (each step's latest run's kernel,
 * else its interpreter, else its language) and their phase among the scope's live kernels; never another cell's kernel.
 */
export function kernelLabel(steps: Step[], kernels: KernelState[]): string | null {
  const families = [...new Set(steps.map((s) => (s.state?.latest ? familyOf(s.state.latest.kernel.spec) : (s.state?.interpreter?.kind ?? s.spec.lang))))]
  if (families.length === 0) return null
  const mine = liveKernels(kernels).filter((k) => families.includes(familyOf(k.id.spec)))
  const phase = mine.some((k) => k.phase === 'busy') ? 'busy' : mine.some((k) => k.phase === 'starting') ? 'starting' : mine.length ? 'idle' : null
  return [families.join(' + '), phase].filter(Boolean).join(' · ')
}

/** 171 §11.3: Run all, progress segments and the kernels, after the last cell of a reply with two or more cells (171F B1: in the transcript's flow). */
export function RunbookBar({ cells, kernels, pass, whyKey, onRunAll, onOpenAsNote, runnable = true }: RunbookBarProps) {
  const [why, setWhy] = useDisclosure(whyKey)
  const { label, done, total, stop } = runAllLabel(cells, pass)
  const live = cells.filter((c) => !c.state?.superseded_by)
  const busy = cells.some((c) => c.state?.latest && !ended(c.state.latest))
  const disabled = !runnable || busy || total === 0
  const kernel = kernelLabel(live, kernels)
  const mod = isApple() ? '⌘' : 'Ctrl'
  return (
    <div
      role="toolbar"
      aria-label="Runbook"
      tabIndex={-1}
      className="my-3 flex flex-wrap items-center gap-2 rounded-xl border border-border bg-layer-1 px-3 py-2 pointer-coarse:flex-col pointer-coarse:items-stretch"
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
        className="h-8 gap-1 pointer-coarse:h-12 aria-disabled:cursor-not-allowed aria-disabled:opacity-50"
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
          {live.map((c, i) => {
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
          {done}/{total}
        </span>
        {kernel && <span className="text-xs text-fg-secondary">{kernel}</span>}
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
