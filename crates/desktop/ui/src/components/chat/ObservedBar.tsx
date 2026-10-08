import { Link } from '@tanstack/react-router'
import { Eye } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { useApi } from '@/lib/rpc'
import { harnessName, observedLabel, sourceTag } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { ObservedMeta } from '@/rpc/bindings/ObservedMeta'
import type { ObservedState } from '@/rpc/bindings/ObservedState'
import { useSessionField, useSessions } from '@/store/sessions'

/** 141 mock: a solid dot while live, a ring while resting, a dashed ring once contact is lost; `completed` is a check. */
const DOT: Record<Exclude<ObservedState, 'completed'>, string> = {
  running: 'bg-success',
  requires_action: 'bg-warning',
  failed: 'bg-danger',
  cancelled: 'bg-fg-tertiary',
  pending: 'border-[1.5px] border-current opacity-45',
  idle: 'border-[1.5px] border-current opacity-45',
  unknown: 'border border-dashed border-current opacity-60',
}

/**
 * The state mark every watch-only surface shares (tree row, strip, roster, bar): colour by the harness's state, a
 * `✓` once finished (Q1: `idle` is not finished). Re-keyed on `beat` so the one-shot `subagent-blink` restarts per
 * streaming event (93) — only while the state is live, a resting row sits still. `sr-only` text names the state.
 */
export function WatchDot({ state, beat, className }: { state: ObservedState; beat?: number; className?: string }) {
  if (state === 'completed')
    return (
      <span className={cn('inline-block w-2 text-center text-xs leading-none text-success', className)}>
        ✓<span className="sr-only"> finished</span>
      </span>
    )
  const live = state === 'running' || state === 'requires_action'
  return (
    <span className={cn('inline-flex w-2 shrink-0 items-center justify-center', className)}>
      <span key={live ? (beat ?? 0) : 0} className={cn('subagent-dot block size-2 rounded-full', DOT[state], live && beat !== undefined && 'subagent-blink')} />
      <span className="sr-only">{observedLabel[state]}</span>
    </span>
  )
}

/** The eye every watch-only row carries, labelled for screen readers. */
export function EyeMark({ className }: { className?: string }) {
  return <Eye className={cn('size-3 shrink-0 opacity-70', className)} aria-label="watch-only" role="img" />
}

/** `omp task` / `opencode` / `ACP`: which feed describes the child. */
export function SourceTag({ observed, className }: { observed: Pick<ObservedMeta, 'source'>; className?: string }) {
  return <span className={cn('shrink-0 rounded-sm border border-border-subtle px-1 text-[10px] leading-4 text-fg-tertiary', className)}>{sourceTag(observed)}</span>
}

/** The bar's second line: why there is no composer, plus the one-line caveat a source or state adds. */
export function observedNote(observed: ObservedMeta, harness: string): string {
  const base = `${harness} runs this subagent, so you can watch it but not message it.`
  if (observed.state === 'unknown') return `${base} theuth lost the harness; the last known state is shown.`
  if (observed.state === 'idle') return `${base} idle — ${harness} may resume it.`
  if (observed.source === 'opencode_task') return `${base} opencode 1.x does not stream subagent activity.`
  return base
}

/** The bar itself, from facts: rendered where the composer would be. `onStop` is called only when the harness can stop the child. */
export function ObservedBarView({
  observed,
  harness,
  parent,
  onStop,
}: {
  observed: ObservedMeta
  harness: string
  parent: string | null | undefined
  onStop(): void
}) {
  const stop = observed.can_cancel
  const cannot = `${harness} cannot stop one subagent — stop the parent`
  return (
    <div
      role="status"
      className="mx-auto mb-3 flex w-full shrink-0 flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border border-dashed border-fg-accent/40 bg-fg-accent/5 px-3 py-2 text-sm md:max-w-[768px]"
    >
      <span className="flex shrink-0 items-center gap-2 font-medium">
        <WatchDot state={observed.state} />
        {observedLabel[observed.state]}
      </span>
      <span className="min-w-[180px] flex-1 text-xs text-fg-secondary">{observedNote(observed, harness)}</span>
      {parent && (
        <Button asChild variant="outline" size="sm">
          <Link to="/c/$session" params={{ session: parent }}>
            Open parent
          </Link>
        </Button>
      )}
      <Button variant="outline" size="sm" disabled={!stop} aria-disabled={!stop} title={stop ? undefined : cannot} onClick={stop ? onStop : undefined}>
        Stop subagent
      </Button>
    </div>
  )
}

/** 141 §9.2(d): the watch-only session's bar in the composer's place — state, why, Open parent, Stop subagent (RFD `cancel` only). */
export function ObservedBar({ session }: { session: string }) {
  const api = useApi()
  const observed = useSessionField(session, 'observed')
  const parent = useSessionField(session, 'parent')
  const parentAgent = useSessions((s) => (parent ? s.sessions[parent]?.agent : undefined))
  if (!observed) return null
  const stop = () => api.cancel(session, 'root').catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
  return <ObservedBarView observed={observed} harness={harnessName(observed, parentAgent)} parent={parent} onStop={() => void stop()} />
}
