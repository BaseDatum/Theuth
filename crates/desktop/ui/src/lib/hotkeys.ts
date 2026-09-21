import { useEffect } from 'react'
import { container } from '@/lib/platform'
import { matches } from '@/lib/chord'
import { DEFS, IDS, type ShortcutId, useShortcuts } from '@/lib/shortcuts'

/** Window `CustomEvent` names the shell dispatches; consumers `addEventListener` on `window`. */
export type Hotkey =
  | 'theuth:new-chat'
  | 'theuth:search'
  | 'theuth:toggle-sidebar'
  | 'theuth:focus-composer'
  | 'theuth:copy-last-response'
  | 'theuth:copy-last-code'
  | 'theuth:shortcuts'
  | 'theuth:dictate'

/** Window-scope shortcuts and the event each dispatches (Settings › Shortcuts owns the chords). */
const EVENTS = {
  'new-chat': 'theuth:new-chat',
  search: 'theuth:search',
  'toggle-sidebar': 'theuth:toggle-sidebar',
  'focus-composer': 'theuth:focus-composer',
  'copy-last-response': 'theuth:copy-last-response',
  'copy-last-code': 'theuth:copy-last-code',
  shortcuts: 'theuth:shortcuts',
  dictate: 'theuth:dictate',
} satisfies Partial<Record<ShortcutId, Hotkey>>

const WINDOW_IDS = IDS.filter((id): id is keyof typeof EVENTS => id in EVENTS)

/** Primary pointer is a finger: no hover, no keyboard chrome. */
export const touch = (): boolean => window.matchMedia('(pointer: coarse)').matches

/** The macOS Tauri window: traffic lights to inset around, a title bar to drag by. False in iOS Tauri and every browser. */
export const isMacDesktop = (): boolean => container() === 'macos'

/**
 * One `keydown` listener for the whole window; mount once in the root shell. Touch devices keep only ⌘K/Ctrl+K
 * (49 §5.1): a hardware keyboard still reaches search, and the physical `code` + modifier match already excludes
 * software keyboards.
 */
export function useHotkeys(): void {
  const chords = useShortcuts((s) => s.chords)
  useEffect(() => {
    const ids = touch() ? WINDOW_IDS.filter((id) => id === 'search') : WINDOW_IDS
    const onKey = (e: KeyboardEvent) => {
      const target = e.target
      const editable =
        target instanceof HTMLElement &&
        (target.isContentEditable ||
          target instanceof HTMLInputElement ||
          target instanceof HTMLTextAreaElement)
      const hit = ids.find((id) => (DEFS[id].inEditable || !editable) && matches(chords[id], e))
      if (!hit) return
      e.preventDefault()
      window.dispatchEvent(new CustomEvent(EVENTS[hit]))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [chords])
}
