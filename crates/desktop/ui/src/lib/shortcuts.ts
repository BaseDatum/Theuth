// Settings › Shortcuts: every binding the app listens for, user-rebindable. `localStorage` + zustand like
// `meeting-settings.ts`; chord grammar and matching in `chord.ts`.

import { register, unregister } from '@tauri-apps/plugin-global-shortcut'
import { create } from 'zustand'
import { isApple } from '@/lib/chord'
import { container } from '@/lib/platform'

const KEY = 'shortcuts'

export type ShortcutId =
  | 'new-chat'
  | 'search'
  | 'toggle-sidebar'
  | 'focus-composer'
  | 'copy-last-response'
  | 'copy-last-code'
  | 'shortcuts'
  | 'dictate'
  | 'voice-key'
  | 'send'
  | 'interject'
  | 'newline'
  | 'withdraw'
  | 'stop'
  | 'toggle-window'
  | 'cancel-all'
  | 'talk'
  | 'talk-transcript'

/** `window`: one keydown listener (`useHotkeys`); `composer`: the chat box's own keydown; `global`: a Tauri global shortcut (macOS). */
export type Scope = 'window' | 'composer' | 'global'

export type Def = {
  label: string
  /** `Mod` = ⌘ on Apple platforms, Ctrl elsewhere; resolved by `defaults()`. */
  chord: string
  scope: Scope
  /** Window scope: also fires while an input has focus. */
  inEditable?: boolean
}

export const DEFS: Record<ShortcutId, Def> = {
  'new-chat': { label: 'New chat', chord: 'Mod+Shift+O', scope: 'window' },
  search: { label: 'Search', chord: 'Mod+K', scope: 'window', inEditable: true },
  'toggle-sidebar': { label: 'Toggle sidebar', chord: 'Mod+Shift+S', scope: 'window' },
  'focus-composer': { label: 'Focus composer', chord: 'Shift+Escape', scope: 'window', inEditable: true },
  'copy-last-response': { label: 'Copy last response', chord: 'Mod+Shift+C', scope: 'window' },
  'copy-last-code': { label: 'Copy last code block', chord: 'Mod+Shift+Semicolon', scope: 'window' },
  shortcuts: { label: 'Show shortcuts', chord: 'Mod+Slash', scope: 'window' },
  dictate: { label: 'Dictate', chord: 'Control+D', scope: 'window', inEditable: true },
  // 96 §0.1: registered by the Rust shell (`voice_key_bind`), not by `App.tsx`; adjacent to `dictate` on purpose.
  // 149 D16: ⌃S on macOS; Ctrl+Alt+Space elsewhere (the companions' §8 table mirrors it); a persisted override wins.
  // `container() === 'macos'` is `hotkeys.ts`'s `isMacDesktop()`, inlined: hotkeys.ts imports this module (cycle at init).
  'voice-key': { label: 'Ask theuth about the screen', chord: container() === 'macos' ? 'Control+S' : 'Control+Alt+Space', scope: 'global' },
  talk: { label: 'Stop the talk', chord: 'Mod+T', scope: 'window', inEditable: true },
  'talk-transcript': { label: 'Show or hide the talk transcript', chord: 'Mod+Shift+T', scope: 'window', inEditable: true },
  send: { label: 'Send', chord: 'Enter', scope: 'composer' },
  interject: { label: 'Send and interrupt the running turn', chord: 'Mod+Enter', scope: 'composer' },
  newline: { label: 'New line', chord: 'Shift+Enter', scope: 'composer' },
  withdraw: { label: 'Withdraw last queued message', chord: 'Alt+ArrowUp', scope: 'composer' },
  stop: { label: 'Stop the running turn', chord: 'Escape', scope: 'composer' },
  'toggle-window': { label: 'Show or hide the window', chord: 'Alt+Space', scope: 'global' },
  'cancel-all': { label: 'Cancel every running session', chord: 'Alt+Escape', scope: 'global' },
}

export const IDS = Object.keys(DEFS) as ShortcutId[]

/** The default chord for `id` on this platform. */
export const fallback = (id: ShortcutId): string => DEFS[id].chord.replace('Mod', isApple() ? 'Super' : 'Control')

type Store = {
  chords: Record<ShortcutId, string>
  set(id: ShortcutId, chord: string): void
}

function load(): Record<ShortcutId, string> {
  const base = Object.fromEntries(IDS.map((id) => [id, fallback(id)])) as Record<ShortcutId, string>
  try {
    const saved = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Record<ShortcutId, string>>
    for (const id of IDS) if (typeof saved[id] === 'string') base[id] = saved[id]
  } catch {}
  return base
}

export const useShortcuts = create<Store>()((set, get) => ({
  chords: load(),
  set: (id, chord) => {
    const chords = { ...get().chords, [id]: chord }
    // Only overrides are persisted, so a default that changes in a release reaches everyone who kept it.
    const overrides = Object.fromEntries(IDS.filter((i) => chords[i] !== fallback(i)).map((i) => [i, chords[i]]))
    localStorage.setItem(KEY, JSON.stringify(overrides))
    set({ chords })
  },
}))

/** Can this chord be registered right now? A conflict (another app, or our own live binding) throws. */
export async function probeShortcut(chord: string): Promise<void> {
  await register(chord, () => {})
  await unregister(chord)
}
