// 03 §2: the live transcript dropdown between the brief pane and the request box. Collapsed (the default) it is one
// row — `▸ Transcript` plus the newest line as a muted preview; open it is the last `LINES_KEEP` rows from
// `store/meeting.ts`, painted like the control center's live log (`mm:ss  me:/them: text`, partials muted,
// requests tinted), scrolling inside a fixed height and pinned to the bottom until the user scrolls up.

import { ChevronRight } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { type Line, mmss, useMeeting } from '@/store/meeting'

/** Open/closed survives a reload of this webview only. */
const KEY = 'island.transcript.open'
/** Distance from the bottom (px) within which new lines keep the view pinned. */
const STICK = 8
/** Preview length on the collapsed row. */
const PREVIEW = 60

const who = (l: Line): string => (l.track === 'Mic' ? 'me:' : 'them:')

/** The collapsed row's preview: `me: first sixty chars…`. */
export function preview(l: Line, max = PREVIEW): string {
  const text = l.text.trim()
  return `${who(l)} ${text.length > max ? `${text.slice(0, max).trimEnd()}…` : text}`
}

export function IslandTranscript({ measure }: { measure: (el: HTMLElement | null) => void }) {
  const lines = useMeeting((s) => s.lines)
  const [open, setOpen] = useState(() => localStorage.getItem(KEY) === '1')
  const list = useRef<HTMLDivElement>(null)
  const stuck = useRef(true)
  // Newest at the bottom: follow it while the user has not scrolled up; opening always lands on the newest line.
  useEffect(() => {
    if (!open) stuck.current = true
    const el = list.current
    if (el && stuck.current) el.scrollTop = el.scrollHeight
  }, [lines, open])
  const last = lines.at(-1)
  return (
    <div ref={measure} className="border-t border-border-subtle">
      <button
        type="button"
        className="flex h-6 w-full items-center gap-1 px-2 text-[11px] leading-4 text-fg-secondary hover:bg-accent"
        aria-expanded={open}
        onClick={() =>
          setOpen((o) => {
            localStorage.setItem(KEY, o ? '0' : '1')
            return !o
          })
        }
      >
        <ChevronRight className={cn('size-3 shrink-0 transition-transform', open && 'rotate-90')} />
        <span className="shrink-0">Transcript</span>
        {!open && last && <span className="min-w-0 truncate text-fg-tertiary">{preview(last)}</span>}
      </button>
      {open && (
        <div
          ref={list}
          className="h-32 overflow-y-auto px-2 pb-1 font-mono text-[11px] leading-4"
          onScroll={(e) => {
            const el = e.currentTarget
            stuck.current = el.scrollHeight - el.scrollTop - el.clientHeight <= STICK
          }}
        >
          {lines.length === 0 && <span className="text-fg-tertiary">Waiting for speech…</span>}
          {lines.map((l, i) => (
            <div key={`${l.track}-${l.t0_ms}-${i}`} className={cn('flex gap-1.5', l.open && 'text-fg-tertiary', l.request && 'bg-fg-accent/10')}>
              <span className="shrink-0 text-fg-tertiary">{mmss(l.t0_ms)}</span>
              <span className={cn('shrink-0', l.track === 'Mic' ? 'text-fg-accent' : 'text-fg-secondary')}>{who(l)}</span>
              <span className="min-w-0 break-words">{l.text}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
