import { useEffect, useRef } from 'react'

const W = 96
const H = 36
const BARS = 24
/** Settle to the flat line this long after the last packet (54 §7). */
const IDLE_MS = 400

/**
 * Dictation level meter (54 §7): the last 24 mic levels (20 ms each, newest at the right) as mirrored bars,
 * 3 px wide with a 1 px gap, height `4 + 28·log10(1 + 9·level)`, accent colour fading toward the left.
 * Packets land ~6×/s in bursts of 8; a rAF loop eases the drawn bars toward the latest packet at 60 fps and
 * relaxes to a 4 px line 400 ms after the last one (or whenever `active` is false).
 */
export function LevelMeter({ levels, active }: { levels: number[]; active: boolean }) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const target = useRef(new Float32Array(BARS))
  const shown = useRef(new Float32Array(BARS))
  const lastPacket = useRef(0)
  const activeRef = useRef(active)
  activeRef.current = active

  useEffect(() => {
    const t = target.current
    t.fill(0)
    const tail = levels.slice(-BARS)
    t.set(tail, BARS - tail.length)
    lastPacket.current = performance.now()
  }, [levels])

  useEffect(() => {
    const el = canvas.current
    const ctx = el?.getContext('2d')
    if (!el || !ctx) return
    const dpr = window.devicePixelRatio || 1
    el.width = W * dpr
    el.height = H * dpr
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    const style = getComputedStyle(el)
    ctx.fillStyle = style.getPropertyValue('--fg-accent').trim() || style.color
    let raf = 0
    const draw = (now: number) => {
      const idle = !activeRef.current || now - lastPacket.current > IDLE_MS
      const t = target.current
      const s = shown.current
      ctx.clearRect(0, 0, W, H)
      for (let i = 0; i < BARS; i++) {
        const goal = idle ? 0 : (t[i] ?? 0)
        const v = (s[i] ?? 0) + (goal - (s[i] ?? 0)) * 0.3
        s[i] = v
        const h = 4 + 28 * Math.min(1, Math.max(0, Math.log10(1 + 9 * v)))
        ctx.globalAlpha = 0.25 + (0.75 * (i + 1)) / BARS
        ctx.beginPath()
        ctx.roundRect(i * 4, H / 2 - h / 2, 3, h, 1.5)
        ctx.fill()
      }
      raf = requestAnimationFrame(draw)
    }
    raf = requestAnimationFrame(draw)
    return () => cancelAnimationFrame(raf)
  }, [])

  return <canvas ref={canvas} className="text-fg-accent" style={{ width: W, height: H }} aria-hidden />
}
