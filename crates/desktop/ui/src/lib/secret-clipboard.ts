// 221 §9.1 / §12.5: a secret value or Send link copied to the clipboard is cleared `CLIPBOARD_MS` later, but only while
// it is still what the clipboard holds: anything the user copied since stays. macOS asks the shell for the pasteboard's
// change count (`pasteboard_change_count` / `pasteboard_clear_if`, the value never leaves the webview); elsewhere the
// text is read back and compared. iOS is left alone: reading the clipboard there shows the system's Paste prompt.

import { invoke } from '@tauri-apps/api/core'
import { container } from './platform'

/** How long a copied secret stays on the clipboard. */
export const CLIPBOARD_MS = 60_000

/** The pending clear; a newer copy replaces it (the older value is already off the clipboard). */
let pending: ReturnType<typeof setTimeout> | undefined

/** Write `text` to the clipboard and clear it after `ms` unless something else was copied meanwhile. */
export async function copySecret(text: string, ms: number = CLIPBOARD_MS): Promise<void> {
  await navigator.clipboard.writeText(text)
  clearTimeout(pending)
  const where = container()
  const count = where === 'macos' ? await invoke<number | null>('pasteboard_change_count').catch(() => null) : null
  if (count !== null) {
    pending = setTimeout(() => void invoke('pasteboard_clear_if', { count }).catch(() => undefined), ms)
    return
  }
  if (where === 'ios') return
  pending = setTimeout(() => void clearIfStill(text), ms)
}

async function clearIfStill(text: string): Promise<void> {
  const now = await navigator.clipboard.readText().catch(() => null)
  if (now === text) await navigator.clipboard.writeText('').catch(() => undefined)
}

/** Drop the pending clear (tests). */
export function cancelSecretClear(): void {
  clearTimeout(pending)
  pending = undefined
}
