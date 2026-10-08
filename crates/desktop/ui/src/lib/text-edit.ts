// Edits the composer makes itself (a paste, a completion, a chip ✕, a dictation) as editing commands the webview
// records (138 UNDO): a store write makes React set `node.value`, which no browser puts on the undo stack and
// which orphans the steps before it. `execCommand('insertText')` edits like a keystroke: one undo step, redoable,
// and the `input` it fires runs the textarea's normal `onChange`, so the store stays the single source of truth.

/** The smallest single replacement turning `prev` into `next` (`null` when equal); never splits a UTF-16 surrogate pair. */
export function diffRange(prev: string, next: string): { start: number; end: number; text: string } | null {
  if (prev === next) return null
  let start = 0
  const max = Math.min(prev.length, next.length)
  while (start < max && prev.charCodeAt(start) === next.charCodeAt(start)) start++
  // The prefix ended inside a pair (same high surrogate, different lows): back up to the high one.
  if (start > 0 && (prev.charCodeAt(start - 1) & 0xfc00) === 0xd800) start--
  let end = prev.length
  let nend = next.length
  while (end > start && nend > start && prev.charCodeAt(end - 1) === next.charCodeAt(nend - 1)) {
    end--
    nend--
  }
  // The suffix started inside a pair (same low surrogate, different highs): hand the low one to the edit too.
  if (end < prev.length && (prev.charCodeAt(end) & 0xfc00) === 0xdc00) {
    end++
    nend++
  }
  return { start, end, text: next.slice(start, nend) }
}

/** What `editText` needs of a textarea / the document — structural, so node tests stub them. */
export type EditableText = {
  value: string
  readOnly: boolean
  disabled: boolean
  focus(options?: { preventScroll?: boolean }): void
  setSelectionRange(start: number, end: number): void
}
export type EditDoc = {
  activeElement: unknown
  execCommand(command: string, showUI?: boolean, value?: string): boolean
}

/**
 * Replace `[start, end)` of `ta` with `text` as one undoable editing command. `true` only when the command took and
 * the value is what it should be; `false` (nothing to undo, caller writes the store instead) otherwise, and without
 * touching a read-only or disabled box.
 */
export function editText(ta: EditableText, start: number, end: number, text: string, doc: EditDoc = document): boolean {
  if (ta.readOnly || ta.disabled) return false
  if (text === '' && start === end) return true
  const want = ta.value.slice(0, start) + text + ta.value.slice(end)
  if (doc.activeElement !== ta) ta.focus({ preventScroll: true })
  ta.setSelectionRange(start, end)
  const ok = text === '' ? doc.execCommand('delete') : doc.execCommand('insertText', false, text)
  return ok && ta.value === want
}

/** Make `ta`'s value `next` as one undo step (caret after the change); `true` with no edit when it already is. */
export function applyText(ta: EditableText, next: string, doc: EditDoc = document): boolean {
  const d = diffRange(ta.value, next)
  return d === null || editText(ta, d.start, d.end, d.text, doc)
}
