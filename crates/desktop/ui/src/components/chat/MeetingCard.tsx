// 53 §4.4: the `[meeting …]` lines in a meeting's session get their own card — a left-aligned notice with a
// chip naming what it is (transcript batch, request, tap, dismissal, deeper, start) instead of a user bubble,
// as harness nudges do. The text stays literal; nothing is hidden from the record.

import { Mic } from 'lucide-react'
import type { MeetingLine } from '@/lib/meeting-line'
import { cn } from '@/lib/utils'

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

export function MeetingCard({ line }: { line: MeetingLine }) {
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
    </div>
  )
}
