import { Atom, Eclipse, Flip, Orbit } from 'loading-dev'
import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'
import type { Activity } from '@/store/sessions'
import { EclipseMark } from '@/components/Mark'
import { Pulse } from './Pulse'

/*
 * Animated glyphs (loading-dev: CSS-only, colour from `currentColor`); component files import these, never the
 * library. Sizes match the lucide glyphs they replaced (14 = `size-3.5`, 20 = `size-5`) so nothing shifts. The
 * spinners size themselves from the `size` prop — never a `size-*` class on them, it would fight the library's width.
 */

/** A tool call is running: the small orbit after the tool name. */
export const ToolSpinner = () => <Orbit size={14} className="shrink-0 text-fg-tertiary" />

/** The model is thinking: the Θ-moon eclipsing (a 3 s loop); `className` sizes the box (`size-3.5`) for the sidebar's activity glyph. */
export function ThinkingIndicator({ className, ...props }: ComponentProps<'span'>) {
  return (
    <span className={cn('inline-flex size-3.5 shrink-0 items-center justify-center', className)} {...props}>
      <EclipseMark className="size-3.5" duration={3} />
    </span>
  )
}

/** A knowledge harvest is in flight: an eclipse, same box as `ThinkingIndicator` for the sidebar's activity glyph. */
export function HarvestIndicator({ className, ...props }: ComponentProps<'span'>) {
  return (
    <span className={cn('inline-flex size-3.5 shrink-0 items-center justify-center text-fg-tertiary', className)} {...props}>
      <Eclipse size={14} />
    </span>
  )
}

/** The transcript footer's glyph for `activity`, in a fixed 14 px box so the row holds still as it changes; the Pulse dot for the rest. */
export function ActivityIndicator({ activity }: { activity: Activity }) {
  return (
    <span className="inline-flex size-3.5 shrink-0 items-center justify-center">
      {activity === 'harvesting' ? (
        <Eclipse size={14} className="text-fg-tertiary" />
      ) : activity === 'thinking' || activity === 'reasoning' ? (
        <EclipseMark className="size-3.5" duration={3} />
      ) : activity === 'tool' ? (
        <Orbit size={14} className="text-fg-tertiary" />
      ) : (
        <Pulse />
      )}
    </span>
  )
}

/** The session's headless browser: an atom that spins while a `browser_*` call is in flight and holds still otherwise. */
export function BrowserIndicator({ live, size = 16, className }: { live: boolean; size?: number; className?: string }) {
  return <Atom size={size} playState={live ? 'running' : 'paused'} className={cn('shrink-0', className)} />
}

/** An artifact being written or compiled: the card's 56×72 preview slot with a flipping tile in it. */
export const ArtifactWriting = (
  <div className="flex h-[72px] w-14 shrink-0 items-center justify-center rounded-lg bg-layer-2 text-fg-tertiary">
    <Flip size={24} />
  </div>
)

/**
 * lucide's `SquareTerminal`, path for path, with the prompt chevron and cursor addressable: `live` nudges the chevron
 * and blinks the cursor (`.pty-live` in styles.css); still otherwise. Takes a LucideIcon's props; `children` (an SVG
 * `<title>` tooltip) render after the shapes.
 */
export function PtyIcon({ live, className, children, ...props }: ComponentProps<'svg'> & { live: boolean }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="24"
      height="24"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      className={cn('lucide lucide-square-terminal', live && 'pty-live', className)}
      {...props}
    >
      <path className="pty-prompt" d="m7 11 2-2-2-2" />
      <path className="pty-cursor" d="M11 13h4" />
      <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
      {children}
    </svg>
  )
}
