// 53 §4.4: the `[meeting …]` lines in a meeting's session get their own card — a left-aligned notice with a
// chip naming what it is (transcript batch, request, tap, dismissal, deeper, start) instead of a user bubble,
// as engine nudges do. The text stays literal; nothing is hidden from the record. 57 §3.4: a transcript
// batch the decision oracle handled gets one dim Zap status row under it (expandable to the probabilities),
// from the live meeting store — batches of past meetings show none.

import { Mic, Zap } from 'lucide-react'
import { type MeetingLine, batchStamps, oracleRow } from '@/lib/meeting-line'
import { cn } from '@/lib/utils'
import type { OracleDecision } from "@/rpc/bindings/event.meeting_2eevents.output"
import { useMeeting } from '@/store/meeting'
import { Details } from './disclosure'

/** One `them:`/`me:` row of a batch, or the `live:/dismissed:` header. */
function BatchRow({ line }: { line: string }) {
  const me = line.startsWith('me: ')
  const them = line.startsWith('them: ')
  if (!me && !them) return <div className="text-fg-tertiary">{line}</div>
  return (
    <div className="flex gap-2">
      <span className={cn('w-10 shrink-0 text-right', me ? 'text-fg-accent' : 'text-fg-tertiary')}>{me ? 'me' : 'them'}</span>
      <span className="min-w-0 wrap-anywhere">{line.slice(me ? 4 : 6)}</span>
    </div>
  )
}

/** 57 §3.4: Zap + `topic 0.12 · skip · 78 ms`; open for every label's probability. */
export function OracleRow({ decision }: { decision: OracleDecision }) {
  return (
    <Details
      part={`oracle|${decision.batch_id}`}
      className="mt-1 text-[11px] leading-5 text-fg-tertiary"
      triggerClassName="truncate"
      trigger={
        <span title={`${decision.mode} · ${decision.family}`}>
          <Zap className="mr-1 inline size-3" />
          {decision.batch_id !== '' && <span className="mr-1 tabular-nums">{decision.batch_id}</span>}
          {oracleRow(decision)}
        </span>
      }
    >
      <ul className="ml-4 list-disc font-mono text-[11px]">
        {decision.top.map(([label, prob]) => (
          <li key={label}>
            {label} {prob.toFixed(3)}
          </li>
        ))}
      </ul>
    </Details>
  )
}

export function MeetingCard({ line }: { line: MeetingLine }) {
  const stamps = line.kind === 'batch' ? batchStamps(line) : []
  const oracle = useMeeting((s) => s.oracle)
  const rows = stamps.length > 0 ? oracle.filter((d) => stamps.includes(d.batch_id)) : []
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <div className="flex items-center gap-2 text-xs text-fg-secondary">
        <Mic className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="font-medium">meeting</span>
        <span className="rounded-full bg-layer-2 px-2 leading-5">{line.label}</span>
      </div>
      {line.body && (
        <div className={cn('mt-2 leading-6 whitespace-pre-wrap text-fg-secondary', line.kind === 'batch' && 'font-mono text-[13px] whitespace-normal')}>
          {line.kind === 'batch'
            ? line.body.split('\n').map((l, i) => <BatchRow key={`${i}-${l}`} line={l} />)
            : line.kind === 'started'
              ? `context: ${line.body.replace(/^context:\s*/, '')}`
              : line.body}
        </div>
      )}
      {rows.map((d) => (
        <OracleRow key={d.batch_id} decision={d} />
      ))}
    </div>
  )
}
