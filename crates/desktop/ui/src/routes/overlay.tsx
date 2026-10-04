import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useRef } from 'react'
import { onPointCancel, onPointSamples, onPointStart, onPointStop } from '@/lib/tauri'
import { annotateMode, trailColor } from '@/lib/trail'

/** Stroke width, ring diameter and glow in CSS px (96 §0.5). */
const STROKE = 6
const RING = 14
const GLOW = 2

/**
 * 96 §5.3: `/overlay?display=<n>` — one full-window canvas per display, click-through and transparent (the shell
 * sets both). `point:start` clears; `point:samples` appends the display-local points and strokes once per frame
 * in theuth's green with a ring at the live pointer; `point:stop` / `point:cancel` clear. With `pen: false` (the
 * mouse button is up in `mouse` mode) the polyline breaks and only the ring follows, at 40 percent. No engine, no hotkeys.
 */
export const Route = createFileRoute('/overlay')({ component: OverlayRoute })

function OverlayRoute() {
  const ref = useRef<HTMLCanvasElement>(null)
  useEffect(() => {
    // `display` is for labelling only (devtools, screenshots); the shell routes samples by window.
    document.documentElement.dataset.overlay = new URLSearchParams(location.search).get('display') ?? ''
    const canvas = ref.current
    const ctx = canvas?.getContext('2d')
    if (!canvas || !ctx) return
    let path = new Path2D()
    /** The newest point (the ring) and whether the pen was down there (the polyline continues from it). */
    let last: [number, number] | null = null
    let down = false
    let color = trailColor()
    let frame = 0
    const resize = () => {
      canvas.width = Math.round(innerWidth * devicePixelRatio)
      canvas.height = Math.round(innerHeight * devicePixelRatio)
    }
    const clear = () => {
      cancelAnimationFrame(frame)
      frame = 0
      last = null
      down = false
      path = new Path2D()
      ctx.setTransform(1, 0, 0, 1, 0, 0)
      ctx.clearRect(0, 0, canvas.width, canvas.height)
    }
    // The path persists while held (a circle stays a circle); the ring is redrawn at the newest point each frame.
    const draw = () => {
      frame = 0
      const dpr = devicePixelRatio
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      ctx.clearRect(0, 0, innerWidth, innerHeight)
      ctx.globalAlpha = 0.85
      ctx.strokeStyle = color
      ctx.shadowColor = color
      ctx.shadowBlur = GLOW
      ctx.lineCap = ctx.lineJoin = 'round'
      ctx.lineWidth = STROKE
      ctx.stroke(path)
      if (!last) return
      ctx.globalAlpha = down ? 0.85 : 0.4
      ctx.beginPath()
      ctx.arc(last[0], last[1], RING / 2, 0, Math.PI * 2)
      ctx.lineWidth = 2
      ctx.stroke()
    }
    const offs = [
      onPointStart(() => {
        color = trailColor()
        // In `mouse` mode the shell lets this window take the clicks; the cursor says so. Harmless otherwise.
        document.documentElement.dataset.annotate = annotateMode()
        clear()
      }),
      onPointSamples(({ pts, pen }) => {
        for (const [x, y] of pts) {
          if (pen === false) down = false
          else {
            if (down) path.lineTo(x, y)
            else path.moveTo(x, y)
            down = true
          }
          last = [x, y]
        }
        frame ||= requestAnimationFrame(draw)
      }),
      onPointStop(clear),
      onPointCancel(clear),
    ]
    resize()
    addEventListener('resize', resize)
    return () => {
      clear()
      removeEventListener('resize', resize)
      for (const u of offs) void u.then((f) => f())
      delete document.documentElement.dataset.overlay
    }
  }, [])
  return <canvas ref={ref} className="pointer-events-none fixed inset-0 h-dvh w-dvw" aria-hidden />
}
