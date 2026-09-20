/** The ibis mark from icons/icon.svg in `currentColor`; the eye is a hole so it sits on any surface. */
export function Mark({ className }: { className?: string }) {
  return (
    <svg viewBox="120 120 784 784" className={className} aria-hidden="true">
      <mask id="mark-eye">
        <rect x="120" y="120" width="784" height="784" fill="#fff" />
        <circle cx="610" cy="288" r="36" fill="#000" />
      </mask>
      <g fill="currentColor" mask="url(#mark-eye)">
        <circle cx="640" cy="340" r="165" />
        <path d="M 660 178 C 400 196, 230 470, 186 846 L 244 890 C 300 570, 400 380, 600 500 Z" />
      </g>
    </svg>
  )
}
