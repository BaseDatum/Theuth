// 218: the `ink` plugin's reference API — thin typed wrappers of its three commands. The desktop UI calls
// `invoke`/`Channel` directly (`crates/desktop/ui/src/lib/ink-native.ts`); this file documents the contract
// the Swift side (`ios/Sources`) and `src/models.rs` implement.

import { Channel, invoke } from '@tauri-apps/api/core'

export type ListenResult = { ok: boolean }
export type InkEvent =
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

/** Report Pencil double-tap and squeeze on `onEvent`; `ok` is false below iPadOS 17.5, off iPad and on
 * desktop. A second call replaces the channel. */
export async function pencilListen(onEvent: (event: InkEvent) => void): Promise<ListenResult> {
  const events = new Channel<InkEvent>()
  events.onmessage = onEvent
  return call<ListenResult>('pencil_listen', { onEvent: events })
}

/** Stop reporting Pencil gestures. */
export const pencilUnlisten = () => call<null>('pencil_unlisten')

/** On-device handwriting of a base64 PNG (no `data:` prefix); `text` ≤ 200 chars. */
export const recognizeText = (png: string) =>
  call<{ text: string; confidence: number }>('recognize_text', { png })
