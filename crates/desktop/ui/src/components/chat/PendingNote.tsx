// 45 §7.2: a voice note on its way (`lib/voice-pending`) — where its `you` row will land, the length under a mic, then
// "Uploading…" or "Transcribing… 0:12"; a failure keeps the recording behind Retry / Discard.

import { CircleAlert, Loader2, Mic } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { type PendingNote, clock, usePendingNotes } from '@/lib/voice-pending'

/** The phase line: what the bubble says now, `now` the clock it is read at. */
export function phaseLabel(n: PendingNote, now: number): string {
  if (n.phase === 'uploading') return 'Uploading…'
  if (n.phase === 'transcribing') return `Transcribing… ${clock(now - n.since)}`
  return n.error ? `Not sent — ${n.error}` : 'Not sent'
}

export function PendingNoteBubble({ noteKey, note, className }: { noteKey: string; note: PendingNote; className?: string }) {
  const [now, setNow] = useState(() => Date.now())
  const failed = note.phase === 'failed'
  useEffect(() => {
    if (failed) return
    setNow(Date.now())
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [failed, note.since])
  const notes = usePendingNotes.getState()
  return (
    <div className={cn('flex flex-col items-end gap-2 py-2', className)} data-pending-note={note.phase}>
      {/* The landed row's caption (`MessageUser`: `[voice note, N s]`'s N), so nothing moves when it takes over. */}
      <span className="flex items-center gap-1 text-xs text-fg-secondary">
        <Mic className="size-3" /> {Math.max(1, Math.ceil(note.ms / 1000))} s
      </span>
      <div
        role={failed ? 'alert' : 'status'}
        className={cn(
          'flex max-w-[85%] items-center gap-2 rounded-[22px] px-4 py-2.5 text-sm md:max-w-[70%]',
          failed ? 'border border-warning-border/30 bg-warning-bg/40' : 'bg-layer-3 text-fg-secondary',
        )}
      >
        {failed ? (
          <CircleAlert className="size-4 shrink-0 text-warning" aria-hidden="true" />
        ) : (
          <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden="true" />
        )}
        <span className="min-w-0 flex-1 tabular-nums">{phaseLabel(note, now)}</span>
        {failed && (
          <>
            <Button size="sm" variant="outline" onClick={() => notes.discard(noteKey)}>
              Discard
            </Button>
            <Button size="sm" onClick={() => notes.retry(noteKey)}>
              Retry
            </Button>
          </>
        )}
      </div>
    </div>
  )
}
