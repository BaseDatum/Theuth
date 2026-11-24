import { defaultRangeExtractor, type Range, type VirtualItem, type Virtualizer, useVirtualizer } from '@tanstack/react-virtual'
import { ArrowDown, ChevronDown, ChevronUp } from 'lucide-react'
import { useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { bucket, counters, estimateRow, heights, MeasuredContext, persist, remember } from '@/lib/measured'
import { useNudges } from '@/lib/nudges'
import { useApi } from '@/lib/rpc'
import { useToolRuns } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import { type Prompt, useApprovals, useAsks, useSecretRequests } from '@/store/approvals'
import { useElementForms } from '@/store/element-forms'
import { useScope } from '@/store/engine-scope'
import {
  type Activity,
  freshness,
  shownActivity,
  statusLine,
  useEntries,
  useSessionField,
} from '@/store/sessions'
import { FormPromptCard, Message, PromptCard } from './Message'
import { ActivityIndicator } from './Indicators'
import { window as reach } from './lookahead'
import { type Row, buildRows } from './rows'
import { type Anchor, step } from './scroll-anchor'
import { ToolRunCard } from './ToolRunCard'
import { settleOn, takeJump, useTranscriptJump } from './jump'
import { followContent, followViewport } from './viewport-follow'
import { NextSteps, useNextPick, useNextSteps } from './NextSteps'

const PAGE = 60
/** Distance from the bottom (px) within which new content keeps the view pinned. */
const STICK = 80
/** Distance from the top (px) at which the previous page loads. */
const TOP = 200
/** Ms without a scroll event (and after `touchend`) before the scroller counts as at rest (135 §3.3). */
const REST = 200
/** Upper bound of the slack (px). */
const SLACK_MAX = 40_000
/** Content width (px) assumed before the column is measured: the `md:max-w-[768px]` column minus `px-4`. */
const DEFAULT_WIDTH = 736

type Scroller = Virtualizer<HTMLDivElement, Element>

/** What a row is anchored by across re-keying: its (first) entry id; a prompt or gap row has only its key. */
const rowEntry = (row: Row): string => ('entry' in row ? row.entry.id : 'run' in row ? (row.run[0]?.id ?? row.key) : row.key)
const rowHas = (row: Row | undefined, id: string): boolean =>
  row !== undefined && ('run' in row ? row.run.some((e) => e.id === id) : rowEntry(row) === id)

/**
 * The anchor element: inside `root` (the anchor row's element), the deepest descendant whose box straddles the
 * scroller's top `edge` (depth ≤ 8, zero-height nodes skipped) — what the reader is actually looking at.
 */
function anchorElement(root: Element, edge: number): Element {
  let node = root
  for (let depth = 0; depth < 8; depth++) {
    let next: Element | null = null
    for (const child of node.children) {
      const r = child.getBoundingClientRect()
      if (r.height > 0 && r.top <= edge && r.bottom > edge) {
        next = child
        break
      }
    }
    if (!next) break
    node = next
  }
  return node
}

/** `paddingStart := px` applied now, with the positions recomputed in full (not from the row that just resized). */
function setSlack(v: Scroller, px: number): void {
  // `getMeasurements` is incremental from the lowest resized row: drain a pending partial pass at the old padding,
  // change it, then force the full pass so every row's start carries the new padding. Both reads are memoised.
  v.getTotalSize()
  v.setOptions({ ...v.options, paddingStart: px })
  v.getTotalSize()
}

const WORD: Record<Activity, string> = {
  idle: '',
  thinking: 'thinking…',
  reasoning: 'thinking…',
  streaming: 'streaming…',
  tool: 'running tool…',
  compacting: 'compacting…',
  harvesting: 'harvesting…',
  vision: 'inspecting images…',
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

/** The session's transcript: virtualized, pinned to the bottom until the user scrolls up, paged backwards from the top.
 * `compact` (59 §7.3, the panel under a note): no ruler, no centred column, tighter padding. `next`: the chat view's
 * "Next:" suggestions, the scrolling content's last item (they scroll away with the chat; a pick is sent by the composer). */
export function Transcript({ session, compact = false, next: suggest = false }: { session: string; compact?: boolean; next?: boolean }) {
  const api = useApi()
  const all = useEntries(session)
  const showNudges = useNudges((s) => s.show)
  // Display-only: hidden nudges remain in the session and its context.
  const entries = useMemo(() => (showNudges ? all : all.filter((e) => e.kind !== 'engine')), [all, showNudges])
  const approvals = useApprovals((s) => s.pending)
  const asks = useAsks((s) => s.pending)
  const secrets = useSecretRequests((s) => s.pending)
  const pending = useMemo<Prompt[]>(() => [...approvals, ...asks, ...secrets], [approvals, asks, secrets])
  const more = useSessionField(session, 'more')
  const firstSeq = useSessionField(session, 'firstSeq')
  const running = useSessionField(session, 'running')
  const activity = shownActivity(useSessionField(session, 'activity'), useSessionField(session, 'harvesting'))
  const visionStatus = useSessionField(session, 'visionStatus')
  const statusText = useSessionField(session, 'statusText')
  const children = useSessionField(session, 'children')
  const suggestions = useNextSteps(session, !suggest)
  // This engine's store, read after the await: a page landing after a switch must not touch another engine's slice.
  const { sessions: store } = useScope()
  const ref = useRef<HTMLDivElement>(null)
  const stuck = useRef(true)
  const height = useRef(0)
  const paging = useRef(false)
  const [unstuck, setUnstuck] = useState(false)
  /** Row at the viewport's midpoint: the ruler's active turn while scrolled up. */
  const [at, setAt] = useState(0)
  const [, bump] = useReducer((x: number) => x + 1, 0)

  // 135 §2: the content column's width bucket; heights are remembered and recalled per session at this bucket.
  const column = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(0)
  const widthRef = useRef(0)
  /** Row keys resized since the last remember pass (every size change passes through the predicate). */
  const fresh = useRef(new Set<string>())

  // 135 §3: the slack is the virtualizer's `paddingStart`; a ref so the hook's and the predicate's `setOptions` agree.
  const slack = useRef(0)
  const anchor = useRef<(Anchor & { entry: string }) | null>(null)
  /** The instance of the previous render, for the prepend arithmetic that runs before the hook. */
  const vRef = useRef<Scroller | null>(null)
  const anchorEl = useRef<Element | null>(null)
  const restTimer = useRef(0)
  const touching = useRef(false)
  const touchEnd = useRef(0)
  /** `scrollTop` delta owed to the next commit: the compensated half of a rest write. */
  const pendingWrite = useRef(0)
  const stats = useRef({ absorbed: 0, wrote: 0, renders: 0 })

  const collapse = useToolRuns((s) => s.collapse)
  const pinnedThinking = useToolRuns((s) => s.pinned)
  const formsById = useElementForms((s) => s.pending)
  const forms = useMemo(() => Object.values(formsById), [formsById])
  const rows = useMemo(
    () => buildRows(entries, pending, session, children, collapse, pinnedThinking, forms),
    [entries, pending, session, children, collapse, pinnedThinking, forms],
  )
  // R1.2: the library's measurement memo keys on `getItemKey`'s identity — a new one means a full pass, so it changes
  // only when the rows do (and with the width, so the pass re-estimates at the new bucket). The remembered heights
  // map is hoisted: `remember` mutates it in place; a new bucket makes a new one through `width`.
  const getItemKey = useCallback((i: number) => rows[i]?.key ?? i, [rows, width])
  const known = useMemo(() => (width ? heights(session, width).h : null), [session, width])
  const estimateSize = useCallback(
    (i: number): number => {
      const row = rows[i]
      if (!row) return 96
      return known?.get(row.key) ?? estimateRow(row, (width || DEFAULT_WIDTH) - 32)
    },
    [rows, width, known],
  )
  if (import.meta.env.DEV) stats.current.renders++

  // Rows may move or be re-keyed (a prepend, a gap row, a loose prompt, a run refolded with the page before it):
  // the anchor follows the entry it was on.
  const a = anchor.current
  if (a && !rowHas(rows[a.index], a.entry)) {
    const i = rows.findIndex((r) => rowHas(r, a.entry))
    if (i >= 0) a.index = i
    else anchor.current = null
  }
  // Prepend (§3.3): the new leading rows land in the slack, so the rows the reader sees keep their starts and the
  // same `setOptions` below carries the new count and the reduced `paddingStart`. Tracked by the first entry (row keys
  // change when the page's last tool calls fold into the first row's run); a refolded first row counts as grown by
  // its new estimate over its old measured size.
  const firstEntry = useRef<{ id: string; key: string } | null>(null)
  const head = rows[0] ? { id: rowEntry(rows[0]), key: rows[0].key } : null
  const prev = firstEntry.current
  if (prev && head && head.id !== prev.id) {
    const k = rows.findIndex((r) => rowHas(r, prev.id))
    if (k > 0) {
      let estimated = 0
      for (let i = 0; i < k; i++) estimated += estimateSize(i)
      if (rows[k]!.key !== prev.key) estimated += estimateSize(k) - (vRef.current?.itemSizeCache.get(prev.key) ?? estimateSize(k))
      slack.current = step({ slack: slack.current, target: 0, anchor: null }, { kind: 'prepend', estimated })[0].slack
    }
  }
  firstEntry.current = head

  /** §3.3: every size change comes here before the library decides whether to write `scrollTop`. */
  const predicate = (item: VirtualItem, delta: number, inst: Scroller): boolean => {
    fresh.current.add(String(item.key))
    const a = anchor.current
    let moved = 0
    const el = ref.current
    if (a && item.index === a.index && anchorEl.current?.isConnected && el) {
      moved = anchorEl.current.getBoundingClientRect().top - el.getBoundingClientRect().top - a.offset
    }
    const [s, e] = step({ slack: slack.current, target: 0, anchor: a }, { kind: 'grow', index: item.index, delta, moved })
    if (a && s.anchor && s.anchor.offset !== a.offset) a.offset = s.anchor.offset
    if (e.slack !== slack.current) {
      if (import.meta.env.DEV) stats.current.absorbed += Math.abs(slack.current - e.slack)
      slack.current = e.slack
      setSlack(inst, e.slack)
      // Commit before paint: the library's own notify is an async re-render; a microtask after this ResizeObserver
      // callback (or after React's commit, on the `measureElement` ref path) still runs before the frame paints.
      queueMicrotask(() => flushSync(bump))
    }
    return e.library
  }

  /** R2.1: the mounted window in viewports around the visible range, biased toward the scroll direction (`lookahead.ts`).
   * Stable (the library memoises on its identity); reads the instance through the ref. Re-run on every visible-range
   * change, so the rest window applies from the next range change after a stop (a mount, a jump, a resize). */
  const rangeExtractor = useCallback((r: Range): number[] => {
    const inst = vRef.current
    if (!inst) return defaultRangeExtractor(r)
    const [from, to] = reach(inst.scrollOffset ?? 0, inst.scrollRect?.height ?? 0, inst.scrollDirection, inst.getTotalSize())
    const lo = Math.min(r.startIndex, inst.getVirtualItemForOffset(from)?.index ?? r.startIndex)
    const hi = Math.max(r.endIndex, inst.getVirtualItemForOffset(to)?.index ?? r.endIndex)
    const out: number[] = []
    for (let i = lo; i <= hi; i++) out.push(i)
    return out
  }, [])

  const v = useVirtualizer({
    count: rows.length,
    getScrollElement: () => ref.current,
    estimateSize,
    overscan: 0,
    rangeExtractor,
    getItemKey,
    // Keeps the row at the fold in place when a page is prepended above it.
    anchorTo: 'end',
    paddingStart: slack.current,
  })
  vRef.current = v
  // An instance field, not an option (3.17.11): reassigned every render so it closes over the current rows and anchor.
  v.shouldAdjustScrollPositionOnItemSizeChange = predicate

  // Pinned: whenever the content grows (deltas, measurements) the view lands at the bottom.
  // Scroll-only renders leave it alone so a small upward scroll is not undone.
  // The "Next:" suggestions are content too (the last item, read by this component so their arrival re-renders it).
  useLayoutEffect(() => {
    const el = ref.current
    if (el) height.current = followContent(el, stuck.current, height.current)
  })
  // The viewport itself changed height with the content unchanged — the dash row or the composer below took or gave
  // back room — so the effect above never runs: re-pin when the view was at the bottom, before paint.
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    let last = el.clientHeight
    const ro = new ResizeObserver(() => {
      last = followViewport(el, stuck.current, last)
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  // The compensated half of a rest write, in the same commit as the moved positions. Pinned, the effect above owns
  // the position (the bottom moved by the same amount).
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || pendingWrite.current === 0) return
    if (!stuck.current) el.scrollTop += pendingWrite.current
    pendingWrite.current = 0
  })

  /** §3.3 anchor: the row under the viewport's top edge, once per frame, from virtual coordinates (no layout). R1.4: the
   * element walk (`getBoundingClientRect`) only on a slow frame — the scroller moved < `clientHeight / 4` since the
   * last tracked one — and at rest (`settled`); mid-fling the anchor element is `null` and its row's growth counts as
   * `moved: 0`. */
  const tracking = useRef(0)
  const tracked = useRef(0)
  const track = (settled = false) => {
    tracking.current = 0
    const el = ref.current
    if (!el) return
    const top = el.scrollTop
    const slow = settled || Math.abs(top - tracked.current) < el.clientHeight / 4
    tracked.current = top
    const item = v.getVirtualItemForOffset(top)
    const row = item && rows[item.index]
    if (!item || !row) {
      anchor.current = null
      anchorEl.current = null
      return
    }
    const rowEl = slow ? v.elementsCache.get(item.key) : undefined
    const edge = rowEl ? el.getBoundingClientRect().top : 0
    const node = rowEl ? anchorElement(rowEl, edge) : null
    anchorEl.current = node
    anchor.current = { index: item.index, entry: rowEntry(row), offset: node ? node.getBoundingClientRect().top - edge : 0 }
  }

  /** §3.3 rest: remember what was measured, then rebase the slack toward its target with one compensated write.
   * R1.3: no synchronous `persist` — `remember` arms the ≥ 1 s debounce; unmount persists once. */
  const rest = () => {
    const el = ref.current
    if (!el) return
    if (v.isScrolling || touching.current || Date.now() - touchEnd.current < 100) return armRest()
    track(true)
    if (width) for (const key of fresh.current) {
      const h = v.itemSizeCache.get(key)
      if (h !== undefined) remember(session, width, key, h)
    }
    fresh.current.clear()
    const content = v.getTotalSize() - slack.current
    // §3.1: a page's worth of average rows, at least two viewports, only while there is more to load and it scrolls.
    const target = more && rows.length && content > el.clientHeight ? Math.min(SLACK_MAX, Math.max(2 * el.clientHeight, (PAGE * content) / rows.length)) : 0
    const [s, e] = step({ slack: slack.current, target, anchor: anchor.current }, { kind: 'rest', scrollTop: el.scrollTop })
    if (import.meta.env.DEV) {
      stats.current.wrote += Math.abs(e.write)
      const { absorbed, wrote, renders } = stats.current
      console.debug(`steady-scroll: absorbed ${Math.round(absorbed)} px / wrote ${Math.round(wrote)} · estimates ${counters.estimates} · renders ${renders}`)
    }
    if (e.write === 0 && s.slack === slack.current) return
    slack.current = s.slack
    setSlack(v, s.slack)
    pendingWrite.current += e.write
    flushSync(bump)
  }
  const latest = useRef(rest)
  latest.current = rest
  const armRest = () => {
    clearTimeout(restTimer.current)
    restTimer.current = window.setTimeout(() => latest.current(), REST)
  }

  // Column width (bucketed) and viewport size: a new bucket forgets what was measured at the old one; both re-arm rest.
  // The first bucket arrives after the first measurement pass: `measure()` redoes it with the remembered heights.
  useLayoutEffect(() => {
    const col = column.current
    if (!col) return
    const measure = () => {
      const b = bucket(col.clientWidth)
      if (b !== widthRef.current) {
        if (widthRef.current === 0 && b) v.measure()
        widthRef.current = b
        fresh.current.clear()
        setWidth(b)
      }
      armRest()
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(col)
    return () => ro.disconnect()
  }, [])
  // Touch state for the rest check; `armRest` after a touch ends so a fling that stops without a last scroll event still rebases.
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const start = () => {
      touching.current = true
    }
    const end = () => {
      touching.current = false
      touchEnd.current = Date.now()
      armRest()
    }
    el.addEventListener('touchstart', start, { passive: true })
    el.addEventListener('touchend', end, { passive: true })
    el.addEventListener('touchcancel', end, { passive: true })
    return () => {
      el.removeEventListener('touchstart', start)
      el.removeEventListener('touchend', end)
      el.removeEventListener('touchcancel', end)
    }
  }, [])
  // `more` flipping and a prepend landing both change the target; persist on unmount.
  useEffect(armRest, [more, head?.id])
  useEffect(
    () => () => {
      clearTimeout(restTimer.current)
      persist(session)
    },
    [session],
  )

  // Guarded by the slice's freshness at request time (48 §5): a page read before a context edit
  // (or before a snapshot replaced the transcript) is dropped rather than reintroducing removed rows.
  const page = async () => {
    if (paging.current || !more || firstSeq === null || firstSeq === undefined) return
    const s = store.getState().sessions[session]
    if (!s) return
    paging.current = true
    try {
      const older = await api.load_session(session, firstSeq, PAGE)
      store.getState().loadHistory(session, older, 'prepend', freshness(s))
    } finally {
      paging.current = false
    }
  }

  // A first page shorter than the viewport never scrolls: fetch until it does or history ends.
  useEffect(() => {
    const el = ref.current
    if (el && more && el.scrollHeight - slack.current <= el.clientHeight) void page()
  })

  const onScroll = () => {
    const el = ref.current
    if (!el) return
    const gap = el.scrollHeight - el.scrollTop - el.clientHeight
    stuck.current = gap <= STICK
    setUnstuck(!stuck.current)
    setAt(v.getVirtualItemForOffset(el.scrollTop + el.clientHeight / 2)?.index ?? 0)
    if (!tracking.current) tracking.current = requestAnimationFrame(() => track())
    armRest()
    if (el.scrollTop < slack.current + TOP) void page()
  }

  const ticks = useMemo(() => ticksOf(rows), [rows])
  const pinned = !unstuck
  const jump = (index: number) => {
    stuck.current = false
    setUnstuck(true)
    v.scrollToIndex(index, { align: 'start' })
  }
  // "jump to message" (SendCard → `useTranscriptJump`): this transcript's virtualiser brings the row into range, then
  // the card is centred and flashed once mounted. Nothing depends on the send card staying mounted; leaving the
  // session drops a request still pending for it.
  // The nonce alone: `takeJump` clearing the request must not re-run this effect (its cleanup would cancel the fine-tune).
  const jumpNonce = useTranscriptJump((s) => s.nonce)
  useEffect(() => {
    const target = takeJump(session, rows)
    if (!target) return
    stuck.current = false
    setUnstuck(true)
    v.scrollToIndex(target.index, { align: 'center' })
    const e = target.entry
    const id = e.kind === 'peer' && e.origin ? `peer-msg-${e.origin.id}` : null
    return settleOn(ref.current, () => (id ? document.getElementById(id) : null) ?? ref.current?.querySelector(`[data-index="${target.index}"]`) ?? null)
  }, [jumpNonce]) // eslint-disable-line react-hooks/exhaustive-deps -- one run per request; `rows`/`v` as of it
  useEffect(
    () => () => {
      const j = useTranscriptJump.getState()
      if (j.session === session) j.clear()
    },
    [session],
  )

  const items = v.getVirtualItems()
  // R2.2: the rows the library reports visible (± 1) render now; the rest of the window one priority lower through
  // `useDeferredValue`, so a heavy mount two viewports ahead never blocks a scroll frame. The deferred value is the
  // list of keys (identity kept while the content is unchanged); each is looked up by key at render time, so a
  // deferred row never carries a stale index or position. A key that reached the visible set renders urgently instead.
  const lo = (v.range?.startIndex ?? 0) - 1
  const hi = (v.range?.endIndex ?? -1) + 1
  const aheadRef = useRef<string[]>([])
  const next = items.filter((it) => it.index < lo || it.index > hi).map((it) => String(it.key))
  const ahead = next.length === aheadRef.current.length && next.every((k, i) => k === aheadRef.current[i]) ? aheadRef.current : (aheadRef.current = next)
  const deferred = useDeferredValue(ahead)
  const indexOf = useMemo(() => new Map(rows.map((r, i) => [r.key, i])), [rows])
  const cell = (item: VirtualItem, row: Row) => (
    <div key={item.key} data-index={item.index} ref={v.measureElement} className="absolute top-0 left-0 w-full" style={{ transform: `translateY(${item.start}px)` }}>
      {'entry' in row ? (
        <Message entry={row.entry} prompts={row.prompts} session={session} />
      ) : 'run' in row ? (
        <ToolRunCard run={row.run} prompts={row.prompts} session={session} />
      ) : 'gap' in row ? (
        <div className="flex items-center gap-3 py-2 text-xs text-fg-tertiary">
          <span className="h-px flex-1 bg-border-subtle" />
          <span className="tabular-nums">{row.gap}</span>
          <span className="h-px flex-1 bg-border-subtle" />
        </div>
      ) : 'form' in row ? (
        <FormPromptCard form={row.form} from={row.from} />
      ) : (
        <PromptCard prompt={row.prompt} from={row.from} />
      )}
    </div>
  )
  const measured = useMemo(() => (width ? { session, width } : null), [session, width])
  return (
    <div className="relative min-h-0 flex-1">
      {/* Never a page-wide horizontal scroll: long words wrap (inherited; `pre` and `nowrap` cells keep scrolling inside their own boxes). */}
      <div ref={ref} onScroll={onScroll} className="h-full overflow-x-hidden overflow-y-auto wrap-anywhere">
        <div
          ref={column}
          className={cn('relative w-full', compact ? 'px-2' : 'mx-auto md:max-w-[768px] px-4')}
          style={{ height: v.getTotalSize() }}
        >
          <MeasuredContext value={measured}>
            <div className="relative h-full w-full">
              {items.map((item) => {
                const row = item.index >= lo && item.index <= hi ? rows[item.index] : undefined
                return row ? cell(item, row) : null
              })}
              {deferred.map((key) => {
                const i = indexOf.get(key)
                if (i === undefined || (i >= lo && i <= hi)) return null
                const item = v.measurementsCache[i]
                const row = rows[i]
                return item && row ? cell(item, row) : null
              })}
            </div>
          </MeasuredContext>
        </div>
        {suggestions && (
          <div data-next-slot="" className={cn('w-full pt-1 pb-3', compact ? 'px-2' : 'mx-auto md:max-w-[768px] px-4')}>
            <NextSteps items={suggestions} onPick={(text) => useNextPick.getState().pick(session, text)} />
          </div>
        )}
        {(running || activity !== 'idle') && (
          <div className={cn('flex w-full items-center gap-2 py-3 text-sm text-fg-secondary', compact ? 'px-4' : 'mx-auto md:max-w-[768px] px-8')}>
            <ActivityIndicator activity={activity} />
            {statusLine(activity, statusText, visionStatus) || WORD[activity]}
          </div>
        )}
      </div>
      {!compact && <Ruler ticks={ticks} at={pinned ? Number.POSITIVE_INFINITY : at} onJump={jump} />}
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
