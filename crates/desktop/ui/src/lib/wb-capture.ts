// 218 §7: native capture of a rectangle of the main webview (`wb_capture`, WKWebView takeSnapshot). Returns base64 PNG.
// `null` outside Tauri or when the command rejects (non-Apple target, empty rect, not built) — callers fall back to
// the canvas's own composition.

import { invoke } from '@tauri-apps/api/core'
import { isTauri } from '@/lib/tauri'

export type WbCaptured = { png: string; width: number; height: number; scale: number }

export async function wbCapture(rect: { x: number; y: number; width: number; height: number }, width?: number): Promise<WbCaptured | null> {
  if (typeof window === 'undefined' || !isTauri()) return null
  if (!(rect.width > 0 && rect.height > 0)) return null
  try {
    const r = await invoke<WbCaptured>('wb_capture', { rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height }, width })
    return r && typeof r.png === 'string' && r.png.length > 0 ? r : null
  } catch {
    return null
  }
}

/** base64 (no `data:` prefix) → Blob. */
export function base64Blob(b64: string, mime = 'image/png'): Blob {
  const bin = atob(b64)
  const bytes = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
  return new Blob([bytes], { type: mime })
}
