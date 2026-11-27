import { container } from '@/lib/platform'

/**
 * C0 control characters (excluding tab `\t`, LF `\n`, CR `\r`), DEL (`0x7F`), and Apple's Private Use Area
 * `NSFunctionKey` range (`U+F700`–`U+F74F`, where Left/Right arrow are `U+F702`/`U+F703`).
 *
 * On macOS, Tauri's `unstable` feature hosts the page inside a child `WryWebViewParent` (tauri-apps/tauri#10194,
 * wry#1177): unhandled arrow / Home / End keys in `<input>` or `<textarea>` fall through AppKit's `interpretKeyEvents:`
 * into `insertText:` with classic Mac control codes (`U+001C` Left, `U+001D` Right, `U+001E` Up, `U+001F` Down,
 * `U+0001` Home, `U+0004` End) or `NSFunctionKey` PUA code points, which render as tofu / `.notdef` boxes.
 */
// eslint-disable-next-line no-control-regex
const ONLY_STRAY = /^[\x00-\x08\x0B\x0C\x0E-\x1F\x7F\uF700-\uF74F]+$/
// eslint-disable-next-line no-control-regex
const ANY_STRAY = /[\x00-\x08\x0B\x0C\x0E-\x1F\x7F\uF700-\uF74F]/g

/** True when `data` is non-empty and consists solely of stray control / NSFunctionKey characters. */
export function isStrayControlText(data: string | null | undefined): boolean {
  return typeof data === 'string' && data.length > 0 && ONLY_STRAY.test(data)
}

/** Strips stray control / NSFunctionKey characters from `text`, preserving tabs, newlines and normal characters. */
export function stripStrayControlText(text: string): string {
  return text.replace(ANY_STRAY, '')
}

const INSERT_TYPES = new Set(['insertText', 'insertReplacementText'])

/**
 * Installs capture-phase `beforeinput` (primary) and `input` (fallback) guards on `win` that drop stray macOS
 * arrow/function-key control insertions before they enter fields or React state. No-op off macOS desktop unless
 * `force` is set (tests).
 */
export function installMacStrayKeyGuard(win: Window = window, force = false): () => void {
  if (!force && container() !== 'macos') return () => undefined

  const onBeforeInput = (e: Event) => {
    const ie = e as InputEvent
    if (!INSERT_TYPES.has(ie.inputType)) return
    if (isStrayControlText(ie.data)) ie.preventDefault()
  }

  const onInput = (e: Event) => {
    const ie = e as InputEvent
    if (!INSERT_TYPES.has(ie.inputType) || !isStrayControlText(ie.data)) return
    const target = ie.target
    if (!(target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)) return
    const cur = target.value
    const cleaned = stripStrayControlText(cur)
    if (cleaned === cur) return
    const start = target.selectionStart
    const end = target.selectionEnd
    const nextStart = start === null ? null : stripStrayControlText(cur.slice(0, start)).length
    const nextEnd = end === null ? null : stripStrayControlText(cur.slice(0, end)).length
    target.value = cleaned
    if (nextStart !== null && nextEnd !== null) target.setSelectionRange(nextStart, nextEnd)
  }

  win.addEventListener('beforeinput', onBeforeInput, true)
  win.addEventListener('input', onInput, true)
  return () => {
    win.removeEventListener('beforeinput', onBeforeInput, true)
    win.removeEventListener('input', onInput, true)
  }
}
