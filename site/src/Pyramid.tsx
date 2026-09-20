import { useEffect, useRef } from 'react'

/** Base side and height (px, before scale); Giza ratio. */
const S = 520
const H = 330
const HALF = S / 2
/** Slant height and the face tilt from vertical. */
const L = Math.hypot(H, HALF)
const THETA = Math.atan2(HALF, H)

/** Warm key from upper left, faint cold fill from below (view space; -y is up, +z toward the viewer). */
const KEY = norm([-0.6, -0.7, 0.5])
const FILL = norm([0.2, 1, 0.3])
const OCHRE = [1, 0.86, 0.66]
const FAIENCE = [0.36, 0.56, 0.54]

type V = [number, number, number]

function norm([x, y, z]: V): V {
  const n = Math.hypot(x, y, z)
  return [x / n, y / n, z / n]
}

function rotY([x, y, z]: V, a: number): V {
  return [x * Math.cos(a) + z * Math.sin(a), y, -x * Math.sin(a) + z * Math.cos(a)]
}

function rotX([x, y, z]: V, a: number): V {
  return [x, y * Math.cos(a) - z * Math.sin(a), y * Math.sin(a) + z * Math.cos(a)]
}

/** Outward normal of face `i` after the pyramid's own rotation. */
function normal(i: number, ry: number, rx: number): V {
  return rotX(rotY([Math.cos(THETA) * Math.sin((i * Math.PI) / 2), -Math.sin(THETA), Math.cos(THETA) * Math.cos((i * Math.PI) / 2)], ry), rx)
}

function stone(): V {
  const v = getComputedStyle(document.documentElement).getPropertyValue('--stone').trim().split(/\s+/).map(Number)
  return [v[0] ?? 232, v[1] ?? 220, v[2] ?? 200]
}

/** Flat Lambert: ambient + ochre key + faience fill, on the limestone base. */
function shade(n: V, base: V): string {
  const k = Math.max(0, n[0] * KEY[0] + n[1] * KEY[1] + n[2] * KEY[2])
  const f = Math.max(0, n[0] * FILL[0] + n[1] * FILL[1] + n[2] * FILL[2])
  const c = base.map((b, i) => Math.round(b * (0.42 + 0.62 * k * (OCHRE[i] ?? 1)) + 34 * f * (FAIENCE[i] ?? 1)))
  return `rgb(${c[0]} ${c[1]} ${c[2]})`
}

/**
 * A square-base limestone pyramid in CSS 3D, pinned behind the page. Scroll rotates it ~100° around Y and eases the
 * X tilt; motion is inertial (lerp per frame). Static under `prefers-reduced-motion`.
 */
export function Pyramid() {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const pyr = ref.current
    if (!pyr) return
    const faces = Array.from(pyr.children) as HTMLElement[]
    const still = matchMedia('(prefers-reduced-motion: reduce)').matches
    let cur = 0
    let target = 0
    let raf = 0
    let base = stone()

    const paint = (p: number) => {
      const ry = (-28 + p * 100) * (Math.PI / 180)
      const rx = (12 - p * 7) * (Math.PI / 180)
      const k = Math.min(1, Math.max(0.45, Math.min(innerWidth, innerHeight) / 900))
      pyr.style.transform = `scale(${k}) rotateX(${rx}rad) rotateY(${ry}rad)`
      // Past the hero the pyramid sits under copy: recede so bone type stays readable over it.
      pyr.style.opacity = String(1 - 0.65 * Math.min(1, p / 0.15))
      faces.forEach((f, i) => {
        f.style.backgroundColor = shade(normal(i, ry, rx), base)
      })
    }
    const progress = () => scrollY / Math.max(1, document.documentElement.scrollHeight - innerHeight)
    const tick = () => {
      cur += (target - cur) * 0.07
      paint(cur)
      raf = Math.abs(target - cur) > 0.0005 ? requestAnimationFrame(tick) : 0
    }
    const onScroll = () => {
      target = progress()
      if (still) paint(target)
      else if (!raf) raf = requestAnimationFrame(tick)
    }
    const onScheme = () => {
      base = stone()
      paint(cur)
    }
    cur = target = progress()
    paint(cur)
    addEventListener('scroll', onScroll, { passive: true })
    addEventListener('resize', onScroll)
    const scheme = matchMedia('(prefers-color-scheme: light)')
    scheme.addEventListener('change', onScheme)
    return () => {
      removeEventListener('scroll', onScroll)
      removeEventListener('resize', onScroll)
      scheme.removeEventListener('change', onScheme)
      cancelAnimationFrame(raf)
    }
  }, [])

  return (
    <div className="stage" aria-hidden>
      <div className="absolute inset-0 translate-x-[22vw] translate-y-[6vh] opacity-25 max-md:translate-x-0 max-md:translate-y-[-4vh] md:opacity-100">
        <div ref={ref} className="pyr">
          {[0, 1, 2, 3].map((i) => (
            <div
              key={i}
              className="face"
              style={{
                width: S,
                height: L,
                left: -HALF,
                top: -L,
                transform: `rotateY(${i * 90}deg) translateZ(${HALF}px) translateY(${H / 2}px) rotateX(${THETA}rad)`,
              }}
            />
          ))}
        </div>
      </div>
    </div>
  )
}
