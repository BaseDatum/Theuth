// Present mode and the rules that keep chrome from popping up uninvited. Presenting hides every piece of chrome (the
// cluster, card and frame toolbars, the composer dock, the outline, mark menus, off-screen chevrons, the reply bubble),
// locks the Hand tool (pan and zoom still work, nothing selects or drags) and hides the cursor after 2 s without a move.
// Shift+P toggles it, Esc leaves it before anything else Esc does; the tool latched before comes back afterwards (the
// ink-tool latch: only the owner's pick, Hand/Select or Esc puts an ink tool down, and Present is not one of them).
import type { Mode, WbStore } from './canvas/store'

/** The cursor hides after this long without a pointer move while presenting. */
export const CURSOR_IDLE_MS = 2_000
/** The exit button, once raised (a move into its corner, a tap), stays this long after the last such move. */
export const EXIT_SHOW_MS = 2_500
/** The strip along the right edge (lower half) where a mouse moving rightwards raises the cluster. */
export const REVEAL_STRIP_PX = 120
/** The top-right corner where a mouse move raises the exit button while presenting. */
export const EXIT_CORNER_PX = 160

export type PresentMode = { on(): void; off(): void; toggle(): void; dispose(): void }

/** Enter/leave Present on `store`, and hold its invariants while on: Hand, no selection, no entered card. */
export function presentMode(store: WbStore): PresentMode {
  let before: Mode | null = null
  const dispose = store.subscribe((s) => {
    // Any writer (a Pencil double-tap, a stray shortcut, a click on a card) is put back at once.
    if (s.present && (s.mode !== 'hand' || s.selection.length > 0 || s.entered !== null)) s.set({ mode: 'hand', selection: [], entered: null })
  })
  const set = (on: boolean) => {
    const s = store.getState()
    if (on === s.present) return
    if (on) {
      before = s.mode
      s.set({ present: true, mode: 'hand', selection: [], entered: null })
    } else {
      s.set({ present: false, mode: before ?? s.mode })
      before = null
    }
  }
  return { on: () => set(true), off: () => set(false), toggle: () => set(!store.getState().present), dispose }
}

export type PresentKey = 'toggle' | 'exit' | 'swallow' | null

/** What a keydown outside a text field does to Present: Shift+P toggles; while presenting Esc exits and the shell's
 * and canvas's single-key shortcuts (tools, add, delete, enter) are swallowed. Modifier chords pass through. */
export function presentKey(e: Pick<KeyboardEvent, 'key' | 'shiftKey' | 'metaKey' | 'ctrlKey' | 'altKey'>, present: boolean): PresentKey {
  if (e.metaKey || e.ctrlKey || e.altKey) return null
  if (e.shiftKey && e.key.toLowerCase() === 'p') return 'toggle'
  if (!present) return null
  if (e.key === 'Escape') return 'exit'
  if ((e.key.length === 1 && e.key !== ' ') || e.key === 'Delete' || e.key === 'Backspace' || e.key === 'Enter') return 'swallow'
  return null
}

type At = { x: number; y: number }
type Rect = { left: number; top: number; width: number; height: number }

/** Desktop: the cluster rises when the mouse moves toward it — rightwards inside the strip along the right edge, lower
 * half — never on a rest (no dwell timer), never with Hand (no hover UI while panning) and never while presenting. */
export function revealsCluster(at: At, r: Rect, movementX: number, mode: Mode, present: boolean): boolean {
  if (present || mode === 'hand' || movementX <= 0) return false
  return at.x > r.left + r.width - REVEAL_STRIP_PX && at.y > r.top + r.height / 2
}

/** While presenting: a mouse move into the top-right corner raises the exit button. */
export function revealsExit(at: At, r: Rect): boolean {
  return at.x > r.left + r.width - EXIT_CORNER_PX && at.y < r.top + EXIT_CORNER_PX
}

export type Awake = { poke(): void; stop(): void; dispose(): void }

/** On while poked, off `ms` after the last poke; `stop` turns it off now. `onChange` only on a change. */
export function awake(o: { ms: number; onChange(on: boolean): void }): Awake {
  let on = false
  let timer: ReturnType<typeof setTimeout> | undefined
  const set = (next: boolean) => {
    if (next === on) return
    on = next
    o.onChange(on)
  }
  return {
    poke() {
      clearTimeout(timer)
      set(true)
      timer = setTimeout(() => set(false), o.ms)
    },
    stop() {
      clearTimeout(timer)
      set(false)
    },
    dispose: () => clearTimeout(timer),
  }
}
