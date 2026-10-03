// 89 VOICE-HOLD: the voice-note gesture's haptics. Under Tauri on a phone → `@tauri-apps/plugin-haptics` (Taptic /
// Android effects), loaded lazily so a desktop build never touches it; a browser with `navigator.vibrate` (Android
// Chrome) → short patterns; everywhere else → nothing. Fire and forget: never throws, never awaited in the gesture.

import type * as Plugin from '@tauri-apps/plugin-haptics'
import { touch } from '@/lib/platform'
import { isTauri } from '@/lib/tauri'

let plugin: Promise<typeof Plugin | null> | undefined

const fire = (pattern: number | number[], native: (p: typeof Plugin) => Promise<unknown>) => {
  try {
    if (typeof window !== 'undefined' && isTauri() && touch()) {
      // Dynamic on purpose: the plugin's native side exists only in the iOS/Android shell; a desktop build never loads it.
      plugin ??= import('@tauri-apps/plugin-haptics').catch(() => null)
      void plugin.then((p) => p && native(p)).catch(() => {})
    } else navigator.vibrate?.(pattern)
  } catch {
    // No haptics here; the gesture goes on.
  }
}

export const haptics = {
  /** The press, and the orb appearing. */
  tick: () => fire(10, (p) => p.impactFeedback('light')),
  /** Slid up past `LOCK_PX`. */
  lock: () => fire(20, (p) => p.impactFeedback('medium')),
  /** Slid left past `CANCEL_PX`. */
  cancel: () => fire([10, 40, 10], (p) => p.notificationFeedback('warning')),
  /** Released: the note goes. */
  send: () => fire(10, (p) => p.impactFeedback('light')),
}
