// 96 §5.1: the island window in voice mode — a 440 × 44 strip while listening/sending (wave, live words, clock,
// lock glyph, hint), a strip with a spinner and one status line while the desk works, a strip grown to fit the
// spoken text (small output wave, Stop, subtitles) while this webview plays a block, a strip with the last sentence
// for `DONE_MS`, and a draggable strip while Settings places it. `lib/voice-key.ts` owns the state; this file paints
// it, feeds it this webview's playback (`speech.useSpeaking()`) and sizes the window through the island's own
// `islandPlace`/`islandShow`/`islandHide`.

import { emitTo } from '@tauri-apps/api/event'
import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { LoaderCircle, Lock, Square } from 'lucide-react'
import { Fragment, useCallback, useEffect, useRef, useState } from 'react'
import { VoiceWave, useVoiceSource } from '@/components/voice/VoiceWave'
import type { VisualFrame } from '@/lib/audio/types'
import { createFrameRelay } from '@/lib/audio/frame-source'
import * as dictation from '@/lib/dictation'
import { useMeetingSettings } from '@/lib/meeting-settings'
import * as speech from '@/lib/speech'
import { islandHide, islandPlace, islandShow, isTauri } from '@/lib/tauri'
import { cn } from '@/lib/utils'
import { type VisualSource, analyserNodeSource, tapOrLevels } from '@/lib/visual-source'
import {
  DEFAULT_HINT,
  HINT_USES,
  MOUSE_HINT,
  cancel,
  donePlacing,
  lastSentence,
  playback,
  seed,
  statusLine,
  useVoiceKey,
  useVoiceKeySettings,
} from '@/lib/voice-key'
import { mmss } from '@/store/meeting'
import { useSessions } from '@/store/sessions'

const WIDTH = 440
const STRIP = 44
/** The grown panel's ceiling while speaking (§0.3). */
const REPLY = 132
/** The subtitles' bottom padding (`pb-3`). */
const PAD = 12
/** `islandShow` once per settled size; `Island.tsx`'s `MOVE_SAVE_MS` for the drag end. */
const SHOW_MS = 40
const MOVE_SAVE_MS = 300

/** 83 §1 (as `routes/talk.$session.tsx`): an analyser on speech.ts's output node — every TTS packet this webview plays. */
function speakerSource(): VisualSource {
  const relay = createFrameRelay<VisualFrame>()
  let tap: VisualSource | null = null
  const off = speech.onOutputReady((node) => {
    tap = analyserNodeSource(node.context, node)
    relay.setSource(tap.source)
  })
  return {
    source: relay,
    dispose: () => {
      off()
      tap?.dispose()
    },
  }
}

/** Bring the main window to the desk's chat (the tray's `navigate` route, `App.tsx`). */
async function openChat(desk: string): Promise<void> {
  if (!isTauri()) return
  await emitTo('main', 'navigate', `/c/${desk}`)
  const main = await WebviewWindow.getByLabel('main')
  await main?.show()
  await main?.setFocus()
}

export function VoicePanel() {
  const s = useVoiceKey()
  const dict = dictation.useDictation()
  const settings = useVoiceKeySettings()
  const placed = useRef(false)
  const key = useRef<string | null>(null)

  // The window is transparent; only the strip paints (`styles.css` `html[data-island]`).
  useEffect(() => {
    document.documentElement.dataset.island = ''
    return () => {
      delete document.documentElement.dataset.island
    }
  }, [])

  // The island webview may boot mid-gesture: ask the shell where the machine is.
  useEffect(() => void seed(), [])

  // This webview's playback drives `working ↔ speaking` and the reply's end (`lib/speech.ts` exports no subscriber).
  const speaking = speech.useSpeaking()
  useEffect(() => playback(speaking?.entry ?? null), [speaking])

  // Speaking grows to the spoken text, up to `REPLY`; measured from the subtitles' content (one `islandShow` per settled size).
  const [fit, setFit] = useState(REPLY)
  const measure = useCallback((el: HTMLElement | null) => {
    if (!el) return
    const ro = new ResizeObserver(() => setFit(Math.min(REPLY, STRIP + Math.ceil(el.offsetHeight) + PAD)))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const height = s.phase === 'speaking' ? fit : STRIP
  useEffect(() => {
    if (s.phase === 'idle') {
      placed.current = false
      void islandHide()
      return
    }
    const show = () => void islandShow(WIDTH, height)
    if (placed.current) {
      const t = window.setTimeout(show, SHOW_MS)
      return () => clearTimeout(t)
    }
    placed.current = true
    void islandPlace(useMeetingSettings.getState().positions)
      .then(([, , k]) => (key.current = k))
      .finally(show)
  }, [s.phase, height])

  // Placing: a drag end saves the spot under the current screen-configuration key (as `Island.tsx` does for the meeting island).
  useEffect(() => {
    if (s.phase !== 'placing' || !isTauri()) return
    const win = getCurrentWindow()
    let timer = 0
    const off = win.onMoved(() => {
      clearTimeout(timer)
      timer = window.setTimeout(async () => {
        const k = key.current
        if (!k) return
        const p = (await win.outerPosition()).toLogical(await win.scaleFactor())
        useMeetingSettings.getState().setPosition(k, [p.x, p.y])
      }, MOVE_SAVE_MS)
    })
    return () => {
      clearTimeout(timer)
      void off.then((f) => f())
    }
  }, [s.phase])

  // Esc inside the panel (the shell holds the global Esc only between press and send).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && cancel()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const listening = s.phase === 'listening'
  const micSource = useVoiceSource(listening && dict.phase === 'dictating', () => tapOrLevels(dictation.tap(), dictation.onLevels))
  const speakerWave = useVoiceSource(s.phase === 'speaking', speakerSource)

  if (s.phase === 'idle') return null
  return (
    <div
      className="flex h-dvh w-full flex-col overflow-hidden rounded-[22px] bg-layer-1 text-foreground shadow-lg"
      role="status"
      aria-live="polite"
    >
      <div data-tauri-drag-region={s.phase === 'placing' || undefined} className="flex h-11 shrink-0 items-center gap-2 px-3">
        {s.phase === 'hint' ? (
          <span className="truncate text-sm text-fg-secondary">{s.hint}</span>
        ) : s.phase === 'placing' ? (
          <>
            <span data-tauri-drag-region className="min-w-0 flex-1 cursor-grab truncate text-sm text-fg-secondary">
              Drag me where you want the voice panel
            </span>
            <button type="button" className="shrink-0 rounded-full px-2.5 py-1 text-xs text-fg-accent hover:bg-fg-accent/10" onClick={donePlacing}>
              Done
            </button>
          </>
        ) : s.phase === 'error' ? (
          <span className="truncate text-sm text-destructive">{s.error}</span>
        ) : s.phase === 'working' ? (
          <Working desk={s.desk} approval={s.approval} />
        ) : s.phase === 'speaking' ? (
          <>
            <VoiceWave state="speaking" source={speakerWave} label="theuth speaking" className="h-7 w-16 shrink-0" />
            <span className="min-w-0 flex-1" />
            <Stop />
          </>
        ) : s.phase === 'done' ? (
          <>
            <LastWords desk={s.desk} entry={s.spoken} />
            {s.desk && (
              <button type="button" className="shrink-0 text-xs text-fg-accent hover:underline" onClick={() => void openChat(s.desk ?? '').then(cancel)}>
                Open chat
              </button>
            )}
          </>
        ) : (
          <>
            <VoiceWave state={listening ? 'listening' : 'thinking'} source={listening ? micSource : null} label="Voice level" className="h-7 w-16 shrink-0" />
            <Words utts={dict.active?.utts ?? []} text={s.text} muted={!listening} />
            {s.phase === 'sending' && <Dots />}
            {listening && settings.hintsShown <= HINT_USES && !s.locked && !dict.active?.utts.length && (
              <span className="shrink-0 text-xs text-fg-tertiary">{settings.annotate === 'mouse' ? MOUSE_HINT : DEFAULT_HINT}</span>
            )}
            {s.locked && <Lock className="size-3.5 shrink-0 text-fg-secondary" aria-label="Locked" />}
            <Clock from={s.startedAt} running={listening} frozen={s.durationMs} />
          </>
        )}
      </div>
      {s.phase === 'speaking' && (
        <div className="flex min-h-0 flex-1 flex-col px-4 pb-3 text-sm">
          <Subtitles desk={s.desk} fallback={s.reply} measure={measure} />
        </div>
      )}
    </div>
  )
}

/** Live words: each open utterance muted, closed ones saturated (`Composer.tsx`'s `DictatedText` rule); the final text once stopped. */
function Words({ utts, text, muted }: { utts: dictation.Utt[]; text: string; muted: boolean }) {
  const spoken = utts.filter((u) => dictation.shown(u) !== '')
  return (
    <div className={cn('min-w-0 flex-1 truncate text-sm', muted && 'text-fg-tertiary')}>
      {spoken.length
        ? spoken.map((u, i) => (
            <Fragment key={i}>
              {i > 0 && ' '}
              <span className={cn('transition-colors duration-150', !u.final && 'text-fg-tertiary')}>{dictation.shown(u)}</span>
            </Fragment>
          ))
        : text || 'Sending…'}
    </div>
  )
}

function Dots() {
  return (
    <span className="shrink-0 text-fg-tertiary" aria-label="Sending">
      <span className="animate-pulse">·</span>
      <span className="animate-pulse [animation-delay:150ms]">·</span>
      <span className="animate-pulse [animation-delay:300ms]">·</span>
    </span>
  )
}

/** `mm:ss` since the press while listening; the hold's duration afterwards. */
function Clock({ from, running, frozen }: { from: number; running: boolean; frozen: number }) {
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    if (!running) return
    const t = window.setInterval(() => setNow(Date.now()), 250)
    return () => clearInterval(t)
  }, [running])
  return <span className="shrink-0 font-mono text-xs tabular-nums text-fg-secondary">{mmss(running ? now - from : frozen)}</span>
}

/** The Stop square (→ `speech.stop` + `point_cancel`). */
function Stop() {
  return (
    <button
      type="button"
      className="flex size-7 shrink-0 items-center justify-center rounded-full text-fg-secondary hover:bg-fg-accent/10"
      aria-label="Stop"
      onClick={cancel}
    >
      <Square className="size-3 fill-current" />
    </button>
  )
}

/** The working row: a spinner and what the desk is doing (its slice; the running tool's name when one is pending). */
function Working({ desk, approval }: { desk: string | null; approval: boolean }) {
  const slice = useSessions((st) => (desk ? st.sessions[desk] : undefined))
  let tool: string | undefined
  for (let i = (slice?.entries.length ?? 0) - 1; i >= 0; i--) {
    const e = slice?.entries[i]
    if (e?.kind === 'tool' && !e.result) {
      tool = e.name
      break
    }
    if (e?.kind === 'user') break
  }
  return (
    <>
      <LoaderCircle className="size-4 shrink-0 animate-spin text-fg-tertiary" aria-hidden />
      <span className="min-w-0 flex-1 truncate text-sm text-fg-secondary">
        {statusLine(slice, tool, approval)}
        {approval && (
          <>
            {' — '}
            <button type="button" className="text-fg-accent hover:underline" onClick={() => void openChat(desk ?? '')}>
              Open chat
            </button>
          </>
        )}
      </span>
      <Stop />
    </>
  )
}

/** `done`: the last sentence of the entry spoken here, muted; "Done" when nothing was spoken. */
function LastWords({ desk, entry }: { desk: string | null; entry: string | null }) {
  const text = useSessions((st) => (desk && entry ? st.sessions[desk]?.entries.find((e) => e.id === entry) : undefined))
  const line = text && 'text' in text ? lastSentence(text.text) : ''
  return <span className="min-w-0 flex-1 truncate text-sm text-fg-tertiary">{line || 'Done'}</span>
}

/** The block being spoken (`speech.useSpeaking().entry` in the desk's slice), else the turn's text so far; `measure` sees its content. */
function Subtitles({ desk, fallback, measure }: { desk: string | null; fallback: string; measure: (el: HTMLElement | null) => void }) {
  const speaking = speech.useSpeaking()
  const entry = useSessions((st) => (desk && speaking ? st.sessions[desk]?.entries.find((e) => e.id === speaking.entry) : undefined))
  const text = (entry && 'text' in entry ? entry.text : fallback).trim()
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.scrollTo({ top: ref.current.scrollHeight })
  }, [text])
  return (
    <div ref={ref} className="min-h-0 flex-1 overflow-y-auto">
      <div ref={measure} className="whitespace-pre-wrap break-words leading-5 text-fg-secondary">
        {text || <span className="text-fg-tertiary">…</span>}
      </div>
    </div>
  )
}
