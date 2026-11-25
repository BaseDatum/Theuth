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

/** null: not a Tauri iOS shell, the plugin is missing, or `native: false`. Never throws. */
export async function attachNativeInk(on: { strokes(b: StrokeBatch): void; event(e: InkEvent): void }): Promise<NativeInk | null> {
  try {
    if (!loader.tauri()) return null
    const core = await loader.core()
    if (!core) return null
    const { invoke, Channel } = core
    const onStrokes = new Channel<StrokeBatch>()
    onStrokes.onmessage = (b) => { try { on.strokes(b) } catch { /* the plugin resends unacked strokes */ } }
    const onEvent = new Channel<InkEvent>()
    onEvent.onmessage = (e) => { try { on.event(e) } catch { /* an event handler never breaks the channel */ } }
    const result = await invoke<AttachResult>('plugin:ink|attach', { onStrokes, onEvent })
    if (!result?.native) return null
    // Fire and forget: a failed call leaves the overlay as it was; the next state change retries.
    const fire = (cmd: string, args: Record<string, unknown>) => { void invoke(`plugin:ink|${cmd}`, args).catch(() => {}) }
    let detached = false
    return {
      result,
      setEnabled: (enabled) => !detached && fire('set_enabled', { enabled }),
      setViewport: (v) => !detached && fire('set_viewport', { tx: v.tx, ty: v.ty, zoom: v.zoom }),
      setTool: (t) => !detached && fire('set_tool', { tool: t.tool, colour: t.colour, width: t.width }),
      setPickerVisible: (visible) => !detached && fire('set_picker_visible', { visible }),
      ack: (seqs) => !detached && seqs.length > 0 && fire('ack', { seqs }),
      recognizeText: async (png) => {
        try {
          const r = await invoke<{ text: string; confidence: number }>('plugin:ink|recognize_text', { png })
          return r && typeof r.text === 'string' ? r : null
        } catch { return null }
      },
      detach: async () => {
        if (detached) return
        detached = true
        try { await invoke('plugin:ink|detach', {}) } catch { /* gone already */ }
      },
    }
  } catch {
    return null
  }
}
