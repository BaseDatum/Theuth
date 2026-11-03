// macOS editing keys in the terminal viewer (167 D1). xterm 6 sends ⌥←/⌥→ as `ESC[1;3D`/`ESC[1;3C`, which bash and
// zsh do not bind (they insert `;3D` into the line), and drops ⌘←/⌘→/⌘⌫. Terminal.app's bytes instead: readline's
// word and line motions. Windows and Linux keep xterm's defaults.

/** The subset of `KeyboardEvent` the map reads. */
export type KeyInput = Pick<KeyboardEvent, 'type' | 'key' | 'code' | 'altKey' | 'ctrlKey' | 'shiftKey' | 'metaKey'>

const ALT: Record<string, string> = { ArrowLeft: '\x1bb', ArrowRight: '\x1bf' }
const META: Record<string, string> = { ArrowLeft: '\x01', ArrowRight: '\x05', Backspace: '\x15' }

/** Bytes for a macOS editing keydown (`ESC b`, `ESC f`, ^A, ^E, ^U); `null` leaves the key to xterm. ⌥⌫ is xterm's own `ESC DEL`. */
export function macKeyBytes(e: KeyInput, mac: boolean): string | null {
  if (!mac || e.type !== 'keydown' || e.ctrlKey || e.shiftKey || e.altKey === e.metaKey) return null
  return (e.altKey ? ALT : META)[e.code] ?? null
}
