// One chord grammar for every binding, `global_hotkey`'s: `Alt+Shift+E`, `Super+K`, `Control+Enter`,
// `Shift+Enter`, `Escape` — modifiers by name, then the physical `KeyboardEvent.code` with `Key`/`Digit`
// stripped. The same string registers as a Tauri global shortcut and matches a keydown. No DOM at load.

/** The subset of `KeyboardEvent` chords read. */
export type Key = Pick<KeyboardEvent, 'code' | 'key' | 'altKey' | 'ctrlKey' | 'shiftKey' | 'metaKey'>

export const isApple = (): boolean =>
  /Mac|iPhone|iPad/.test(navigator.platform) || /Mac|iPhone|iPad/.test(navigator.userAgent)

const MOD_KEYS = ['Alt', 'Control', 'Shift', 'Meta']

/**
 * Chord from a keydown; `null` for a bare modifier, or, unless `bare`, a key without one (`bare` is for the
 * composer's editor keys: `Enter`, `Escape`).
 */
export function chordOf(e: Key, bare = false): string | null {
  const mods = [e.altKey && 'Alt', e.ctrlKey && 'Control', e.shiftKey && 'Shift', e.metaKey && 'Super'].filter(Boolean)
  if ((mods.length === 0 && !bare) || MOD_KEYS.includes(e.key)) return null
  const key = e.code.replace(/^Key|^Digit/, '')
  return key ? [...mods, key].join('+') : null
}

/** Does this keydown match `chord` exactly (every modifier accounted for)? */
export function matches(chord: string, e: Key): boolean {
  const parts = chord.split('+')
  const key = parts.pop()
  if (key !== e.code.replace(/^Key|^Digit/, '')) return false
  const has = (m: string) => parts.includes(m)
  return has('Alt') === e.altKey && has('Control') === e.ctrlKey && has('Shift') === e.shiftKey && has('Super') === e.metaKey
}

const APPLE: Record<string, string> = { Super: '⌘', Control: '⌃', Alt: '⌥', Shift: '⇧', Enter: '⏎' }
const PC: Record<string, string> = { Super: 'Win', Control: 'Ctrl' }
const KEYS: Record<string, string> = {
  Escape: 'Esc',
  ArrowUp: '↑',
  ArrowDown: '↓',
  ArrowLeft: '←',
  ArrowRight: '→',
  Semicolon: ';',
  Slash: '/',
  Backquote: '`',
  Comma: ',',
  Period: '.',
  Quote: "'",
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Minus: '-',
  Equal: '=',
}
/** Apple order: ⌃⌥⇧⌘, then the key. */
const ORDER = ['Control', 'Alt', 'Shift', 'Super']

/** The chord as the platform spells it: `⇧⌘O` on Apple, `Ctrl+Shift+O` elsewhere. */
export function label(chord: string): string {
  const parts = chord.split('+').map((p) => KEYS[p] ?? p)
  if (!isApple()) return parts.map((p) => PC[p] ?? p).join('+')
  const mods = parts.filter((p) => ORDER.includes(p)).sort((a, b) => ORDER.indexOf(a) - ORDER.indexOf(b))
  const key = parts.filter((p) => !ORDER.includes(p))
  return [...mods, ...key].map((p) => APPLE[p] ?? p).join('')
}
