import { Link } from '@tanstack/react-router'
import { type ReactNode, useMemo } from 'react'
import { useSessionDrag } from '@/components/chat/Panes'
import { observedLabel, stripRows } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { AgentInfo, AgentState } from '@/rpc/extra'
import { useSessionField } from '@/store/sessions'
import { WatchDot } from './ObservedBar'

/**
 * 93: `● ● ●` centred above the to-do card, else directly above the composer (no label — Sky, 2026-10-04; above the
 * to-do — Sky, 2026-10-06, 112 §1) — one dot per live subagent (any depth) of the open session, in spawn order,
 * blinking on that subagent's streaming events (`AgentInfo.beat`, stamped by the reducer). Nothing when the subtree
 * is quiet; one row, never more, `+N` past `STRIP_CAP`. `trailing` (125 §8.2: the advisor pill) rides the same row after
 * the dots and keeps it rendered on its own when no subagent is live.
 */
export function SubagentStrip({ session, trailing }: { session: string; trailing?: ReactNode }) {
  const children = useSessionField(session, 'children')
  const { shown, more } = useMemo(() => stripRows(children ?? {}), [children])
  if (shown.length === 0 && !trailing) return null
  const n = shown.length + more
  return (
    <div className="mx-auto flex h-7 w-full shrink-0 select-none items-center justify-center gap-3 px-4 text-xs text-fg-tertiary md:max-w-[768px]">
      {shown.length > 0 && (
        <div className="flex items-center gap-1.5 overflow-hidden whitespace-nowrap" aria-label={`${n} subagent${n === 1 ? '' : 's'} running`}>
          {shown.map((row) => (
            <SubagentDot key={row.id} row={row} />
          ))}
          {more > 0 && <span className="tabular-nums">+{more}</span>}
        </div>
      )}
      {trailing}
    </div>
  )
}

/** The sidebar tree's phrasing of a row's state (`AgentTree`). */
const stateText = (s: AgentState): string =>
  s.state === 'awaiting' ? `awaiting ${s.from}` : s.state === 'approval' ? `approval: ${s.tool}` : s.state

/**
 * One subagent: the connected-state dot (green; amber when blocked on an approval) inside a 24 px link that opens
 * the subagent's chat or drags it into a pane. The circle is keyed on `beat`, so each beat remounts it and the
 * one-shot `subagent-blink` animation restarts — no timers. A watch-only child (141 §9.2c) is the same dot in its
 * harness state's colour, titled `watching`.
 */
function SubagentDot({ row }: { row: AgentInfo }) {
  const drag = useSessionDrag(row.id, row.name)
  const observed = row.observed ?? undefined
  const label = `${row.name} · ${observed ? `watching · ${observedLabel[observed.state]}` : stateText(row.state)}${row.model ? ` · ${row.model}` : ''}`
  return (
    <Link
      to="/c/$session"
      params={{ session: row.id }}
      {...drag}
      className="subagent-dot-in flex size-6 shrink-0 items-center justify-center"
      title={label}
      aria-label={label}
    >
      {observed ? (
        <WatchDot state={observed.state} beat={row.beat} />
      ) : (
        <span
          key={row.beat ?? 0}
          className={cn(
            'subagent-dot size-2 rounded-full',
            row.state.state === 'approval' ? 'bg-warning' : 'bg-success',
            row.beat !== undefined && 'subagent-blink',
          )}
        />
      )}
    </Link>
  )
}
