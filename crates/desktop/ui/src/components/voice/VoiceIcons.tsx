// The composer's voice glyphs (V2, "Θ voice"): lucide-shaped SVGs (viewBox 24, 2 px round strokes in
// `currentColor`, sized by `className`) with one carnelian stroke — the logo's pen — in the theme's shade.

import { type ReactNode, type SVGProps, useEffect, useRef } from 'react'
import { useFrameSource } from '@/hooks/use-frame-source'
import { useReducedMotion } from '@/hooks/use-reduced-motion'
import { subscribeFrame } from '@/lib/audio/frame-loop'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { type AudioFeatures, SILENT_FEATURES, WaveformEngine, visualFrameToAudioFeatures } from '@/lib/audio/waveform'

/** The carnelian pen (`styles.css` `--project-carnelian`: #8f2f1f light, #d95038 dark). */
export const PEN_STROKE = 'var(--project-carnelian)'
const pen = { stroke: PEN_STROKE }

type GlyphProps = Omit<SVGProps<SVGSVGElement>, 'children'>

function Glyph({ children, ...props }: GlyphProps & { children: ReactNode }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      width={24}
      height={24}
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}
    >
      {children}
    </svg>
  )
}

/** The microphone: capsule and stand, a carnelian bar across the capsule. */
export function MicGlyph(props: GlyphProps) {
  return (
    <Glyph {...props}>
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
      <path d="M6.5 8.5h11" style={pen} />
    </Glyph>
  )
}

/** Talk: the Θ ring, its bar a still carnelian wave. */
export function TalkGlyph(props: GlyphProps) {
  return (
    <Glyph {...props}>
      <circle cx="12" cy="12" r="9" />
      <path d="M5.5 12c1.6-3.2 3.2-3.2 4.3 0s2.7 3.2 4.3 0 3-3.2 4.4 0" style={pen} />
    </Glyph>
  )
}

/** Samples across the line, its span and the furthest it swings (well inside the r 9 ring). */
const POINTS = 25
const X0 = 6
const X1 = 18
const MID = 12
const MAX_D = 3.2
/** A source that stops sending for this long counts as silent (as `VoiceWave`). */
const STALE_MS = 200
export const FLAT_LINE = `M${X0} ${MID}H${X1}`

/** The line's path for `disps` (one per sample): ends tapered to the centre by sin(πu), clamped to ±`MAX_D`. */
export function livePath(disps: ArrayLike<number>): string {
  let d = ''
  for (let i = 0; i < POINTS; i++) {
    const u = i / (POINTS - 1)
    const y = MID + Math.max(-MAX_D, Math.min(MAX_D, (disps[i] ?? 0) * Math.sin(Math.PI * u)))
    d += `${i === 0 ? 'M' : 'L'}${(X0 + u * (X1 - X0)).toFixed(2)} ${y.toFixed(2)}`
  }
  return d
}

/**
 * Talk while it runs: the ring in `currentColor` and a short carnelian line inside it, the dictation waveform's
 * engine driven by the talk's live frames (`source`; `active` while someone may be heard). Silence, or no source,
 * is the engine's idle ripple; reduced motion is a still straight line.
 */
export function TalkLiveGlyph({ source, active, ...props }: GlyphProps & { source: FrameSource<VisualFrame> | null; active: boolean }) {
  const reducedMotion = useReducedMotion()
  const line = useRef<SVGPathElement | null>(null)
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
    const path = line.current
    if (!path || reducedMotion) {
      path?.setAttribute('d', FLAT_LINE)
      return
    }
    const engine = new WaveformEngine()
    const disps = new Float32Array(POINTS)
    let lastMs: number | null = null
    return subscribeFrame((nowMs) => {
      const dt = lastMs === null ? 0 : (nowMs - lastMs) / 1000
      lastMs = nowMs
      const fresh = performance.now() - input.current.at < STALE_MS
      engine.update(active && fresh ? input.current.features : SILENT_FEATURES, active, dt)
      engine.displacements(POINTS, MAX_D, disps)
      path.setAttribute('d', livePath(disps))
    })
  }, [active, reducedMotion])

  return (
    <Glyph data-slot="talk-live" {...props}>
      <circle cx="12" cy="12" r="9" />
      <path ref={line} d={FLAT_LINE} strokeWidth={1.8} style={pen} />
    </Glyph>
  )
}
