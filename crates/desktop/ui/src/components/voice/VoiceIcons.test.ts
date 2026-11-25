import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { FLAT_LINE, MicGlyph, TalkGlyph, TalkLiveGlyph, livePath } from '@/components/voice/VoiceIcons'

const PEN = 'style="stroke:var(--project-carnelian)"'

describe('static glyphs', () => {
  it('MicGlyph: capsule, stand, and the carnelian bar on the theme token', () => {
    const html = renderToStaticMarkup(h(MicGlyph, { className: 'size-5' }))
    expect(html).toContain('class="size-5"')
    expect(html).toContain('viewBox="0 0 24 24"')
    expect(html).toContain('<rect x="9" y="3" width="6" height="11" rx="3"></rect>')
    expect(html).toContain('<path d="M5 11a7 7 0 0 0 14 0M12 18v3"></path>')
    expect(html).toContain(`<path d="M6.5 8.5h11" ${PEN}></path>`)
  })

  it('TalkGlyph: the ring in currentColor, the wave in carnelian', () => {
    const html = renderToStaticMarkup(h(TalkGlyph, { className: 'size-5' }))
    expect(html).toContain('stroke="currentColor"')
    expect(html).toContain('<circle cx="12" cy="12" r="9"></circle>')
    expect(html).toContain(`<path d="M5.5 12c1.6-3.2 3.2-3.2 4.3 0s2.7 3.2 4.3 0 3-3.2 4.4 0" ${PEN}></path>`)
  })
})

describe('livePath', () => {
  it('meets the centre at both ends and never swings past ±3.2', () => {
    const d = livePath(new Float32Array(25).fill(40))
    const pts = [...d.matchAll(/[ML]([\d.]+) ([\d.]+)/g)].map((m) => [Number(m[1]), Number(m[2])] as const)
    expect(pts).toHaveLength(25)
    expect(pts[0]).toEqual([6, 12])
    expect(pts[24]).toEqual([18, 12])
    expect(Math.max(...pts.map(([, y]) => y))).toBeCloseTo(15.2)
  })
})

describe('TalkLiveGlyph', () => {
  let cleanup = async () => {}
  afterEach(async () => {
    await cleanup()
    vi.unstubAllGlobals()
    window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) as unknown as MediaQueryList
  })

  async function mount() {
    const frames = new Map<number, FrameRequestCallback>()
    let next = 1
    const raf = vi.fn((cb: FrameRequestCallback) => (frames.set(next, cb), next++))
    const caf = vi.fn((id: number) => void frames.delete(id))
    vi.stubGlobal('requestAnimationFrame', raf)
    vi.stubGlobal('cancelAnimationFrame', caf)
    const host = document.body.appendChild(document.createElement('div'))
    const root = createRoot(host)
    await act(async () => root.render(h(TalkLiveGlyph, { className: 'size-5', source: null, active: false })))
    const unmount = async () => {
      await act(async () => root.unmount())
      host.remove()
    }
    cleanup = unmount
    const run = (t: number) => {
      const due = [...frames.entries()]
      frames.clear()
      for (const [, cb] of due) cb(t)
    }
    return { host, raf, caf, frames, run, unmount, line: () => host.querySelectorAll('path')[0]!.getAttribute('d') }
  }

  it('draws the idle ripple on one rAF loop and stops it on unmount', async () => {
    const g = await mount()
    expect(g.host.querySelector('circle')?.getAttribute('r')).toBe('9')
    expect((g.host.querySelector('path') as SVGPathElement | null)?.style.stroke).toBe('var(--project-carnelian)')
    expect(g.raf).toHaveBeenCalledTimes(1)
    for (let t = 16; t < 16 * 40; t += 16) g.run(t)
    expect(g.line()).not.toBe(FLAT_LINE)
    expect(g.frames.size).toBe(1)
    await g.unmount()
    expect(g.caf).toHaveBeenCalled()
    expect(g.frames.size).toBe(0)
  })

  it('is a still straight line under reduced motion, with no frames requested', async () => {
    window.matchMedia = () => ({ matches: true, addEventListener() {}, removeEventListener() {} }) as unknown as MediaQueryList
    const g = await mount()
    expect(g.raf).not.toHaveBeenCalled()
    expect(g.line()).toBe(FLAT_LINE)
  })
})
