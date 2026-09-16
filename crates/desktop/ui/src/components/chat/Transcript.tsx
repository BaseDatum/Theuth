import { useVirtualizer } from '@tanstack/react-virtual'
import { ArrowDown, ChevronDown, ChevronUp } from 'lucide-react'
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { type Prompt, useApprovals, useAsks, useSecretRequests } from '@/store/approvals'
import {
  type Activity,
  type Entry,
  useEntries,
  useSessionField,
  useSessions,
} from '@/store/sessions'
import { Message, PromptCard } from './Message'
import { Pulse } from './Pulse'

const PAGE = 60
/** Distance from the bottom (px) within which new content keeps the view pinned. */
const STICK = 80
/** Distance from the top (px) at which the previous page loads. */
const TOP = 200

const WORD: Record<Activity, string> = {
  idle: '',
  thinking: 'thinking…',
  reasoning: 'thinking…',
  streaming: 'streaming…',
  tool: 'running tool…',
  compacting: 'compacting…',
  harvesting: 'harvesting…',
}

type Row = { key: string; entry: Entry; prompts: Prompt[] | null } | { key: string; prompt: Prompt }

/** Entries, each with the prompts waiting on its (unfinished, same-named) tool call; prompts with no such row come last. */
function buildRows(entries: Entry[], pending: Prompt[], session: string): Row[] {
  const rows: Row[] = entries.map((entry) => ({
    key: entry.id,
    entry,
    prompts: null,
  }))
  const loose: Row[] = []
  for (const prompt of pending) {
    if (prompt.session !== session) continue
    const tool =
      prompt.type === 'ApprovalRequested' ? prompt.tool : prompt.type === 'AskRequested' ? 'ask_user' : 'secret_request'
    let at = entries.length - 1
    while (at >= 0) {
      const e = entries[at]
      if (e?.kind === 'tool' && e.name === tool && !e.result) break
      at--
    }
    const row = rows[at]
    if (row && 'entry' in row) row.prompts = [...(row.prompts ?? []), prompt]
    else loose.push({ key: prompt.id, prompt })
  }
  return rows.concat(loose)
}

type Tick = { index: number; ask: string; reply: string }

/** One tick per turn opener: its row index, the question and the first reply text. */
function ticksOf(rows: Row[]): Tick[] {
  const out: Tick[] = []
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]
    if (!row || !('entry' in row)) continue
    const e = row.entry
    const last = out[out.length - 1]
    if (e.kind === 'user' && e.turn !== undefined) out.push({ index: i, ask: e.text, reply: '' })
    else if (e.kind === 'assistant' && last && !last.reply) last.reply = e.text
  }
  return out
}

/**
 * Grok-style ruler on the right: one muted line per turn, the turn at `at`
 * highlighted; hovering shows prev/next chevrons and a preview per line.
 * Pointer-only: hidden below `md`.
 */
function Ruler({ ticks, at, onJump }: { ticks: Tick[]; at: number; onJump(index: number): void }) {
  if (ticks.length < 2) return null
  const cur = ticks.findLastIndex((t) => t.index <= at)
  const prev = ticks[cur - 1]
  const next = ticks[cur + 1]
  const chevron = 'size-5 rounded text-fg-tertiary opacity-0 transition-opacity group-hover:opacity-100 hover:text-foreground disabled:invisible'
  return (
    <div className="group absolute inset-y-3 right-4 hidden w-6 flex-col items-center justify-center md:flex">
      <button type="button" className={chevron} aria-label="Previous turn" disabled={!prev} onClick={() => prev && onJump(prev.index)}>
        <ChevronUp className="size-5" />
      </button>
      <div className="flex min-h-0 w-full shrink flex-col items-end py-1">
        {ticks.map((t, i) => (
          <Tooltip key={t.index}>
            <TooltipTrigger asChild>
              <button
                type="button"
                className="flex h-2.5 min-h-0 w-full shrink items-center justify-end"
                aria-label={`Turn ${i + 1}`}
                onClick={() => onJump(t.index)}
              >
                <span
                  className={cn(
                    'h-px rounded-full transition-all',
                    i === cur ? 'w-4 bg-foreground' : 'w-2.5 bg-fg-tertiary/50 group-hover:w-3 hover:bg-foreground',
                  )}
                />
              </button>
            </TooltipTrigger>
            <TooltipContent side="left" sideOffset={4} className="max-w-xs text-left text-wrap">
              <div className="line-clamp-1 font-medium">{t.ask}</div>
              {t.reply && <div className="line-clamp-3 opacity-80">{t.reply}</div>}
            </TooltipContent>
          </Tooltip>
        ))}
      </div>
      <button type="button" className={chevron} aria-label="Next turn" disabled={!next} onClick={() => next && onJump(next.index)}>
        <ChevronDown className="size-5" />
      </button>
    </div>
  )
}

/** The session's transcript: virtualized, pinned to the bottom until the user scrolls up, paged backwards from the top. */
export function Transcript({ session }: { session: string }) {
  const api = useApi()
  const entries = useEntries(session)
  const approvals = useApprovals((s) => s.pending)
  const asks = useAsks((s) => s.pending)
  const secrets = useSecretRequests((s) => s.pending)
  const pending = useMemo<Prompt[]>(() => [...approvals, ...asks, ...secrets], [approvals, asks, secrets])
  const more = useSessionField(session, 'more')
  const firstSeq = useSessionField(session, 'firstSeq')
  const running = useSessionField(session, 'running')
  const activity = useSessionField(session, 'activity') ?? 'idle'
  const loadHistory = useSessions((s) => s.loadHistory)
  const ref = useRef<HTMLDivElement>(null)
  const stuck = useRef(true)
  const height = useRef(0)
  const paging = useRef(false)
  const [unstuck, setUnstuck] = useState(false)
  /** Row at the viewport's midpoint: the ruler's active turn while scrolled up. */
  const [at, setAt] = useState(0)

  const rows = useMemo(() => buildRows(entries, pending, session), [entries, pending, session])
  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => ref.current,
    estimateSize: () => 96,
    overscan: 4,
    getItemKey: (i) => rows[i]?.key ?? i,
    // Keeps the row at the fold in place when a page is prepended above it.
    anchorTo: 'end',
  })

  // Pinned: whenever the content grows (deltas, measurements) the view lands at the bottom.
  // Scroll-only renders leave it alone so a small upward scroll is not undone.
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || el.scrollHeight === height.current) return
    height.current = el.scrollHeight
    if (stuck.current) el.scrollTop = el.scrollHeight
  })

  const page = async () => {
    if (paging.current || !more || firstSeq === null || firstSeq === undefined) return
    paging.current = true
    try {
      loadHistory(session, await api.load_session(session, firstSeq, PAGE), 'prepend')
    } finally {
      paging.current = false
    }
  }

  // A first page shorter than the viewport never scrolls: fetch until it does or history ends.
  useEffect(() => {
    const el = ref.current
    if (el && more && el.scrollHeight <= el.clientHeight) void page()
  })

  const onScroll = () => {
    const el = ref.current
    if (!el) return
    const gap = el.scrollHeight - el.scrollTop - el.clientHeight
    stuck.current = gap <= STICK
    setUnstuck(!stuck.current)
    setAt(v.getVirtualItemForOffset(el.scrollTop + el.clientHeight / 2)?.index ?? 0)
    if (el.scrollTop < TOP) void page()
  }

  const ticks = useMemo(() => ticksOf(rows), [rows])
  const pinned = !unstuck
  const jump = (index: number) => {
    stuck.current = false
    setUnstuck(true)
    v.scrollToIndex(index, { align: 'start' })
  }

  const items = v.getVirtualItems()
  return (
    <div className="relative min-h-0 flex-1">
      <div ref={ref} onScroll={onScroll} className="h-full overflow-y-auto">
        <div
          className="relative mx-auto w-full md:max-w-[768px] px-4"
          style={{ height: v.getTotalSize() }}
        >
          <div className="relative h-full w-full">
            {items.map((item) => {
              const row = rows[item.index]
              if (!row) return null
              return (
                <div
                  key={item.key}
                  data-index={item.index}
                  ref={v.measureElement}
                  className="absolute top-0 left-0 w-full"
                  style={{ transform: `translateY(${item.start}px)` }}
                >
                  {'entry' in row ? (
                    <Message entry={row.entry} prompts={row.prompts} session={session} />
                  ) : (
                    <PromptCard prompt={row.prompt} />
                  )}
                </div>
              )
            })}
          </div>
        </div>
        {(running || activity !== 'idle') && (
          <div className="mx-auto flex w-full md:max-w-[768px] items-center gap-2 px-8 py-3 text-sm text-fg-secondary">
            <Pulse />
            {WORD[activity]}
          </div>
        )}
      </div>
      <Ruler ticks={ticks} at={pinned ? Number.POSITIVE_INFINITY : at} onJump={jump} />
      {unstuck && (
        <Button
          variant="outline"
          className="absolute bottom-3 left-1/2 size-9 -translate-x-1/2 rounded-full shadow-sm"
          aria-label="Scroll to bottom"
          onClick={() => {
            stuck.current = true
            setUnstuck(false)
            if (ref.current) ref.current.scrollTop = ref.current.scrollHeight
          }}
        >
          <ArrowDown />
        </Button>
      )}
    </div>
  )
}
