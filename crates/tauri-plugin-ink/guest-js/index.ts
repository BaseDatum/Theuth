// 218 §6: the `ink` plugin's reference API — thin typed wrappers of its eight commands. The desktop UI does
// not depend on this file tonight (`crates/desktop/ui/src/lib/ink-native.ts` calls `invoke`/`Channel`
// directly); it documents the contract the Swift side (`ios/Sources`) and `src/models.rs` implement.

import { Channel, invoke } from '@tauri-apps/api/core'

export type AttachResult = {
  native: boolean
  reason?: 'desktop' | 'unsupported_os' | 'not_ipad' | 'route_failed'
  os?: string
  route?: 'A' | 'B'
}
export type NativeView = { tx: number; ty: number; zoom: number }
export type NativeStroke = {
  /** Monotonic per attach, from 1; identity for ack and de-duplication. */
  seq: number
  tool: 'monoline' | 'pen' | 'marker'
  /** `#rrggbb` as picked. */
  colour: string
  /** Points. */
  width: number
  /** Ms since epoch at the first point. */
  t0: number
  /** The viewport captured at stroke begin. */
  view: NativeView
  /** Flat, 4 numbers a point: x, y (web-view points), force 0..1, t (ms since t0). */
  pts: number[]
  /** 0 on first delivery. */
  retry: number
}
export type StrokeBatch = { batch: number; strokes: NativeStroke[] }
export type InkTool = 'monoline' | 'pen' | 'marker' | 'eraser'
export type InkEvent =
  | { kind: 'state'; drawing: boolean }
  | { kind: 'tool'; tool: InkTool; colour: string; width: number }
  | {
      kind: 'pencil'
      gesture: 'tap' | 'squeeze'
      action:
        | 'switchEraser'
        | 'switchPrevious'
        | 'showColorPalette'
        | 'showInkAttributes'
        | 'showContextualPalette'
        | 'runSystemShortcut'
        | 'ignore'
    }
  | { kind: 'error'; message: string }

const call = <T>(cmd: string, args: Record<string, unknown> = {}) => invoke<T>(`plugin:ink|${cmd}`, args)

/** Mount the overlay; idempotent (a second attach replaces the channels and renumbers unacked strokes). */
export async function attach(
  onStrokes: (batch: StrokeBatch) => void,
  onEvent: (event: InkEvent) => void,
): Promise<AttachResult> {
  const strokes = new Channel<StrokeBatch>()
  strokes.onmessage = onStrokes
  const events = new Channel<InkEvent>()
  events.onmessage = onEvent
  return call<AttachResult>('attach', { onStrokes: strokes, onEvent: events })
}

/** Remove the overlay, recogniser and picker; unacknowledged strokes are dropped. */
export const detach = () => call<null>('detach')

/** False in link mode, with the eraser, while a text field is focused or the whiteboard is not visible. */
export const setEnabled = (enabled: boolean) => call<null>('set_enabled', { enabled })

/** `screen = world × zoom + (tx, ty)`; send on attach and on every viewport-change end. */
export const setViewport = (view: NativeView) =>
  call<null>('set_viewport', { tx: view.tx, ty: view.ty, zoom: view.zoom })

/** Select the tool (and the matching picker item); `colour` is `#rrggbb` resolved for the theme. */
export const setTool = (tool: { tool: InkTool; colour: string; width: number }) =>
  call<null>('set_tool', { tool: tool.tool, colour: tool.colour, width: tool.width })

/** Hidden while the composer sheet is open. */
export const setPickerVisible = (visible: boolean) => call<null>('set_picker_visible', { visible })

/** Acknowledge a batch's strokes (committed and skipped alike); they leave the overlay. */
export const ack = (seqs: number[]) => call<null>('ack', { seqs })

/** On-device handwriting of a base64 PNG (no `data:` prefix); `text` ≤ 200 chars. */
export const recognizeText = (png: string) =>
  call<{ text: string; confidence: number }>('recognize_text', { png })
