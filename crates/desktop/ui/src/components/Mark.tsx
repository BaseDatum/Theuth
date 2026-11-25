import { useId } from 'react'
import { cn } from '@/lib/utils'

const RING = 'M18 60 a42 42 0 1 0 84 0 a42 42 0 1 0 -84 0 Z M27 60 a37 37 0 1 0 74 0 a37 37 0 1 0 -74 0 Z'
const DISC = 'M18 60 a42 42 0 1 0 84 0 a42 42 0 1 0 -84 0 Z'
const BAR = 'M24 56.5 H86 L101 60 L86 63.5 H24 Z'
const PEN = '#8f2f1f'

/** The theuth mark (T0 "Theta moon", assets/brand): a Θ whose ring is a crescent moon in `currentColor` and whose
 *  bar is a reed pen in carnelian (#8f2f1f, the same in light and dark), so it sits on any surface. */
export function Mark({ className }: { className?: string }) {
  return (
    <svg viewBox="14 14 92 92" className={className} aria-hidden="true">
      <path fill="currentColor" fillRule="evenodd" d={RING} />
      <path fill={PEN} d={BAR} />
    </svg>
  )
}

/**
 * The mark as an eclipse: a shadow disc (r 37) slides left → right across the full moon (r 42), passes the annular
 * ring at mid-eclipse, rests on the logo's own crescent, then slides off. The pen never moves. `.theta-eclipse`
 * (styles.css) drives it; with reduced motion it holds on the logo's crescent. `duration` in seconds.
 */
export function EclipseMark({ className, duration = 8 }: { className?: string; duration?: number }) {
  const mask = `theta-eclipse-${useId().replace(/:/g, '')}`
  return (
    <svg viewBox="14 14 92 92" className={cn('shrink-0', className)} aria-hidden="true">
      <mask id={mask} maskUnits="userSpaceOnUse" x="0" y="0" width="120" height="120">
        <rect width="120" height="120" fill="#fff" />
        <circle className="theta-eclipse" cx="60" cy="60" r="37" fill="#000" style={{ animationDuration: `${duration}s` }} />
      </mask>
      <path fill="currentColor" d={DISC} mask={`url(#${mask})`} />
      <path fill={PEN} d={BAR} />
    </svg>
  )
}

/**
 * The send button's glyph: the mark at rest; each new `pulse` (> 0) plays the eclipse once (1.4 s, `.theta-eclipse-once`
 * in styles.css): the shadow slides off to the full moon, comes back round and settles on the crescent. The pen never moves.
 */
export function SendMark({ pulse, className }: { pulse: number; className?: string }) {
  const mask = `theta-send-${useId().replace(/:/g, '')}`
  return (
    <svg viewBox="14 14 92 92" className={cn('shrink-0', className)} aria-hidden="true">
      <mask id={mask} maskUnits="userSpaceOnUse" x="0" y="0" width="120" height="120">
        <rect width="120" height="120" fill="#fff" />
        <circle key={pulse} className={pulse > 0 ? 'theta-eclipse-once' : 'theta-eclipse-rest'} cx="60" cy="60" r="37" fill="#000" />
      </mask>
      <path fill="currentColor" d={DISC} mask={`url(#${mask})`} />
      <path fill={PEN} d={BAR} />
    </svg>
  )
}

/**
 * The mark as a one-colour glyph (option C) on the lucide 24-unit grid, for small sizes beside other glyphs: the moon in
 * `currentColor`, the pen on top at half strength, no carnelian. `animate` runs the eclipse (`.theta-glyph-eclipse`,
 * 3 s); still, it rests on the logo crescent.
 */
export function ThetaGlyph({ animate = false, className }: { animate?: boolean; className?: string }) {
  const mask = `theta-glyph-${useId().replace(/:/g, '')}`
  return (
    <svg viewBox="0 0 24 24" className={cn('shrink-0', className)} aria-hidden="true">
      <mask id={mask} maskUnits="userSpaceOnUse" x="0" y="0" width="24" height="24">
        <rect width="24" height="24" fill="#fff" />
        <circle className={animate ? 'theta-glyph-eclipse' : 'theta-glyph-rest'} cx="12" cy="12" r="8.4" fill="#000" />
      </mask>
      <circle cx="12" cy="12" r="9.5" fill="currentColor" mask={`url(#${mask})`} />
      <path fill="currentColor" opacity={0.55} d="M3.6 11.1 H18.4 L21.8 12 L18.4 12.9 H3.6 Z" />
    </svg>
  )
}
