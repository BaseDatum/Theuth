// Pencil over an entered live page (pointer-ink path). A sandboxed, opaque-origin frame receives the pen's events
// itself; the host cannot filter them. So the page relays them: while the host has switched capture on (the card is
// entered, the pen inks or erases, native ink is not drawing), the relay script takes `pointerType === 'pen'` events —
// never touch or mouse, so fingers keep scrolling and tapping the page — and posts their points to the host, which
// draws and commits them like any other stroke. Only whiteboard page cards carry the relay (`withPenRelay`, applied
// after `injectBootstrap`); chat pages are unchanged.

/** Host → page: `{ on: boolean }`. */
export const PEN_CAPTURE = 'theuth/pen-capture'
/** Page → host: `{ phase, pts: [x, y, pressure, t][] }` in the frame's CSS px and the page's event clock (ms). */
export const PEN = 'theuth/pen'

export type PenPhase = 'down' | 'move' | 'up' | 'cancel'
export type PenMessage = { phase: PenPhase; pts: [x: number, y: number, pressure: number, t: number][] }

/** Points accepted in one message (a frame's coalesced events are a handful; this bounds a hostile page). */
export const PEN_MAX_PTS = 256

/**
 * The page's message, validated: the page is untrusted (design §6.5). Points outside the frame's viewport (plus a
 * margin) or non-finite are refused as a whole.
 */
export function readPen(data: unknown, viewport: { w: number; h: number }): PenMessage | null {
  if (!data || typeof data !== 'object') return null
  const d = data as { jsonrpc?: unknown; method?: unknown; params?: unknown }
  if (d.jsonrpc !== '2.0' || d.method !== PEN || !d.params || typeof d.params !== 'object') return null
  const p = d.params as { phase?: unknown; pts?: unknown }
  if (p.phase !== 'down' && p.phase !== 'move' && p.phase !== 'up' && p.phase !== 'cancel') return null
  if (!Array.isArray(p.pts) || p.pts.length === 0 || p.pts.length > PEN_MAX_PTS) return null
  const m = 64
  const pts: PenMessage['pts'] = []
  for (const q of p.pts) {
    if (!Array.isArray(q) || q.length !== 4 || !q.every((n) => typeof n === 'number' && Number.isFinite(n))) return null
    const [x, y, pr, t] = q as number[]
    if (x! < -m || y! < -m || x! > viewport.w + m || y! > viewport.h + m) return null
    pts.push([x!, y!, Math.min(1, Math.max(0, pr!)), t!])
  }
  return { phase: p.phase, pts }
}

/** The page-side relay script (inserted into whiteboard page cards by `withPenRelay`). Self-contained, no globals. */
export const PEN_RELAY_JS = `(() => {
  let on = false, id = null
  const post = (phase, evs) => parent.postMessage({ jsonrpc: '2.0', method: '${PEN}', params: { phase, pts: evs.map((e) => [e.clientX, e.clientY, e.pressure, e.timeStamp]) } }, '*')
  addEventListener('message', (e) => { const d = e.data; if (e.source === parent && d && d.method === '${PEN_CAPTURE}') { on = !!(d.params && d.params.on); if (!on) id = null } })
  const stop = (e) => { e.preventDefault(); e.stopImmediatePropagation() }
  addEventListener('pointerdown', (e) => {
    if (!on || e.pointerType !== 'pen' || id !== null) return
    stop(e); id = e.pointerId
    try { e.target.setPointerCapture(id) } catch {}
    post('down', [e])
  }, true)
  addEventListener('pointermove', (e) => {
    if (e.pointerId !== id) return
    stop(e)
    const evs = e.getCoalescedEvents ? e.getCoalescedEvents() : []
    for (let i = 0; i < evs.length; i += 200) post('move', evs.slice(i, i + 200))
    if (!evs.length) post('move', [e])
  }, true)
  const end = (phase) => (e) => { if (e.pointerId !== id) return; stop(e); id = null; post(phase, [e]) }
  addEventListener('pointerup', end('up'), true)
  addEventListener('pointercancel', end('cancel'), true)
  addEventListener('click', (e) => { if (on && e.pointerType === 'pen') stop(e) }, true)
  const stylus = (e) => on && Array.from(e.changedTouches).some((t) => t.touchType === 'stylus')
  addEventListener('touchstart', (e) => { if (stylus(e)) e.preventDefault() }, { capture: true, passive: false })
  addEventListener('touchmove', (e) => { if (id !== null) e.preventDefault() }, { capture: true, passive: false })
})();`

/** Adds the relay script to a document `injectBootstrap` produced, just before its theme block (head start). */
export function withPenRelay(injected: string): string {
  const at = injected.indexOf('<style id="theuth-theme">')
  return at < 0 ? injected : `${injected.slice(0, at)}<script>${PEN_RELAY_JS}</script>${injected.slice(at)}`
}
