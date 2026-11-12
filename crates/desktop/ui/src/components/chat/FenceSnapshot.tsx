// Design 164 D3/D4: one fence drawn off-screen for a channel picture, imported lazily by `lib/fence-snapshot.ts`.
// The fence mounts with the chat's own renderer for its language (MessageAssistant's streamdown table) in a frame that
// declares the requested appearance's theme variables, `width` CSS px wide, laid out but invisible (opacity 0 off-screen:
// never `display:none`, which stops layout, and not `visibility:hidden`, which every descendant would inherit into the
// picture). No `requestAnimationFrame` / `ResizeObserver` on this path: a hidden window pauses both. Settled = no
// `data-snapshot-pending` placeholder and the DOM quiet for `QUIET_MS`. A lone diagram svg goes out through
// `standalone()` + `rasterize()` like the frame's Copy PNG; anything else as DOM → SVG `<foreignObject>` → canvas.

import { createRoot } from 'react-dom/client'
import { rasterize, standalone } from '@/lib/diagram-export'
import { fitPng, frameFor, type Outcome, settleAt } from '@/lib/fence-snapshot'
import type { Appearance } from '@/lib/page-theme'
import { SnapshotContext, type SnapshotMode } from '@/lib/snapshot-mode'
import type { FenceSnapshotRequested } from "@/rpc/bindings/event.artifact_2efence_2dsnapshot_2drequested.output"
import { plugins } from './MessageAssistant'

/** Side padding of an HTML picture (the svg path is the diagram alone). */
const PAD_X = 16
/** Longest wait for `document.fonts.ready`: a font load that never finishes must not hold both render slots. */
const FONTS_MS = 1000

/** `document.fonts.ready`, or `FONTS_MS` later, whichever comes first. */
function fontsReady(): Promise<unknown> {
  const { promise, resolve } = Promise.withResolvers<unknown>()
  setTimeout(resolve, FONTS_MS)
  return Promise.race([document.fonts.ready, promise])
}

/** Every rule of the document's stylesheets, unconditional layers included (cross-origin sheets are skipped). */
function* rules(list: Iterable<CSSRule> = [...document.styleSheets].flatMap((s) => { try { return [...s.cssRules] } catch { return [] } })): Generator<CSSRule> {
  for (const r of list) {
    if (r instanceof CSSLayerBlockRule) yield* rules(r.cssRules)
    else yield r
  }
}

/**
 * `appearance`'s theme variables as the stylesheet declares them: every custom property of a rule whose selector list
 * names `:root` (Tailwind's `:root, :host` theme layer too — `--color-*` aliases resolve there), `.dark`'s on top.
 */
export function themeVariables(appearance: Appearance): Record<string, string> {
  const out: Record<string, string> = {}
  for (const pass of appearance === 'dark' ? [':root', '.dark'] : [':root']) {
    for (const r of rules()) {
      if (!(r instanceof CSSStyleRule) || !r.selectorText.split(',').some((s) => s.trim() === pass)) continue
      for (const name of r.style) if (name.startsWith('--') || name === 'color-scheme') out[name] = r.style.getPropertyValue(name)
    }
  }
  return out
}

/** Whitespace-free text, to compare what a subtree shows. */
const text = (el: Element) => (el.textContent ?? '').replace(/\s+/g, '')

/** The one diagram `<svg>` that is all the content shows (besides its card's label), or `null`. */
function soleDiagram(content: HTMLElement): SVGSVGElement | null {
  const surfaces = content.querySelectorAll('[role="application"]')
  const svg = surfaces.length === 1 ? surfaces[0]!.querySelector('svg') : null
  const card = surfaces[0]?.closest('[data-streamdown$="-block"]')
  if (!svg || !card) return null
  const label = card.firstElementChild ? text(card.firstElementChild) : ''
  return text(content) === label + text(svg) && content.querySelectorAll('svg').length === card.querySelectorAll('svg').length ? svg : null
}

/**
 * `cs` as declarations, every standard property: a diff against defaults is wrong whenever a skipped value only held
 * together with another (a `0px` border width is the default only while the style is `none`).
 */
function declarations(cs: CSSStyleDeclaration): string {
  let css = ''
  for (let i = 0; i < cs.length; i++) if (!cs[i]!.startsWith('--')) css += `${cs[i]}:${cs.getPropertyValue(cs[i]!)};`
  return css
}

/** `src` deep-cloned with its computed styles inline, pseudo elements as rules, form state and canvases kept. */
function inlined(src: HTMLElement, background: string): { node: HTMLElement; css: string; families: Set<string> } {
  const families = new Set<string>()
  let css = ''
  let n = 0
  const walk = (el: Element, top: boolean): Node => {
    if (el instanceof HTMLCanvasElement) {
      const img = document.createElement('img')
      img.src = el.toDataURL()
      img.setAttribute('style', `width:${el.clientWidth}px;height:${el.clientHeight}px`)
      return img
    }
    const out = el.cloneNode(false) as Element
    const cs = getComputedStyle(el)
    // A picture cannot scroll: no scrollbars (the clipped overflow stays clipped, as in the chat at this width).
    let style = `${declarations(cs)}scrollbar-width:none;`
    // Custom properties a subtree sets for itself (`style={{ '--c': … }}`) still feed `var()` in svg attributes.
    if (el instanceof HTMLElement || el instanceof SVGElement) {
      for (const p of el.style) if (p.startsWith('--')) style += `${p}:${el.style.getPropertyValue(p)};`
    }
    if (top) {
      for (let i = 0; i < cs.length; i++) if (cs[i]!.startsWith('--')) style += `${cs[i]}:${cs.getPropertyValue(cs[i]!)};`
      style += `background-color:${background};margin:0;`
    }
    out.setAttribute('style', style)
    for (const f of cs.fontFamily.split(',')) families.add(f.trim().replace(/^["']|["']$/g, '').toLowerCase())
    for (const pseudo of ['::before', '::after'] as const) {
      const ps = getComputedStyle(el, pseudo)
      if (ps.content === 'none' || ps.content === 'normal') continue
      const cls = `snapshot-p${n++}`
      out.setAttribute('class', `${out.getAttribute('class') ?? ''} ${cls}`)
      let body = ''
      for (let i = 0; i < ps.length; i++) body += `${ps[i]}:${ps.getPropertyValue(ps[i]!)};`
      css += `.${cls}${pseudo}{${body}}`
    }
    if (el instanceof HTMLInputElement) {
      out.setAttribute('value', el.value)
      if (el.checked) out.setAttribute('checked', '')
    } else if (el instanceof HTMLTextAreaElement) out.textContent = el.value
    else if (el instanceof HTMLImageElement && el.complete && el.naturalWidth && !el.src.startsWith('data:')) {
      // A picture loads nothing external: inline the decoded bitmap when the origin allows reading it back.
      try {
        const c = document.createElement('canvas')
        c.width = el.naturalWidth
        c.height = el.naturalHeight
        c.getContext('2d')?.drawImage(el, 0, 0)
        out.setAttribute('src', c.toDataURL())
        out.removeAttribute('srcset')
      } catch {
        // Tainted: the image stays as it is (blank in the picture).
      }
    }
    if (!(el instanceof HTMLTextAreaElement)) {
      for (const child of el.childNodes) {
        if (child instanceof Element) {
          if (child.localName !== 'script') out.appendChild(walk(child, false))
        } else if (child.nodeType === Node.TEXT_NODE) out.appendChild(child.cloneNode())
      }
    }
    return out
  }
  return { node: walk(src, true) as HTMLElement, css, families }
}

/** `blob` as a `data:` URL. */
function dataUrl(blob: Blob): Promise<string> {
  const { promise, resolve, reject } = Promise.withResolvers<string>()
  const r = new FileReader()
  r.onload = () => resolve(String(r.result))
  r.onerror = () => reject(r.error)
  r.readAsDataURL(blob)
  return promise
}

/** The `@font-face` rules of `families` with every `url()` embedded: an svg image loads nothing external. */
async function fontFaces(families: Set<string>): Promise<string> {
  const out: string[] = []
  for (const r of rules()) {
    if (!(r instanceof CSSFontFaceRule)) continue
    const family = r.style.getPropertyValue('font-family').trim().replace(/^["']|["']$/g, '').toLowerCase()
    if (!families.has(family)) continue
    let rule = r.cssText
    for (const m of rule.matchAll(/url\(\s*["']?([^"')]+)["']?\s*\)/g)) {
      if (m[1]!.startsWith('data:')) continue
      const res = await fetch(new URL(m[1]!, r.parentStyleSheet?.href ?? document.baseURI), { signal: AbortSignal.timeout(3000) })
      if (res.ok) rule = rule.replace(m[0], `url("${await dataUrl(await res.blob())}")`)
    }
    out.push(rule)
  }
  return out.join('\n')
}

/** `content` as a standalone svg: the inlined DOM in a `<foreignObject>` of its laid-out size. */
async function htmlPicture(content: HTMLElement, background: string): Promise<{ svg: string; width: number; height: number }> {
  const box = content.getBoundingClientRect()
  const width = Math.ceil(box.width)
  const height = Math.ceil(box.height)
  const { node, css, families } = inlined(content, background)
  const fonts = await fontFaces(families)
  const xhtml = new XMLSerializer().serializeToString(node)
  const style = `<style>${(fonts + css).replace(/]]>/g, ']]&gt;')}</style>`
  return {
    width,
    height,
    svg: `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">${style}<foreignObject x="0" y="0" width="${width}" height="${height}">${xhtml}</foreignObject></svg>`,
  }
}

/** How much wider than its box the content wants to be: the largest horizontal overflow of it or any descendant. */
function overflowX(content: HTMLElement): number {
  let extra = content.scrollWidth - content.clientWidth
  for (const el of content.querySelectorAll('*')) if (el.clientWidth > 0) extra = Math.max(extra, el.scrollWidth - el.clientWidth)
  return Math.max(0, extra)
}

/**
 * The settled fence as a PNG within D4's caps. A picture cannot scroll: content wider than `requested` (a scrolling
 * Kanban or Table, an svg wider than the column) is drawn at its full width, up to `MAX_FRAME`.
 */
async function capture(frame: HTMLElement, content: HTMLElement, requested: number, background: string, scale: number): Promise<Outcome> {
  const svg = soleDiagram(content)
  let picture: { svg: string; width: number; height: number }
  if (svg) {
    const s = standalone(svg.outerHTML, background)
    const width = Math.min(s.width, frameFor(requested, s.width).width)
    picture = { svg: s.svg, width, height: Math.round((s.height * width) / s.width) }
  } else {
    if (!content.textContent?.trim() && !content.querySelector('svg,img,canvas')) return { reason: 'the fence rendered nothing' }
    // Re-lay wider until nothing overflows (layout is synchronous; a nested scroller may need a second pass).
    let width = requested
    for (let pass = 0; pass < 3; pass++) {
      const next = frameFor(requested, width + overflowX(content)).width
      if (next <= width) break
      width = next
      frame.style.width = `${width}px`
    }
    picture = await htmlPicture(content, background)
  }
  return fitPng(picture.width, picture.height, scale, async (s) => new Uint8Array(await (await rasterize(picture, s)).arrayBuffer()))
}

/** Mount, settle and draw `e`; `finish` gets the outcome once. Returns the cancel (unmount without an outcome). */
export function snapshotFence(e: FenceSnapshotRequested, finish: (o: Outcome) => void): () => void {
  const renderer = plugins.renderers?.find((r) => (Array.isArray(r.language) ? r.language.includes(e.lang) : r.language === e.lang))
  if (!renderer) {
    finish({ reason: `no renderer for ${e.lang}` })
    return () => {}
  }
  const frame = document.createElement('div')
  frame.setAttribute('aria-hidden', 'true')
  frame.setAttribute('data-fence-snapshot', e.id)
  // `dark:` utilities follow the frame, not `<html>` (styles.css's `dark` variant skips `[data-snapshot-light]` subtrees).
  if (e.appearance === 'dark') frame.classList.add('dark')
  else frame.setAttribute('data-snapshot-light', '')
  const vars = Object.entries(themeVariables(e.appearance))
    .map(([k, v]) => `${k}:${v};`)
    .join('')
  frame.style.cssText = `position:fixed;left:-100000px;top:0;z-index:-1;opacity:0;pointer-events:none;width:${e.width}px;${vars}color:var(--foreground);background-color:var(--background);font-family:var(--font-sans);font-size:16px;line-height:24px`
  const content = document.createElement('div')
  content.style.cssText = `display:flow-root;padding:0 ${PAD_X}px`
  frame.append(content)
  document.body.append(frame)

  let over = false
  let timer: number | undefined
  const observer = new MutationObserver(() => schedule(Date.now()))
  const teardown = () => {
    if (over) return
    over = true
    window.clearTimeout(timer)
    observer.disconnect()
    // Unmount after the current task: React must not unmount a root from inside its own commit (`fail` from an effect).
    setTimeout(() => {
      root.unmount()
      frame.remove()
    })
  }
  const end = (o: Outcome) => {
    if (over) return
    teardown()
    finish(o)
  }
  const mode: SnapshotMode = { appearance: e.appearance, width: e.width, root: frame, fail: (reason) => end({ reason }) }
  const root = createRoot(content, { onUncaughtError: (err) => end({ reason: err instanceof Error ? err.message : String(err) }) })
  const ready = () => content.childElementCount > 0 && !content.querySelector('[data-snapshot-pending]')
  const mounted = Date.now()
  let lastChange = mounted
  const schedule = (now: number) => {
    if (over) return
    lastChange = now
    window.clearTimeout(timer)
    timer = window.setTimeout(look, Math.max(0, settleAt(mounted, lastChange, ready()) - Date.now()))
  }
  const look = () => {
    if (over) return
    if (!ready()) return end({ reason: 'the layout did not settle' })
    // A change since the timer was set moved the deadline; `schedule` already re-armed.
    if (Date.now() < settleAt(mounted, lastChange, true)) return
    observer.disconnect()
    fontsReady()
      .then(() => capture(frame, content, e.width, getComputedStyle(frame).getPropertyValue('--background').trim() || '#fff', e.scale))
      .then(end, (err: unknown) => end({ reason: err instanceof Error ? err.message : String(err) }))
  }
  observer.observe(content, { subtree: true, childList: true, attributes: true, characterData: true })
  const Fence = renderer.component
  void fontsReady().then(() => {
    if (over) return
    root.render(
      <SnapshotContext value={mode}>
        <Fence code={e.source} language={e.lang} isIncomplete={false} />
      </SnapshotContext>,
    )
    schedule(Date.now())
  })
  return teardown
}
