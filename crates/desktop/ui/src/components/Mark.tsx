/** The theuth mark (T0 "Theta moon", assets/brand): a Θ whose ring is a crescent moon in `currentColor` and whose
 *  bar is a reed pen in carnelian (#8f2f1f, the same in light and dark), so it sits on any surface. */
export function Mark({ className }: { className?: string }) {
  return (
    <svg viewBox="14 14 92 92" className={className} aria-hidden="true">
      <path fill="currentColor" fillRule="evenodd" d="M18 60 a42 42 0 1 0 84 0 a42 42 0 1 0 -84 0 Z M27 60 a37 37 0 1 0 74 0 a37 37 0 1 0 -74 0 Z" />
      <path fill="#8f2f1f" d="M24 56.5 H86 L101 60 L86 63.5 H24 Z" />
    </svg>
  )
}
