// 96 §5.1: the island window in voice mode — a 440 × 44 strip while listening/sending (wave, live words, clock,
// lock glyph, hint) that grows to 132 while theuth replies (subtitles, the speaker's wave, Stop) and stays
// `DONE_MS` after the last word. `lib/voice-key.ts` owns the state; this file paints it and sizes the window
// through the island's own `islandPlace`/`islandShow`/`islandHide`.

import { emitTo } from '@tauri-apps/api/event'
import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { Lock, Square } from 'lucide-react'
import { Fragment, useEffect, useRef, useState } from 'react'
import { VoiceWave, useVoiceSource } from '@/components/voice/VoiceWave'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { createFrameRelay } from '@/lib/audio/frame-source'
import * as dictation from '@/lib/dictation'
import { useMeetingSettings } from '@/lib/meeting-settings'
import * as speech from '@/lib/speech'
import { islandHide, islandPlace, islandShow, isTauri } from '@/lib/tauri'
import { cn } from '@/lib/utils'
import { type VisualSource, analyserNodeSource, tapOrLevels } from '@/lib/visual-source'
import { DEFAULT_HINT, HINT_USES, cancel, seed, useVoiceKey, useVoiceKeySettings } from '@/lib/voice-key'
import { mmss } from '@/store/meeting'
import { useSessions } from '@/store/sessions'

const WIDTH = 440
const STRIP = 44
const REPLY = 132

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
  const hintsShown = useVoiceKeySettings((st) => st.hintsShown)
  const placed = useRef(false)

  // The window is transparent; only the strip paints (`styles.css` `html[data-island]`).
  useEffect(() => {
    document.documentElement.dataset.island = ''
    return () => {
      delete document.documentElement.dataset.island
    }
  }, [])

  // The island webview may boot mid-gesture: ask the shell where the machine is.
  useEffect(() => void seed(), [])

  // Size follows the phase; placed once per mount (the meeting island's rule), hidden when idle.
  const tall = s.phase === 'replying' || s.phase === 'done'
  useEffect(() => {
    if (s.phase === 'idle') {
      placed.current = false
      void islandHide()
      return
    }
    const show = () => void islandShow(WIDTH, tall ? REPLY : STRIP)
    if (placed.current) show()
    else {
      placed.current = true
      void islandPlace(useMeetingSettings.getState().positions).finally(show)
    }
  }, [s.phase, tall])

  // Esc inside the panel (the shell holds the global Esc only between press and send).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && cancel()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const listening = s.phase === 'listening'
  const micSource = useVoiceSource(listening && dict.phase === 'dictating', () => tapOrLevels(dictation.tap(), dictation.onLevels))
  const speakerWave = useVoiceSource(tall, speakerSource)

  if (s.phase === 'idle') return null
  return (
    <div
      className="flex h-dvh w-full flex-col overflow-hidden rounded-[22px] bg-layer-1 text-foreground shadow-lg"
      role="status"
      aria-live="polite"
    >
      <div className="flex h-11 shrink-0 items-center gap-2 px-3">
        {s.phase === 'hint' ? (
          <span className="truncate text-sm text-fg-secondary">{s.hint}</span>
        ) : tall ? (
          <ReplyStrip source={speakerWave} />
        ) : (
          <>
            <VoiceWave state={listening ? 'listening' : 'thinking'} source={listening ? micSource : null} label="Voice level" className="h-7 w-16 shrink-0" />
            <Words utts={dict.active?.utts ?? []} text={s.text} muted={!listening} />
            {s.phase === 'sending' && <Dots />}
            {listening && hintsShown <= HINT_USES && !s.locked && !dict.active?.utts.length && (
              <span className="shrink-0 text-xs text-fg-tertiary">{DEFAULT_HINT}</span>
            )}
            {s.locked && <Lock className="size-3.5 shrink-0 text-fg-secondary" aria-label="Locked" />}
            <Clock from={s.startedAt} running={listening} frozen={s.durationMs} />
          </>
        )}
      </div>
      {tall && (
        <div className="flex min-h-0 flex-1 flex-col gap-1 px-4 pb-3 text-sm">
          <Subtitles desk={s.desk} fallback={s.reply} />
          {s.error && <div className="truncate text-xs text-destructive">{s.error}</div>}
          {s.approval && (
            <button type="button" className="self-start text-xs text-fg-accent hover:underline" onClick={() => void openChat(s.desk ?? '')}>
              Needs your approval — Open chat
            </button>
          )}
          {s.phase === 'done' && !s.approval && s.desk && (
            <button type="button" className="self-start text-xs text-fg-accent hover:underline" onClick={() => void openChat(s.desk ?? '').then(cancel)}>
              Open chat
            </button>
          )}
        </div>
      )}
    </div>
  )
}

/** Live words: each open utterance muted, closed ones saturated (`Composer.tsx`'s `DictatedText` rule); the final text once stopped. */
function Words({ utts, text, muted }: { utts: dictation.Utt[]; text: string; muted: boolean }) {
  const spoken = utts.filter((u) => u.text !== '')
  return (
    <div className={cn('min-w-0 flex-1 truncate text-sm', muted && 'text-fg-tertiary')}>
      {spoken.length
        ? spoken.map((u, i) => (
            <Fragment key={i}>
              {i > 0 && ' '}
              <span className={cn('transition-colors duration-150', !u.final && 'text-fg-tertiary')}>{u.text}</span>
            </Fragment>
          ))
        : text}
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

/** The reply's top row: the speaker's wave and the Stop square (→ `speech.stop` + `point_cancel`). */
function ReplyStrip({ source }: { source: FrameSource<VisualFrame> | null }) {
  return (
    <>
      <VoiceWave state="speaking" source={source} label="theuth speaking" className="h-7 min-w-0 flex-1" />
      <button
        type="button"
        className="flex size-7 shrink-0 items-center justify-center rounded-full text-fg-secondary hover:bg-fg-accent/10"
        aria-label="Stop"
        onClick={cancel}
      >
        <Square className="size-3 fill-current" />
      </button>
    </>
  )
}

/** The block being spoken (`speech.useSpeaking().entry` in the desk's slice), else the turn's text so far. */
function Subtitles({ desk, fallback }: { desk: string | null; fallback: string }) {
  const speaking = speech.useSpeaking()
  const entry = useSessions((st) => (desk && speaking ? st.sessions[desk]?.entries.find((e) => e.id === speaking.entry) : undefined))
  const text = (entry && 'text' in entry ? entry.text : fallback).trim()
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.scrollTo({ top: ref.current.scrollHeight })
  }, [text])
  return (
    <div ref={ref} className="min-h-0 flex-1 overflow-y-auto whitespace-pre-wrap break-words leading-5 text-fg-secondary">
      {text || <span className="text-fg-tertiary">…</span>}
    </div>
  )
}
