import { Link } from '@tanstack/react-router'
import { useMemo } from 'react'
import { useSessionDrag } from '@/components/chat/Panes'
import { stripRows } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { AgentInfo, AgentState } from '@/rpc/extra'
import { useSessionField } from '@/store/sessions'

/**
 * 93: `● ● ●` centred above the composer (no label — Sky, 2026-10-04) — one dot per live subagent (any depth) of the
 * open session, in spawn order, blinking on that subagent's streaming events (`AgentInfo.beat`, stamped by the
 * reducer). Nothing when the subtree is quiet; one row, never more, `+N` past `STRIP_CAP`.
 */
export function SubagentStrip({ session }: { session: string }) {
  const children = useSessionField(session, 'children')
  const { shown, more } = useMemo(() => stripRows(children ?? {}), [children])
  if (shown.length === 0) return null
  return (
    <div className="mx-auto flex h-7 w-full shrink-0 select-none items-center justify-center px-4 text-xs text-fg-tertiary md:max-w-[768px]" aria-label={`${shown.length + more} subagent${shown.length + more === 1 ? '' : 's'} running`}>
      <div className="flex items-center gap-1.5 overflow-hidden whitespace-nowrap">
        {shown.map((row) => (
          <SubagentDot key={row.id} row={row} />
        ))}
        {more > 0 && <span className="tabular-nums">+{more}</span>}
      </div>
    </div>
  )
}

/** The sidebar tree's phrasing of a row's state (`AgentTree`). */
const stateText = (s: AgentState): string =>
  s.state === 'awaiting' ? `awaiting ${s.from}` : s.state === 'approval' ? `approval: ${s.tool}` : s.state

/**
 * One subagent: the connected-state dot (green; amber when blocked on an approval) inside a 24 px link that opens
 * the subagent's chat or drags it into a pane. The circle is keyed on `beat`, so each beat remounts it and the
 * one-shot `subagent-blink` animation restarts — no timers.
 */
function SubagentDot({ row }: { row: AgentInfo }) {
  const drag = useSessionDrag(row.id, row.name)
  const label = `${row.name} · ${stateText(row.state)}${row.model ? ` · ${row.model}` : ''}`
  return (
    <Link
      to="/c/$session"
      params={{ session: row.id }}
      {...drag}
      className="subagent-dot-in flex size-6 shrink-0 items-center justify-center"
      title={label}
      aria-label={label}
    >
      <span
        key={row.beat ?? 0}
        className={cn(
          'subagent-dot size-2 rounded-full',
          row.state.state === 'approval' ? 'bg-warning' : 'bg-success',
          row.beat !== undefined && 'subagent-blink',
        )}
      />
    </Link>
  )
}
