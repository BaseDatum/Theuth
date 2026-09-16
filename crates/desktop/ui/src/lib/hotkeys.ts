import { useEffect } from 'react'

/** Window `CustomEvent` names the shell dispatches; consumers `addEventListener` on `window`. */
export type Hotkey =
  | 'wth:new-chat'
  | 'wth:search'
  | 'wth:toggle-sidebar'
  | 'wth:focus-composer'
  | 'wth:copy-last-response'
  | 'wth:copy-last-code'
  | 'wth:shortcuts'

type Binding = {
  code: string
  mod: boolean
  shift: boolean
  event: Hotkey
  inEditable: boolean
}

/** ChatGPT's bindings (25 §3): ⇧⌘O, ⌘K, ⇧⌘S, ⇧Esc, ⇧⌘C, ⇧⌘;, ⌘/. Physical codes: ⇧; types `:`. `mod` is ⌘ on macOS, Ctrl elsewhere. */
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
]

const isApple = () =>
  /Mac|iPhone|iPad/.test(navigator.platform) || /Mac|iPhone|iPad/.test(navigator.userAgent)

/** The primary modifier's label: `⌘` on Apple platforms, `Ctrl` elsewhere. */
export const modKey = (): '⌘' | 'Ctrl' => (isApple() ? '⌘' : 'Ctrl')

/** Primary pointer is a finger: no hover, no keyboard chrome. */
export const touch = (): boolean => window.matchMedia('(pointer: coarse)').matches

/** The macOS Tauri window: traffic lights to inset around, a title bar to drag by. False in iOS Tauri and every browser. */
export const isMacDesktop = (): boolean =>
  '__TAURI_INTERNALS__' in window && !/iPhone|iPad/.test(navigator.userAgent)

/** One `keydown` listener for the whole window; mount once in the root shell. Registers nothing on touch devices. */
export function useHotkeys(): void {
  useEffect(() => {
    if (touch()) return
    const apple = isApple()
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey || (apple ? e.ctrlKey : e.metaKey)) return
      const mod = apple ? e.metaKey : e.ctrlKey
      const target = e.target
      const editable =
        target instanceof HTMLElement &&
        (target.isContentEditable ||
          target instanceof HTMLInputElement ||
          target instanceof HTMLTextAreaElement)
      const hit = BINDINGS.find(
        (b) =>
          b.mod === mod &&
          b.shift === e.shiftKey &&
          b.code === e.code &&
          (b.inEditable || !editable),
      )
      if (!hit) return
      e.preventDefault()
      window.dispatchEvent(new CustomEvent(hit.event))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
}
