// 218 §6: the JS side of `tauri-plugin-ink` (native Pencil capture on iPad). Every `plugin:ink|…` invoke lives here so
// tests mock one module. The plugin exists only in the iOS shell; the module is loaded dynamically and every failure is
// swallowed — a missing plugin, a desktop build or a browser all mean "no native ink" (`null`), never an exception.

import type * as TauriCore from '@tauri-apps/api/core'
import { isTauri } from '@/lib/tauri'

export type AttachResult = { native: boolean; reason?: 'desktop' | 'unsupported_os' | 'not_ipad' | 'route_failed'; os?: string; route?: 'A' | 'B' }
export type NativeView = { tx: number; ty: number; zoom: number }
export type NativeStroke = {
  seq: number                     // monotonic per attach, from 1; identity for ack and de-duplication
  tool: 'monoline' | 'pen' | 'marker'
  colour: string                  // #rrggbb as picked
  width: number                   // points
  t0: number                      // ms since epoch at the first point
  view: NativeView                // the viewport captured at stroke begin
  pts: number[]                   // flat, 4 numbers a point: x, y (web-view points), force 0..1, t (ms since t0)
  retry: number                   // 0 on first delivery
}
export type StrokeBatch = { batch: number; strokes: NativeStroke[] }
export type InkEvent =
  | { kind: 'state'; drawing: boolean }                                   // pen down / up (palm rule, hide the cluster)
  | { kind: 'tool'; tool: 'monoline' | 'pen' | 'marker' | 'eraser'; colour: string; width: number }   // picker changed
  | { kind: 'pencil'; gesture: 'tap' | 'squeeze'; action: 'switchEraser' | 'switchPrevious' | 'showColorPalette' | 'showInkAttributes' | 'showContextualPalette' | 'runSystemShortcut' | 'ignore' }
  | { kind: 'error'; message: string }

export type NativeTool = { tool: 'monoline' | 'pen' | 'marker' | 'eraser'; colour: string; width: number }
export type NativeInk = {
  result: AttachResult
  setEnabled(enabled: boolean): void; setViewport(v: NativeView): void
  setTool(t: NativeTool): void
  setPickerVisible(visible: boolean): void; ack(seqs: number[]): void
  recognizeText(png: string): Promise<{ text: string; confidence: number } | null>
  detach(): Promise<void>
}
type Core = typeof TauriCore

/** Overridable for tests: the `@tauri-apps/api/core` loader (null = not available). */
export const loader: { core: () => Promise<Core | null>; tauri: () => boolean } = {
  core: () => import('@tauri-apps/api/core').catch(() => null),
  tauri: () => typeof window !== 'undefined' && isTauri(),
}

/**
 * The plugin has one overlay per app; attach and detach are owner-token based (218 review M3). Each `attachNativeInk`
 * takes the next generation synchronously, and every plugin call runs on one promise chain, so a detach queued by an
 * older owner lands before (or is dropped after) a newer attach — never in between. Only the current owner's commands
 * reach the plugin; a superseded owner is told through `on.lost` and its handle goes inert (its `detach` is a no-op).
 */
let generation = 0
let chain: Promise<unknown> = Promise.resolve()
let lostCurrent: (() => void) | null = null
const serial = <T>(f: () => Promise<T>): Promise<T> => {
  const p = chain.then(f, f)
  chain = p.catch(() => {})
  return p
}

/** null: not a Tauri iOS shell, the plugin is missing, `native: false`, or a newer attach superseded this one. Never throws. */
export async function attachNativeInk(on: { strokes(b: StrokeBatch): void; event(e: InkEvent): void; lost?(): void }): Promise<NativeInk | null> {
  const mine = ++generation
  const previous = lostCurrent
  lostCurrent = null
  try { previous?.() } catch { /* the old owner's cleanup never blocks the new one */ }
  const current = () => mine === generation
  try {
    if (!loader.tauri()) return null
    const core = await loader.core()
    if (!core || !current()) return null
    const { invoke, Channel } = core
    const onStrokes = new Channel<StrokeBatch>()
    onStrokes.onmessage = (b) => { if (current()) try { on.strokes(b) } catch { /* the plugin resends unacked strokes */ } }
    const onEvent = new Channel<InkEvent>()
    onEvent.onmessage = (e) => { if (current()) try { on.event(e) } catch { /* an event handler never breaks the channel */ } }
    const result = await serial(() => (current() ? invoke<AttachResult>('plugin:ink|attach', { onStrokes, onEvent }) : Promise.resolve(null)))
    if (!result?.native || !current()) return null
    let detached = false
    const live = () => !detached && current()
    // Fire and forget, in order with attach/detach; a superseded owner's calls are dropped at run time.
    const fire = (cmd: string, args: Record<string, unknown>) => {
      if (!live()) return
      void serial(() => (live() ? invoke(`plugin:ink|${cmd}`, args) : Promise.resolve(null))).catch(() => {})
    }
    lostCurrent = () => {
      detached = true
      on.lost?.()
    }
    return {
      result,
      setEnabled: (enabled) => fire('set_enabled', { enabled }),
      setViewport: (v) => fire('set_viewport', { tx: v.tx, ty: v.ty, zoom: v.zoom }),
      setTool: (t) => fire('set_tool', { tool: t.tool, colour: t.colour, width: t.width }),
      setPickerVisible: (visible) => fire('set_picker_visible', { visible }),
      ack: (seqs) => seqs.length > 0 && fire('ack', { seqs }),
      recognizeText: async (png) => {
        try {
          const r = await invoke<{ text: string; confidence: number }>('plugin:ink|recognize_text', { png })
          return r && typeof r.text === 'string' ? r : null
        } catch { return null }
      },
      detach: async () => {
        if (!live()) return
        detached = true
        lostCurrent = null
        try { await serial(() => (current() ? invoke('plugin:ink|detach', {}) : Promise.resolve(null))) } catch { /* gone already */ }
      },
    }
  } catch {
    return null
  }
}

/** Tests only: forget the current owner. */
export const resetNativeInkOwner = () => {
  generation++
  lostCurrent = null
  chain = Promise.resolve()
}
