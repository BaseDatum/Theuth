import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { useMemo, useState } from 'react'
import { describe } from '@/lib/describe'
import { costShort, usd } from '@/lib/money'
import { queryKeys, useApi } from '@/lib/rpc'
import { detailOf, observedLabel } from '@/lib/session-tree'
import { cn } from '@/lib/utils'
import type { ChildCost } from '@/rpc/bindings/ChildCost'
import type { AgentInfo } from '@/rpc/extra'
import type { Entry } from '@/store/sessions'
import { k } from './CostPill'
import { EyeMark, WatchDot } from './ObservedBar'

type Tool = Extract<Entry, { kind: 'tool' }>

/** The watch-only children a spawn tool call made, in the harness's order (`detail.index`), else by name. */
export function rosterRows(children: Readonly<Record<string, AgentInfo>>, callId: string): AgentInfo[] {
  return Object.values(children)
    .filter((r) => r.observed?.tool_call === callId)
    .sort((a, b) => (detailOf(a.observed).index ?? Number.POSITIVE_INFINITY) - (detailOf(b.observed).index ?? Number.POSITIVE_INFINITY) || a.name.localeCompare(b.name))
}

/** `2 subagents · 1 running · 1 done` from the children's states (not the tool's status — omp's call completes 5 ms in, §9.3). */
export function rosterFooter(rows: readonly AgentInfo[]): string {
  const n = rows.length
  const states = rows.map((r) => r.observed?.state)
  const running = states.filter((s) => s === 'running' || s === 'requires_action').length
  const done = states.filter((s) => s === 'completed').length
  const failed = states.filter((s) => s === 'failed' || s === 'cancelled').length
  const parts = [`${n} subagent${n === 1 ? '' : 's'}`]
  if (running) parts.push(`${running} running`)
  if (done) parts.push(`${done} done`)
  if (failed) parts.push(`${failed} failed`)
  return parts.join(' · ')
}

/** The row's money cell by its cost rule (§7.6): the meter's figure when counted, the harness's greyed when inside the parent, `—` otherwise. */
export function rosterCost(row: AgentInfo, metered: ChildCost | undefined): { text: string; grey: boolean; title?: string } {
  const rule = row.observed?.cost_rule
  const reported = detailOf(row.observed).cost
  if (rule === 'counted') {
    const text = metered ? costShort(metered.total) : reported !== undefined ? usd(reported) : '—'
    return { text, grey: false, title: 'counted — not in the harness’s own total' }
  }
  if (rule === 'inside_parent' && reported !== undefined) return { text: usd(reported), grey: true, title: 'inside parent — already in the harness’s own total' }
  return { text: '—', grey: true, title: rule === 'display_only' ? 'reported by the harness, not counted' : 'not reported by the harness' }
}

/** The roster's body from facts: one link row per child — dot · name · agent/description · activity · tools · tokens · cost · ›. */
export function RosterRows({ rows, costs }: { rows: readonly AgentInfo[]; costs?: readonly ChildCost[] }) {
  return (
    <ul className="divide-y divide-border-subtle">
      {rows.map((row) => {
        const o = row.observed
        const d = detailOf(o)
        const who = d.agent ?? o?.description ?? undefined
        const activity = d.lastIntent ?? d.currentTool ?? (row.last || undefined)
        const tokens = d.tokens ?? d.contextTokens
        const cost = rosterCost(row, costs?.find((c) => c.session === row.id))
        const label = `${row.name} · watching · ${o ? observedLabel[o.state] : ''}`
        return (
          <li key={row.id}>
            <Link
              to="/c/$session"
              params={{ session: row.id }}
              className="flex flex-wrap items-center gap-x-3 gap-y-1 px-1 py-1.5 text-sm hover:bg-layer-2"
              title={label}
              aria-label={label}
            >
              {o && <WatchDot state={o.state} beat={row.beat} className="w-3.5" />}
              <span className="min-w-[70px] font-medium">{row.name}</span>
              <span className="min-w-0 flex-1 basis-40 truncate text-fg-secondary">
                {who}
                {who && activity && ' · '}
                {activity}
              </span>
              {d.completionPercent !== undefined && (
                <span className="h-1 w-16 shrink-0 overflow-hidden rounded-full bg-layer-2" role="progressbar" aria-valuenow={d.completionPercent} aria-valuemin={0} aria-valuemax={100}>
                  <span className="block h-full bg-success" style={{ width: `${Math.max(0, Math.min(100, d.completionPercent))}%` }} />
                </span>
              )}
              {(d.toolCount !== undefined || tokens !== undefined) && (
                <span className="whitespace-nowrap text-xs tabular-nums text-fg-tertiary">
                  {d.toolCount !== undefined && `${d.toolCount} tool${d.toolCount === 1 ? '' : 's'}`}
                  {d.toolCount !== undefined && tokens !== undefined && ' · '}
                  {tokens !== undefined && `${k(tokens)} tok`}
                </span>
              )}
              <span className={cn('whitespace-nowrap text-xs tabular-nums', cost.grey ? 'text-fg-tertiary opacity-60' : 'text-fg-tertiary')} title={cost.title}>
                {cost.text}
              </span>
              <ChevronRight className="ml-auto size-3.5 shrink-0 opacity-40" aria-hidden />
            </Link>
          </li>
        )
      })}
    </ul>
  )
}

/**
 * 141 §9.2(b): the spawn tool call as a roster — one row per watch-only child it made (Q4: the roster replaces the
 * card body; the header keeps the tool's title and a disclosure to the raw arguments/output). Rows beat with their
 * children; money per the cost rule, from the parent's `session_cost` (warm from the sidebar tree) when counted.
 */
export function RosterCard({ tool, session, rows }: { tool: Tool; session: string; rows: readonly AgentInfo[] }) {
  const api = useApi()
  const [raw, setRaw] = useState(false)
  const lines = useMemo(() => describe(tool.name, tool.args), [tool.name, tool.args])
  const { data: cost } = useQuery({ queryKey: queryKeys.sessionCost(session), queryFn: () => api.session_cost(session) })
  const live = rows.some((r) => r.observed?.state === 'running' || r.observed?.state === 'requires_action')
  const Chevron = raw ? ChevronDown : ChevronRight
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-3 py-2 text-sm">
      <div className="flex items-center gap-2 px-1 pb-1.5 font-medium">
        <span className={cn('size-2 shrink-0 rounded-full', live ? 'bg-success subagent-dot animate-pulse' : 'bg-fg-tertiary opacity-40')} aria-hidden />
        <EyeMark />
        <span className="font-mono text-xs">{tool.name}</span>
        <span className="truncate text-xs font-normal text-fg-secondary">{rosterFooter(rows)}</span>
        <button type="button" className="ml-auto shrink-0 rounded-md p-0.5 text-fg-tertiary hover:bg-layer-2" aria-expanded={raw} aria-label="raw tool text" onClick={() => setRaw((v) => !v)}>
          <Chevron className="size-3.5" />
        </button>
      </div>
      <RosterRows rows={rows} costs={cost?.children} />
      {raw && lines.length > 0 && <pre className="mt-2 w-full font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap text-fg-secondary">{lines.join('\n')}</pre>}
      {raw && tool.result && (
        <pre className="mt-2 max-h-56 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">{tool.result.output}</pre>
      )}
    </div>
  )
}
