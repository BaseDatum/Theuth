// Streamdown's mermaid pan/zoom listens for `wheel` (±zoomStep per event) and
// one-pointer drag on `[data-streamdown="mermaid"] > div`; it has no pinch.
// Two fingers on a diagram (inline or fullscreen) become synthetic wheel
// events on that element, so its own state, buttons and reset stay in charge.

const TARGET = '[data-streamdown="mermaid"] > div'
/** Finger-distance ratio per wheel step; streamdown's step is 0.1 of a 0.5–3 range, so ~25 steps span a 2.5× pinch. */
const RATIO = 1.04

const span = (t: TouchList) => {
  const [a, b] = [t.item(0) as Touch, t.item(1) as Touch]
  return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY)
}

export function trackPinch(): void {
  let target: Element | null = null
  let dist = 0
  document.addEventListener(
    'touchstart',
    (e) => {
      target = e.touches.length === 2 ? (e.target as Element).closest(TARGET) : null
      if (target) dist = span(e.touches)
    },
    { passive: true },
  )
  document.addEventListener(
    'touchmove',
    (e) => {
      if (!target || e.touches.length !== 2) return
      e.preventDefault()
      const now = span(e.touches)
      for (; now > dist * RATIO; dist *= RATIO) target.dispatchEvent(new WheelEvent('wheel', { deltaY: -1, cancelable: true }))
      for (; now < dist / RATIO; dist /= RATIO) target.dispatchEvent(new WheelEvent('wheel', { deltaY: 1, cancelable: true }))
    },
    { passive: false },
  )
}
