// theuth's one dictation / voice-note / talk waveform: the Dialogue single-line wave (lib/audio/dialogue-waveform)
// in the accent colour. `text-fg-accent` (or a caller's `text-*`) is the only colour hook: the line is the resolved
// CSS `color`, re-read when the theme or the element's classes change.

import { type CSSProperties, type ReactNode, useEffect, useRef, useState } from 'react'
import { useFrameSource } from '@/hooks/use-frame-source'
import { useReducedMotion } from '@/hooks/use-reduced-motion'
import { useVisibility } from '@/hooks/use-visibility'
import { type AudioFeatures, SILENT_FEATURES, WaveformEngine, visualFrameToAudioFeatures } from '@/lib/audio/dialogue-waveform'
import { subscribeFrame } from '@/lib/audio/frame-loop'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { cn } from '@/lib/utils'
import type { VisualSource } from '@/lib/visual-source'

export type VoiceState = 'idle' | 'listening' | 'recording' | 'thinking' | 'speaking'

/** A source that stops sending for this long counts as silent, so the line settles back into the ocean. */
const STALE_MS = 200
/** Frames between re-reads of the resolved colour (catches caller class changes the theme observer can't see). */
const COLOR_REFRESH_FRAMES = 30
/** One breath of the `thinking` pulse, in seconds. */
const BREATH_SECONDS = 2.4
const INSET = 4

interface Geometry {
  width: number
  height: number
  points: number
  lineWidth: number
  glowWidth: number
}

/** CSS size → stroke geometry: 1.7 px core on compact bars (≤ 28 px), 1.9 px on tall ones (≥ 56 px). */
function geometryFor(width: number, height: number): Geometry {
  const t = Math.max(0, Math.min(1, (height - 28) / 28))
  const lineWidth = 1.7 + 0.2 * t
  return { width, height, points: width >= 160 ? 96 : 72, lineWidth, glowWidth: lineWidth * 1.6 }
}

function drawWave(ctx: CanvasRenderingContext2D, g: Geometry, disps: Float32Array, maxD: number, color: string, glowAlpha: number, alpha: number) {
  const usableW = Math.max(1, g.width - INSET * 2)
  const cy = g.height / 2
  const n = g.points
  const path = () => {
    ctx.beginPath()
    ctx.moveTo(INSET, cy + disps[0]!)
    for (let i = 1; i < n - 1; i++) {
      const xi = INSET + (i / (n - 1)) * usableW
      const yi = cy + disps[i]!
      const xn = INSET + ((i + 1) / (n - 1)) * usableW
      const yn = cy + disps[i + 1]!
      ctx.quadraticCurveTo(xi, yi, (xi + xn) / 2, (yi + yn) / 2)
    }
    ctx.lineTo(INSET + usableW, cy + disps[n - 1]!)
  }
  ctx.clearRect(0, 0, g.width, g.height)
  if (maxD <= 0) return
  ctx.lineCap = 'round'
  ctx.lineJoin = 'round'
  ctx.strokeStyle = color
  path()
  ctx.globalAlpha = glowAlpha * alpha
  ctx.lineWidth = g.glowWidth
  ctx.stroke()
  path()
  ctx.globalAlpha = alpha
  ctx.lineWidth = g.lineWidth
  ctx.stroke()
  ctx.globalAlpha = 1
}

/**
 * `idle` is the ocean; `thinking` is the ocean breathing (`data-loading`); `listening`/`recording`/`speaking` drive
 * voice packets from `source` and settle back into the ocean when it is null or silent. Reduced motion draws one
 * static resting wave.
 */
export function VoiceWave({ source, state, className, style, label }: { source: FrameSource<VisualFrame> | null; state: VoiceState; className?: string; style?: CSSProperties; label: string }) {
  const reducedMotion = useReducedMotion()
  const rootRef = useRef<HTMLDivElement | null>(null)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const visibleRef = useVisibility(rootRef)
  const active = state === 'listening' || state === 'recording' || state === 'speaking'
  const thinking = state === 'thinking'
  const input = useRef<{ features: AudioFeatures; at: number; prevLevel: number }>({ features: { ...SILENT_FEATURES }, at: 0, prevLevel: 0 })

  useFrameSource(
    source,
    (frame) => {
      const { features, nextLevel } = visualFrameToAudioFeatures(frame, input.current.prevLevel)
      input.current = { features, at: performance.now(), prevLevel: nextLevel }
    },
    { enabled: active && !reducedMotion },
  )

  useEffect(() => {
    const root = rootRef.current
    const canvas = canvasRef.current
    const ctx = canvas?.getContext('2d')
    if (!(root && canvas && ctx)) return
    const engine = new WaveformEngine()
    let geometry = geometryFor(0, 0)
    let disps = new Float32Array(geometry.points)
    let color = ''
    let glowAlpha = 0.24
    let framesSinceColor = 0
    let lastMs: number | null = null

    const readColor = () => {
      color = getComputedStyle(root).color
      glowAlpha = document.documentElement.classList.contains('dark') ? 0.32 : 0.24
    }
    const paint = (maxScale: number, alpha: number) => {
      if (geometry.width < 2 || geometry.height < 2) return
      const maxD = Math.max(0, geometry.height / 2 - 3) * maxScale
      engine.displacements(geometry.points, maxD, disps)
      drawWave(ctx, geometry, disps, maxD, color, glowAlpha, alpha)
    }
    const resize = () => {
      const rect = canvas.getBoundingClientRect()
      const dpr = Math.min(window.devicePixelRatio || 1, 2)
      canvas.width = Math.round(rect.width * dpr)
      canvas.height = Math.round(rect.height * dpr)
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      geometry = geometryFor(rect.width, rect.height)
      if (disps.length !== geometry.points) disps = new Float32Array(geometry.points)
      if (reducedMotion) paint(1, 1)
    }

    readColor()
    resize()
    const resizeObserver = new ResizeObserver(resize)
    resizeObserver.observe(canvas)
    const themeObserver = new MutationObserver(() => {
      readColor()
      if (reducedMotion) paint(1, 1)
    })
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style', 'data-theme'] })
    if (reducedMotion) {
      return () => {
        resizeObserver.disconnect()
        themeObserver.disconnect()
      }
    }

    const tick = (nowMs: number) => {
      const dt = lastMs === null ? 0 : (nowMs - lastMs) / 1000
      lastMs = nowMs
      if (!visibleRef.current) return
      if (++framesSinceColor >= COLOR_REFRESH_FRAMES) {
        framesSinceColor = 0
        readColor()
      }
      const fresh = performance.now() - input.current.at < STALE_MS
      engine.update(active && fresh ? input.current.features : SILENT_FEATURES, active, dt)
      if (thinking) {
        const breath = 0.5 + 0.5 * Math.sin((engine.elapsedTime * 2 * Math.PI) / BREATH_SECONDS)
        paint(0.7 + 0.3 * breath, 0.6 + 0.4 * breath)
      } else paint(1, 1)
    }
    const unsubscribe = subscribeFrame(tick)
    return () => {
      unsubscribe()
      resizeObserver.disconnect()
      themeObserver.disconnect()
    }
  }, [active, thinking, reducedMotion, visibleRef])

  return (
    <div
      ref={rootRef}
      role="img"
      aria-label={label}
      data-slot="dialogue-waveform"
      data-state={state}
      data-loading={thinking ? '' : undefined}
      className={cn('text-fg-accent relative h-full w-full', className)}
      style={style}
    >
      <canvas ref={canvasRef} aria-hidden className="absolute inset-0 size-full" />
    </div>
  )
}

/**
 * 82 §C: the composer's text region. While a voice note records the waveform IS the box: `children` (the textarea)
 * stay mounted but invisible, so the value, caret and height come back untouched when the recording ends.
 */
export function RecordingBox({ recording, source, children }: { recording: boolean; source: FrameSource<VisualFrame> | null; children: ReactNode }) {
  return (
    <div className={cn('relative', recording && 'min-h-14')}>
      <div className={recording ? 'invisible' : undefined} aria-hidden={recording || undefined}>
        {children}
      </div>
      {recording && <VoiceWave state="recording" source={source} label="Recording" className="absolute inset-0" />}
    </div>
  )
}

/**
 * Build a `VisualSource` while `enabled`, dispose it when not (or on unmount). `make` is read when the flag turns
 * on, so the tap it reaches for is the one that exists at that moment.
 */
export function useVoiceSource(enabled: boolean, make: () => VisualSource): FrameSource<VisualFrame> | null {
  const [source, setSource] = useState<FrameSource<VisualFrame> | null>(null)
  const makeRef = useRef(make)
  makeRef.current = make
  useEffect(() => {
    if (!enabled) return
    const s = makeRef.current()
    setSource(s.source)
    return () => {
      s.dispose()
      setSource(null)
    }
  }, [enabled])
  return source
}
