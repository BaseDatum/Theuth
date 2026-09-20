// 53 decision 8, 11 / 03 §2: the island — a 440 × 44 strip at the notch (meters, recording clock, topic
// buttons, badges, the eye, ⋮ pause/stop) that grows to fit the brief pane when one is showing. Runs in its own
// non-activating `NSPanel` webview; the shell shows/hides/places it through `lib/tauri.ts`. State is
// `store/meeting.ts`; `Level` bypasses React (CSS variables on a ref).

import { useQueryClient } from '@tanstack/react-query'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ArrowUp, ChevronDown, ChevronUp, Eye, EyeOff, LoaderCircle, MoreVertical, Pause, Play, Square, X, ZoomIn } from 'lucide-react'
import { type ComponentProps, useCallback, useEffect, useRef, useState } from 'react'
import { useMeetingSettings } from '@/lib/meeting-settings'
import { queryKeys, useApi } from '@/lib/rpc'
import {
  isTauri,
  islandHide,
  islandPlace,
  islandResignKey,
  islandShow,
  onScreensChanged,
  setHiddenFromCapture,
} from '@/lib/tauri'
import { cn } from '@/lib/utils'
import type { Degradation } from '@/rpc/bindings/Degradation'
import type { MeetingState } from '@/rpc/bindings/MeetingState'
import { current, elapsed, live, useMeeting } from '@/store/meeting'
import { on } from '@/store/sessions'


const MOVE_SAVE_MS = 300
/** Strip height and default width, logical px (03 §2). */
const STRIP = 44
const WIDTH = 440

/** 03 §2 badges; `PermissionDenied` is a `Failed` start, never shown on a live strip. */
function badge(d: Degradation): string | null {
  if (d === 'MicNoAec') return 'headphones recommended'
  if (d === 'ModelDown') return 'model down'
  if (d === 'DiskLow') return 'disk low'
  if (d === 'NearMaxLength') return '10 min left'
  if (typeof d === 'object' && 'SttDown' in d) return 'transcription down'
  return null
}

const mmss = (ms: number): string => {
  const s = Math.floor(ms / 1000)
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
}

export function Island() {
  const api = useApi()
  const qc = useQueryClient()
  const state = useMeeting((s) => s.state)
  const topics = useMeeting((s) => s.topics)
  const tapped = useMeeting((s) => s.tapped)
  const degraded = useMeeting((s) => s.degraded)
  const listening = useMeeting((s) => s.listening)
  const approval = useMeeting((s) => s.approvalWaiting)
  const activity = useMeeting((s) => s.activity)
  const briefs = useMeeting((s) => s.briefs)
  const cursor = useMeeting((s) => s.cursor)
  const unseen = useMeeting((s) => s.unseen)
  const hidden = useMeetingSettings((s) => s.hidden)
  const [menu, setMenu] = useState(false)
  const brief = current({ briefs, cursor })
  const shown = live(state) || state === 'Starting' || state === 'Stopping'
  const placed = useRef(false)
  const key = useRef<string | null>(null)
  // Content-fit size: the strip, the brief pane at its wrapped height, the request box. Once the user has dragged
  // an edge for this screen configuration, their size wins and the pane scrolls inside it.
  const [natural, setNatural] = useState(STRIP)
  const parts = useRef(new Set<HTMLElement>())
  const ro = useRef<ResizeObserver | null>(null)
  /** Ref callback for every block whose natural height adds up to the window's: the strip, the pane's content, the box. */
  const measure = useCallback((el: HTMLElement | null) => {
    ro.current ??= new ResizeObserver(() => {
      let sum = 2 // the two dividers
      for (const p of parts.current) sum += p.offsetHeight
      setNatural(Math.max(STRIP, Math.ceil(sum)))
    })
    if (el) {
      parts.current.add(el)
      ro.current.observe(el)
    }
  }, [])
  const sizes = useMeetingSettings((s) => s.sizes)
  const saved = key.current ? sizes[key.current] : undefined
  const size: [number, number] = saved ?? [WIDTH, natural]
  /** The size we last asked for; a `Resized` that differs is the user's drag. */
  const asked = useRef(size)

  // The window is transparent; only the strip paints.
  useEffect(() => {
    document.documentElement.dataset.island = ''
    return () => {
      delete document.documentElement.dataset.island
    }
  }, [])

  // Show on `Started` (placed first, once), resize with the pane, hide on Done/Failed (03 §2).
  const [w, h] = size
  useEffect(() => {
    if (!shown) {
      placed.current = false
      void islandHide()
      return
    }
    const show = () => {
      asked.current = [w, h]
      void islandShow(w, h)
    }
    if (placed.current) show()
    else {
      placed.current = true
      void islandPlace(useMeetingSettings.getState().positions)
        .then(([, , k]) => (key.current = k))
        .finally(show)
    }
  }, [shown, w, h])

  // Screen configuration changed → re-place (saved for that configuration, else the notch default).
  // A drag end saves the position, an edge drag the size, under the current configuration key.
  useEffect(() => {
    const offScreens = onScreensChanged(() => {
      void islandPlace(useMeetingSettings.getState().positions).then(([, , k]) => (key.current = k))
    })
    if (!isTauri()) return () => void offScreens.then((f) => f())
    const win = getCurrentWindow()
    let moveTimer = 0
    let sizeTimer = 0
    const offMoved = win.onMoved(() => {
      clearTimeout(moveTimer)
      moveTimer = window.setTimeout(async () => {
        const k = key.current
        if (!k) return
        const p = (await win.outerPosition()).toLogical(await win.scaleFactor())
        useMeetingSettings.getState().setPosition(k, [p.x, p.y])
      }, MOVE_SAVE_MS)
    })
    const offResized = win.onResized(() => {
      clearTimeout(sizeTimer)
      sizeTimer = window.setTimeout(async () => {
        const k = key.current
        if (!k) return
        const s = (await win.innerSize()).toLogical(await win.scaleFactor())
        const [aw, ah] = asked.current
        if (Math.abs(s.width - aw) < 2 && Math.abs(s.height - ah) < 2) return
        useMeetingSettings.getState().setSize(k, [Math.round(s.width), Math.round(s.height)])
      }, MOVE_SAVE_MS)
    })
    return () => {
      clearTimeout(moveTimer)
      clearTimeout(sizeTimer)
      void offScreens.then((f) => f())
      void offMoved.then((f) => f())
      void offResized.then((f) => f())
    }
  }, [])

  // Every cursor move → the daemon, so the control center mirrors it (03 §0.4).
  const briefId = brief?.id ?? null
  useEffect(() => {
    if (shown) void api.meeting_cursor(briefId).catch(() => undefined)
  }, [api, shown, briefId])

  const badges = [
    ...degraded.map(badge).filter((b): b is string => b !== null),
    ...(approval ? ['approval waiting'] : []),
    ...(listening ? ['listening…'] : []),
  ]
  const action = (p: Promise<unknown>) => {
    setMenu(false)
    void p.then(() => qc.invalidateQueries({ queryKey: queryKeys.meetingStatus })).catch(() => undefined)
  }

  return (
    <div className="flex h-dvh flex-col overflow-hidden rounded-2xl border border-border-subtle bg-background/95 text-foreground shadow-lg backdrop-blur">
      <div ref={measure} data-tauri-drag-region className="flex h-11 shrink-0 items-center gap-1.5 px-2">
        <Meters />
        <Clock state={state} />
        <div className="flex min-w-0 flex-1 items-center gap-1 overflow-hidden">
          {menu ? (
            <>
              <StripButton onClick={() => action(state === 'Paused' ? api.meeting_resume() : api.meeting_pause())}>
                {state === 'Paused' ? <Play className="size-3" /> : <Pause className="size-3" />}
                {state === 'Paused' ? 'resume' : 'pause'}
              </StripButton>
              <StripButton onClick={() => action(api.meeting_stop())}>
                <Square className="size-3 fill-current" /> stop
              </StripButton>
              <StripButton onClick={() => setMenu(false)} aria-label="Close menu">
                <X className="size-3" />
              </StripButton>
            </>
          ) : (
            <>
              {topics.map((t) => (
                <StripButton
                  key={t.id}
                  className={cn('group', tapped.includes(t.id) && 'opacity-50')}
                  title={t.label}
                  onClick={() => {
                    useMeeting.getState().markTapped(t.id)
                    void api.meeting_tap(t.id).catch(() => undefined)
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault()
                    void api.meeting_dismiss(t.id).catch(() => undefined)
                  }}
                >
                  <span className="truncate">{t.label}</span>
                  {t.cached !== null && (
                    <span className="size-1.5 shrink-0 rounded-full bg-fg-accent" aria-label="cached brief" title="cached — the brief is ready" />
                  )}
                  <X
                    className="hidden size-3 shrink-0 text-fg-tertiary group-hover:block"
                    aria-label="Dismiss"
                    onClick={(e) => {
                      e.stopPropagation()
                      void api.meeting_dismiss(t.id).catch(() => undefined)
                    }}
                  />
                </StripButton>
              ))}
              {badges.map((b) => (
                <span key={b} className="shrink-0 text-[10px] leading-4 text-fg-tertiary">
                  {b}
                </span>
              ))}
              {activity && (
                <span className="flex shrink-0 items-center gap-1 text-[10px] leading-4 text-fg-tertiary" title="The meeting session is working">
                  <LoaderCircle className="size-3 animate-spin" /> {activity}
                </span>
              )}
            </>
          )}
        </div>
        <button
          type="button"
          className="flex size-7 shrink-0 items-center justify-center rounded-full text-fg-secondary hover:bg-accent"
          aria-label={hidden ? 'Hidden from capture' : 'Visible in capture'}
          title={hidden ? 'Hidden from screen capture' : 'Visible in screen capture'}
          onClick={() => void setHiddenFromCapture(!hidden)}
        >
          {hidden ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
        </button>
        <button
          type="button"
          className="flex size-7 shrink-0 items-center justify-center rounded-full text-fg-secondary hover:bg-accent"
          aria-label="Pause or stop"
          onClick={() => setMenu((m) => !m)}
        >
          <MoreVertical className="size-4" />
        </button>
      </div>
      {brief && (
        <div className="min-h-0 flex-1 overflow-y-auto border-t border-border-subtle">
          <div ref={measure} className="flex gap-2 px-3 pt-2 pb-2">
            <div className="min-w-0 flex-1 text-[12px] leading-4">
              <div className="font-semibold break-words">{brief.title}</div>
              <ul className="mt-1 list-disc space-y-0.5 pl-4 text-fg-secondary">
                {brief.bullets.slice(0, 3).map((b) => (
                  <li key={b} className="break-words">
                    {b}
                  </li>
                ))}
              </ul>
              <StripButton
                className="mt-1.5 h-6 text-[11px]"
                title="Ask for a longer, more specific brief on this"
                onClick={() => void api.meeting_deeper(brief.id).catch(() => undefined)}
              >
                <ZoomIn className="size-3" /> deeper
              </StripButton>
            </div>
            <div className="flex shrink-0 flex-col items-center gap-0.5">
              <button
                type="button"
                className="rounded p-0.5 text-fg-secondary hover:bg-accent disabled:opacity-30"
                aria-label="Newer brief"
                disabled={cursor >= briefs.length - 1}
                onClick={() => useMeeting.getState().move(1)}
              >
                <ChevronUp className="size-4" />
              </button>
              {unseen > 0 && (
                <span className="rounded-full bg-fg-accent px-1.5 text-[10px] leading-4 text-white">↑ {unseen} new</span>
              )}
              <button
                type="button"
                className="rounded p-0.5 text-fg-secondary hover:bg-accent disabled:opacity-30"
                aria-label="Older brief"
                disabled={cursor <= 0}
                onClick={() => useMeeting.getState().move(-1)}
              >
                <ChevronDown className="size-4" />
              </button>
            </div>
          </div>
        </div>
      )}
      <RequestBox measure={measure} />
    </div>
  )
}

/**
 * A typed request to the meeting session (`[meeting request, typed] …` → a brief; a bare topic name is a
 * request for a brief on it). Typing makes the panel key; leaving the box hands key back to the meeting app.
 */
function RequestBox({ measure }: { measure: (el: HTMLElement | null) => void }) {
  const api = useApi()
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const send = async (el: HTMLInputElement) => {
    const t = text.trim()
    if (!t || busy) return
    setBusy(true)
    try {
      await api.meeting_request(t)
      setText('')
      el.blur()
    } catch {
      // The session refused (stopped mid-typing); keep the text.
    } finally {
      setBusy(false)
    }
  }
  return (
    <div ref={measure} className="flex items-center gap-1 border-t border-border-subtle px-2 py-1">
      <input
        className="h-6 min-w-0 flex-1 bg-transparent px-1.5 text-[12px] outline-none placeholder:text-fg-tertiary"
        placeholder="Ask the meeting agent… a topic name = brief it"
        value={text}
        disabled={busy}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => void islandResignKey()}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void send(e.currentTarget)
          else if (e.key === 'Escape') e.currentTarget.blur()
        }}
      />
      <button
        type="button"
        className="flex size-6 shrink-0 items-center justify-center rounded-full text-fg-secondary hover:bg-accent disabled:opacity-30"
        aria-label="Send request"
        disabled={!text.trim() || busy}
        onMouseDown={(e) => e.preventDefault()}
        onClick={(e) => void send(e.currentTarget.previousElementSibling as HTMLInputElement)}
      >
        <ArrowUp className="size-3.5" />
      </button>
    </div>
  )
}

function StripButton({ className, ...props }: ComponentProps<'button'>) {
  return (
    <button
      type="button"
      className={cn(
        'flex h-7 max-w-32 shrink-0 items-center gap-1 rounded-full bg-layer-2 px-2.5 text-[12px] leading-4 hover:bg-accent',
        className,
      )}
      {...props}
    />
  )
}

/** Two 6 px bars driven by `Level` at 10 Hz through CSS variables — no React state. */
function Meters() {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(
    () =>
      on('Level', (e) => {
        const el = ref.current
        if (!el) return
        el.style.setProperty('--mic', String(Math.min(1, e.mic_rms * 10)))
        el.style.setProperty('--sys', String(Math.min(1, e.system_rms * 10)))
      }),
    [],
  )
  return (
    <div ref={ref} className="flex h-4 shrink-0 items-end gap-0.5" aria-hidden>
      <span className="w-1.5 rounded-sm bg-fg-accent" style={{ height: 'calc(4px + var(--mic, 0) * 12px)' }} />
      <span className="w-1.5 rounded-sm bg-fg-secondary" style={{ height: 'calc(4px + var(--sys, 0) * 12px)' }} />
    </div>
  )
}

/** `rec mm:ss` on the recording clock; `⏸` while paused. */
function Clock({ state }: { state: MeetingState }) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    if (state !== 'Recording') return
    const t = window.setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [state])
  const ms = elapsed(useMeeting.getState(), now)
  return (
    <span className="shrink-0 font-mono text-[11px] tabular-nums text-fg-secondary">
      {state === 'Paused' ? '⏸' : 'rec'} {mmss(ms)}
    </span>
  )
}
