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

type Binding = { code: string; meta: boolean; shift: boolean; event: Hotkey; inEditable: boolean }

/** ChatGPT's bindings (25 §3): ⇧⌘O, ⌘K, ⇧⌘S, ⇧Esc, ⇧⌘C, ⇧⌘;, ⌘/. Physical codes: ⇧; types `:`. */
const BINDINGS: Binding[] = [
  { code: 'KeyO', meta: true, shift: true, event: 'wth:new-chat', inEditable: false },
  { code: 'KeyK', meta: true, shift: false, event: 'wth:search', inEditable: true },
  { code: 'KeyS', meta: true, shift: true, event: 'wth:toggle-sidebar', inEditable: false },
  { code: 'Escape', meta: false, shift: true, event: 'wth:focus-composer', inEditable: true },
  { code: 'KeyC', meta: true, shift: true, event: 'wth:copy-last-response', inEditable: false },
  { code: 'Semicolon', meta: true, shift: true, event: 'wth:copy-last-code', inEditable: false },
  { code: 'Slash', meta: true, shift: false, event: 'wth:shortcuts', inEditable: false },
]

/** One `keydown` listener for the whole window; mount once in the root shell. */
export function useHotkeys(): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey || e.ctrlKey) return
      const target = e.target
      const editable =
        target instanceof HTMLElement &&
        (target.isContentEditable || target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)
      const hit = BINDINGS.find(
        (b) => b.meta === e.metaKey && b.shift === e.shiftKey && b.code === e.code && (b.inEditable || !editable),
      )
      if (!hit) return
      e.preventDefault()
      window.dispatchEvent(new CustomEvent(hit.event))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
}
