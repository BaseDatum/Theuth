import { useEffect } from 'react'
import { container } from '@/lib/platform'

/** Window `CustomEvent` names the shell dispatches; consumers `addEventListener` on `window`. */
export type Hotkey =
  | 'wth:new-chat'
  | 'wth:search'
  | 'wth:toggle-sidebar'
  | 'wth:focus-composer'
  | 'wth:copy-last-response'
  | 'wth:copy-last-code'
  | 'wth:shortcuts'
  | 'wth:dictate'

type Binding = {
  code: string
  mod: boolean
  shift: boolean
  event: Hotkey
  inEditable: boolean
  /** Literal Ctrl on every OS, bypassing the `mod` mapping (54 §0.9); `mod` is ignored. */
  ctrl?: boolean
}

/** ChatGPT's bindings (25 §3): ⇧⌘O, ⌘K, ⇧⌘S, ⇧Esc, ⇧⌘C, ⇧⌘;, ⌘/, plus Ctrl+D dictation (54). Physical codes: ⇧; types `:`. `mod` is ⌘ on macOS, Ctrl elsewhere. */
const BINDINGS: Binding[] = [
  {
    code: 'KeyO',
    mod: true,
    shift: true,
    event: 'wth:new-chat',
    inEditable: false,
  },
  {
    code: 'KeyK',
    mod: true,
    shift: false,
    event: 'wth:search',
    inEditable: true,
  },
  {
    code: 'KeyS',
    mod: true,
    shift: true,
    event: 'wth:toggle-sidebar',
    inEditable: false,
  },
  {
    code: 'Escape',
    mod: false,
    shift: true,
    event: 'wth:focus-composer',
    inEditable: true,
  },
  {
    code: 'KeyC',
    mod: true,
    shift: true,
    event: 'wth:copy-last-response',
    inEditable: false,
  },
  {
    code: 'Semicolon',
    mod: true,
    shift: true,
    event: 'wth:copy-last-code',
    inEditable: false,
  },
  {
    code: 'Slash',
    mod: true,
    shift: false,
    event: 'wth:shortcuts',
    inEditable: false,
  },
  {
    code: 'KeyD',
    mod: false,
    ctrl: true,
    shift: false,
    event: 'wth:dictate',
    inEditable: true,
  },
]

const isApple = () =>
  /Mac|iPhone|iPad/.test(navigator.platform) || /Mac|iPhone|iPad/.test(navigator.userAgent)

/** The primary modifier's label: `⌘` on Apple platforms, `Ctrl` elsewhere. */
export const modKey = (): '⌘' | 'Ctrl' => (isApple() ? '⌘' : 'Ctrl')

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
  useEffect(() => {
    const bindings = touch() ? BINDINGS.filter((b) => b.event === 'wth:search') : BINDINGS
    const apple = isApple()
    const onKey = (e: KeyboardEvent) => {
      // A `ctrl` binding matches literal Ctrl alone; the rest bail when the other modifier is held.
      const other = e.altKey || (apple ? e.ctrlKey : e.metaKey)
      const mod = apple ? e.metaKey : e.ctrlKey
      const target = e.target
      const editable =
        target instanceof HTMLElement &&
        (target.isContentEditable ||
          target instanceof HTMLInputElement ||
          target instanceof HTMLTextAreaElement)
      const hit = bindings.find(
        (b) =>
          b.code === e.code &&
          b.shift === e.shiftKey &&
          (b.inEditable || !editable) &&
          (b.ctrl ? e.ctrlKey && !e.metaKey && !e.altKey : b.mod === mod && !other),
      )
      if (!hit) return
      e.preventDefault()
      window.dispatchEvent(new CustomEvent(hit.event))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
}
