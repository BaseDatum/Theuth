import { expect, test, vi } from 'vitest'
import { applyText, diffRange, editText } from './text-edit'

test('diffRange: the smallest single replacement', () => {
  expect(diffRange('abc', 'abc')).toBeNull()
  expect(diffRange('', 'abc')).toEqual({ start: 0, end: 0, text: 'abc' })
  expect(diffRange('abc', '')).toEqual({ start: 0, end: 3, text: '' })
  expect(diffRange('bc', 'abc')).toEqual({ start: 0, end: 0, text: 'a' })
  expect(diffRange('ac', 'abc')).toEqual({ start: 1, end: 1, text: 'b' })
  expect(diffRange('ab', 'abc')).toEqual({ start: 2, end: 2, text: 'c' })
  expect(diffRange('abcd', 'ad')).toEqual({ start: 1, end: 3, text: '' })
  expect(diffRange('a one b', 'a two b')).toEqual({ start: 2, end: 5, text: 'two' })
  expect(diffRange('abc', 'xyz')).toEqual({ start: 0, end: 3, text: 'xyz' })
  expect(diffRange('/mo', '/model ')).toEqual({ start: 3, end: 3, text: 'del ' })
})

test('diffRange: repeated characters never overlap prefix and suffix', () => {
  expect(diffRange('aaa', 'aaaa')).toEqual({ start: 3, end: 3, text: 'a' })
  expect(diffRange('aaaa', 'aaa')).toEqual({ start: 3, end: 4, text: '' })
  expect(diffRange('abab', 'ab')).toEqual({ start: 2, end: 4, text: '' })
  expect(diffRange('ab', 'abab')).toEqual({ start: 2, end: 2, text: 'ab' })
})

test('diffRange: an astral emoji is never split', () => {
  // 😀 D83D DE00 · 😁 D83D DE01 (same high) · 🈀 D83C DE00 (same low)
  expect(diffRange('a😀b', 'a😁b')).toEqual({ start: 1, end: 3, text: '😁' })
  expect(diffRange('😀', '🈀')).toEqual({ start: 0, end: 2, text: '🈀' })
  expect(diffRange('ab', 'a😀b')).toEqual({ start: 1, end: 1, text: '😀' })
  expect(diffRange('a😀b', 'ab')).toEqual({ start: 1, end: 3, text: '' })
  expect(diffRange('😀😀', '😀')).toEqual({ start: 2, end: 4, text: '' })
  for (const [p, n] of [
    ['a😀b', 'a😁b'],
    ['😀', '🈀'],
    ['😀😀', '😀'],
    ['x😀', '😀😀x'],
  ] as const) {
    const d = diffRange(p, n)!
    expect(p.slice(0, d.start) + d.text + p.slice(d.end)).toBe(n)
    expect(p.isWellFormed() && p.slice(0, d.start).isWellFormed() && p.slice(d.end).isWellFormed() && d.text.isWellFormed()).toBe(true)
  }
})

/** A textarea whose `execCommand` applies the edit like a browser would (or lies, per `mode`). */
function box(value: string, opts: { focused?: boolean; readOnly?: boolean; disabled?: boolean; mode?: 'ok' | 'refuse' | 'lie' } = {}) {
  const ta = {
    value,
    readOnly: opts.readOnly ?? false,
    disabled: opts.disabled ?? false,
    sel: [0, 0] as [number, number],
    focus: vi.fn(),
    setSelectionRange(a: number, b: number) {
      this.sel = [a, b]
    },
  }
  const exec = vi.fn((cmd: string, _ui?: boolean, text?: string) => {
    if (opts.mode === 'refuse') return false
    if (opts.mode === 'lie') return true
    const [a, b] = ta.sel
    const ins = cmd === 'delete' ? '' : (text ?? '')
    ta.value = ta.value.slice(0, a) + ins + ta.value.slice(b)
    ta.sel = [a + ins.length, a + ins.length]
    return true
  })
  const doc = { activeElement: opts.focused ? ta : null, execCommand: exec }
  return { ta, doc, exec }
}

test('editText inserts over the range as insertText and reports the result', () => {
  const b = box('hello world')
  expect(editText(b.ta, 6, 11, 'there', b.doc)).toBe(true)
  expect(b.ta.value).toBe('hello there')
  expect(b.ta.sel).toEqual([11, 11])
  expect(b.exec).toHaveBeenCalledWith('insertText', false, 'there')
  expect(b.ta.focus).toHaveBeenCalledWith({ preventScroll: true })
})

test('editText focuses only when the box is not already focused', () => {
  const b = box('abc', { focused: true })
  expect(editText(b.ta, 3, 3, 'd', b.doc)).toBe(true)
  expect(b.ta.focus).not.toHaveBeenCalled()
})

test('editText deletes with the delete command', () => {
  const b = box('a ⟦paste 1⟧ b')
  expect(editText(b.ta, 1, 11, '', b.doc)).toBe(true)
  expect(b.ta.value).toBe('a b')
  expect(b.exec).toHaveBeenCalledWith('delete')
  expect(b.exec).toHaveBeenCalledTimes(1)
})

test('editText is false when the command is refused or the value did not come out right', () => {
  const refused = box('abc', { mode: 'refuse' })
  expect(editText(refused.ta, 3, 3, 'd', refused.doc)).toBe(false)
  expect(refused.ta.value).toBe('abc')
  const liar = box('abc', { mode: 'lie' })
  expect(editText(liar.ta, 3, 3, 'd', liar.doc)).toBe(false)
})

test('editText leaves a read-only or disabled box alone', () => {
  for (const b of [box('abc', { readOnly: true }), box('abc', { disabled: true })]) {
    expect(editText(b.ta, 0, 3, 'x', b.doc)).toBe(false)
    expect(b.exec).not.toHaveBeenCalled()
    expect(b.ta.focus).not.toHaveBeenCalled()
  }
})

test('applyText diffs then edits; equal text is a success with no command', () => {
  const b = box('alpha gamma', { focused: true })
  expect(applyText(b.ta, 'alpha beta gamma', b.doc)).toBe(true)
  expect(b.ta.value).toBe('alpha beta gamma')
  expect(b.exec).toHaveBeenCalledWith('insertText', false, 'beta ')
  expect(applyText(b.ta, 'alpha beta gamma', b.doc)).toBe(true)
  expect(b.exec).toHaveBeenCalledTimes(1)
  const refused = box('abc', { mode: 'refuse' })
  expect(applyText(refused.ta, 'abcd', refused.doc)).toBe(false)
})
