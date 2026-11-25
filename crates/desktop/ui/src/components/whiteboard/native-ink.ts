// 218 §6 / §8.8 "Native ink bridge": between `tauri-plugin-ink` (Pencil capture on iPad, `lib/ink-native.ts`) and the
// canvas. Strokes arrive in batches at pen-idle; each is converted with the viewport it was drawn under, committed once
// (de-duplicated by `seq`: the plugin resends until acked), and the whole batch is acked after the commit painted.
// The store drives the plugin the other way: tool, enabled, picker visibility, and the viewport at each view-change end.

import { type InkEvent, type NativeInk, type NativeStroke, type StrokeBatch, attachNativeInk } from '@/lib/ink-native'
import type { CapturedStroke, View, WhiteboardCanvasHandle } from './canvas/api'
import type { Id, InkColour } from './canvas/model'
import type { WbStore } from './canvas/store'

/** §8.6 named ink colours, light / dark. */
export const PALETTE: Record<Exclude<InkColour, `#${string}`>, [light: string, dark: string]> = {
  ink: ['#1a1612', '#e8dcc8'],
  red: ['#d9480f', '#ff7a45'],
  blue: ['#1f6feb', '#6ea8ff'],
  green: ['#2f9e44', '#5fd27a'],
  amber: ['#b7791f', '#f2c14e'],
  violet: ['#7048e8', '#a78bfa'],
}
/** RGB distance under which a picked colour is taken as the named token (so it flips with the theme). */
const NEAR = 60

const rgb = (hex: string): [number, number, number] | null => {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim())
  if (!m?.[1]) return null
  const n = Number.parseInt(m[1], 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

/** A picked `#rrggbb` → the nearest named token (either theme's shade), else the hex itself. */
export function nearestColour(hex: string): InkColour {
  const c = rgb(hex)
  if (!c) return 'ink'
  let best: InkColour | null = null
  let dist = Number.POSITIVE_INFINITY
  for (const [name, shades] of Object.entries(PALETTE) as [keyof typeof PALETTE, [string, string]][]) {
    for (const s of shades) {
      const p = rgb(s) as [number, number, number]
      const d = Math.hypot(c[0] - p[0], c[1] - p[1], c[2] - p[2])
      if (d < dist) {
        dist = d
        best = name
      }
    }
  }
  return best && dist <= NEAR ? best : (`#${hex.replace(/^#/, '').toLowerCase()}` as InkColour)
}

/** A token (or hex) → the `#rrggbb` the native picker shows in this theme. */
export function resolveColour(colour: InkColour, dark: boolean): string {
  const named = PALETTE[colour as keyof typeof PALETTE]
  return named ? named[dark ? 1 : 0] : colour
}

/** One native stroke → world coordinates with the viewport it was drawn under (`world = (pt − t) / zoom`). */
export function toCaptured(s: NativeStroke): CapturedStroke {
  const { tx, ty, zoom } = s.view
  const z = zoom > 0 ? zoom : 1
  const pts: CapturedStroke['pts'] = []
  for (let i = 0; i + 3 < s.pts.length; i += 4) {
    const x = s.pts[i] as number
    const y = s.pts[i + 1] as number
    const f = s.pts[i + 2] as number
    const t = s.pts[i + 3] as number
    pts.push([(x - tx) / z, (y - ty) / z, Math.min(1, Math.max(0, f)), t])
  }
  return {
    pts,
    tool: s.tool,
    colour: nearestColour(s.colour),
    width: s.width / z,
    hasPressure: pts.some((p) => p[2] > 0),
    at: s.t0,
  }
}

export type NativeBridge = { onBatch(b: StrokeBatch): Promise<void>; reset(): void }

/**
 * De-duplicate by `seq`, commit the rest in order, then ack every seq of the batch (committed and skipped alike).
 * Batches are serialised: a resend that arrives while the first delivery is committing waits and is then skipped.
 * A failed commit acks nothing, so the plugin resends.
 */
export function createNativeBridge(d: { commit(strokes: CapturedStroke[]): Promise<Id[]>; ack(seqs: number[]): void }): NativeBridge {
  const committed = new Set<number>()
  let chain: Promise<void> = Promise.resolve()
  const run = async (b: StrokeBatch) => {
    const ordered = [...b.strokes].sort((a, c) => a.seq - c.seq)
    const fresh: NativeStroke[] = []
    const seen = new Set<number>()
    for (const s of ordered) {
      if (committed.has(s.seq) || seen.has(s.seq)) continue
      seen.add(s.seq)
      fresh.push(s)
    }
    if (fresh.length) {
      await d.commit(fresh.map(toCaptured))
      for (const s of fresh) committed.add(s.seq)
    }
    d.ack(ordered.map((s) => s.seq))
  }
  return {
    onBatch: (b) => {
      chain = chain.then(() => run(b)).catch(() => {})
      return chain
    },
    reset: () => committed.clear(),
  }
}

export type NativeConnection = {
  /** The canvas's view changed (`phase: 'end'`): tell the plugin, in web-view coordinates. */
  viewEnd(view: View, frame: DOMRect): void
  /** The whiteboard is (not) the visible surface: disables capture while hidden. */
  visible(on: boolean): void
  dispose(): void
}

/**
 * Attach the plugin when `store.inkEngine === 'auto'` and wire both directions. Resolves to null where there is no
 * native ink (desktop, browser, an older iPad): the canvas's pointer ink is then the only path.
 */
export async function connectNativeInk(o: {
  store: WbStore
  handle: () => WhiteboardCanvasHandle | null
  dark: () => boolean
  /** The Pencil asked for the tools (tap/squeeze → a palette action): raise the cluster. */
  showTools(): void
  attach?: typeof attachNativeInk
}): Promise<NativeConnection | null> {
  if (o.store.getState().inkEngine !== 'auto') return null
  let previous: { tool: 'monoline' | 'pen' | 'marker'; erase: boolean } = { tool: o.store.getState().tool, erase: false }
  let attached: NativeInk | null = null
  const bridge = createNativeBridge({
    commit: async (strokes) => {
      const h = o.handle()
      if (!h) throw new Error('no canvas')
      return h.commitCaptured(strokes)
    },
    ack: (seqs) => attached?.ack(seqs),
  })
  const onEvent = (e: InkEvent) => {
    const st = o.store.getState()
    if (e.kind === 'state') {
      st.set({ penDown: e.drawing })
      if (e.drawing) st.set({ clusterOpen: false })
    } else if (e.kind === 'tool') {
      previous = { tool: st.tool, erase: st.mode === 'erase' }
      if (e.tool === 'eraser') st.set({ mode: 'erase' })
      else st.set({ tool: e.tool, colour: nearestColour(e.colour), width: e.width, mode: st.mode === 'erase' ? 'idle' : st.mode })
    } else if (e.kind === 'pencil') {
      if (e.action === 'switchEraser') st.set({ mode: st.mode === 'erase' ? 'idle' : 'erase' })
      else if (e.action === 'switchPrevious') {
        const back = previous
        previous = { tool: st.tool, erase: st.mode === 'erase' }
        st.set({ tool: back.tool, mode: back.erase ? 'erase' : st.mode === 'erase' ? 'idle' : st.mode })
      } else if (e.action === 'showColorPalette' || e.action === 'showInkAttributes' || e.action === 'showContextualPalette') o.showTools()
    }
  }
  attached = await (o.attach ?? attachNativeInk)({ strokes: (b) => void bridge.onBatch(b), event: onEvent })
  const ink = attached
  if (!ink) return null
  let shown = true
  const push = () => {
    const s = o.store.getState()
    const enabled = shown && s.inkEngine === 'auto' && s.mode !== 'link' && s.mode !== 'erase' && !s.composerOpen
    ink.setTool({ tool: s.mode === 'erase' ? 'eraser' : s.tool, colour: resolveColour(s.colour, o.dark()), width: s.width })
    ink.setEnabled(enabled)
    ink.setPickerVisible(shown && !s.composerOpen && s.inkEngine === 'auto')
    if (s.nativeInk !== enabled) s.set({ nativeInk: enabled })
  }
  push()
  const unsub = o.store.subscribe((s, p) => {
    if (s.mode !== p.mode || s.tool !== p.tool || s.colour !== p.colour || s.width !== p.width || s.composerOpen !== p.composerOpen || s.inkEngine !== p.inkEngine) push()
  })
  return {
    viewEnd: (view, frame) => ink.setViewport({ tx: view.x + frame.left, ty: view.y + frame.top, zoom: view.zoom }),
    visible: (on) => {
      shown = on
      push()
    },
    dispose: () => {
      unsub()
      o.store.getState().set({ nativeInk: false })
      void ink.detach()
    },
  }
}
