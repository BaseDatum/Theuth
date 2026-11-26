import { expect, test } from 'vitest'
import {
  type DictBuffer,
  begin,
  decorate,
  display,
  edit,
  end,
  final,
  inputEdit,
  locked,
  partial,
  redo,
  replay,
  replayInto,
  select,
  undo,
  volatileRange,
} from './dictation-buffer'

const caret = (b: DictBuffer) => b.sel

test('a partial shows at the caret mid-text, the caret riding after it; the final lands it as plain text', () => {
  let b = begin('hello world', 5, 5)
  b = partial(b, 0, 'there')
  expect(display(b)).toBe('hello there world')
  expect(volatileRange(b)).toEqual({ start: 5, end: 11 })
  expect(caret(b)).toEqual({ start: 11, end: 11 })
  b = partial(b, 0, 'there big')
  expect(display(b)).toBe('hello there big world')
  b = final(b, 0, 'there big')
  expect(b.vol).toBeNull()
  expect(display(b)).toBe('hello there big world')
  expect(caret(b)).toEqual({ start: 15, end: 15 })
})

test('spacing and capitals fit the insertion point, conservatively', () => {
  expect(decorate('', '', 'hi there')).toEqual({ text: 'Hi there', caret: 8 })
  expect(decorate('Done.', '', 'next')).toEqual({ text: ' Next', caret: 5 })
  expect(decorate('one\n', '', 'two')).toEqual({ text: 'Two', caret: 3 })
  expect(decorate('word', 'after', 'mid')).toEqual({ text: ' mid ', caret: 4 })
  expect(decorate('word ', ', after', 'mid')).toEqual({ text: 'mid', caret: 3 })
  expect(decorate('(', ')', 'aside')).toEqual({ text: 'aside', caret: 5 })
  // Never lowered mid-sentence, never a mixed-case word raised.
  expect(decorate('I said', '', 'Paris')).toEqual({ text: ' Paris', caret: 6 })
  expect(decorate('', '', 'iPhone')).toEqual({ text: 'iPhone', caret: 6 })
  expect(decorate('x', '', ', then')).toEqual({ text: ', then', caret: 6 })
})

test('moving the caret between utterances: the next one opens at the new caret', () => {
  let b = final(begin('ab cd', 5, 5), 0, 'one')
  expect(display(b)).toBe('ab cd one')
  b = select(b, 0, 0)
  b = partial(b, 1, 'two')
  expect(display(b)).toBe('Two ab cd one')
  expect(caret(b)).toEqual({ start: 3, end: 3 })
  b = final(b, 1, 'two')
  expect(display(b)).toBe('Two ab cd one')
})

test('moving the caret during a partial: the span keeps its anchor and the caret stays where the user put it', () => {
  let b = partial(begin('hello', 5, 5), 0, 'big')
  b = select(b, 0, 0)
  b = partial(b, 0, 'big world')
  expect(display(b)).toBe('hello big world')
  expect(caret(b)).toEqual({ start: 0, end: 0 })
  b = final(b, 0, 'big world')
  expect(caret(b)).toEqual({ start: 0, end: 0 })
  // A caret after the span moves with its growth.
  b = partial(select(b, 15, 15), 1, 'yes')
  b = select(b, 0, 0)
  b = select(b, display(b).length, display(b).length)
  const at = display(b).length
  b = partial(b, 1, 'yes indeed')
  expect(caret(b).start).toBe(at + ' indeed'.length)
})

test('typing before the span shifts it; typing right behind it stays behind; editing into it is refused', () => {
  let b = partial(begin('abc', 3, 3), 0, 'x y')
  expect(display(b)).toBe('abc x y')
  b = edit(b, 0, 0, 'Z', { start: 1, end: 1 })!
  expect(display(b)).toBe('Zabc x y')
  expect(volatileRange(b)).toEqual({ start: 4, end: 8 })
  // At the anchor (before the span): goes before it.
  b = edit(b, 4, 4, '-', { start: 5, end: 5 })!
  expect(display(b)).toBe('Zabc- x y')
  // Right behind the span: stays behind it as the span grows (the span now ends in a separating space).
  b = edit(b, 9, 9, 'Q', { start: 10, end: 10 })!
  b = partial(b, 0, 'x y z')
  expect(display(b)).toBe('Zabc- x y z Q')
  expect(edit(b, 7, 7, 'k', { start: 8, end: 8 })).toBeNull()
  expect(edit(b, 3, 7, '', { start: 3, end: 3 })).toBeNull()
  const r = volatileRange(b)!
  expect(locked(b, r.start + 1, r.start + 1, 'insertText')).toBe(true)
  expect(locked(b, r.start, r.start, 'insertText')).toBe(false)
  expect(locked(b, r.end, r.end, 'insertText')).toBe(false)
  expect(locked(b, r.end, r.end, 'deleteContentBackward')).toBe(true)
  expect(locked(b, r.start, r.start, 'deleteContentForward')).toBe(true)
  expect(locked(b, r.start, r.start, 'deleteContentBackward')).toBe(false)
  expect(locked(b, 0, r.start + 1, 'insertText')).toBe(true)
  b = final(b, 0, 'x y z')
  expect(display(b)).toBe('Zabc- x y z Q')
  expect(locked(b, 7, 7, 'insertText')).toBe(false)
})

test('a selection is replaced by the dictation, one undo step with its words', () => {
  let b = begin('I like cats a lot', 7, 11)
  b = partial(b, 0, 'dogs')
  expect(display(b)).toBe('I like dogs a lot')
  b = final(b, 0, 'dogs')
  expect(display(b)).toBe('I like dogs a lot')
  b = undo(b)
  expect(display(b)).toBe('I like cats a lot')
  expect(caret(b)).toEqual({ start: 7, end: 11 })
  b = redo(b)
  expect(display(b)).toBe('I like dogs a lot')
  // Nothing said: the selection comes back.
  const none = final(partial(begin('I like cats', 7, 11), 0, 'd'), 0, '')
  expect(display(none)).toBe('I like cats')
  expect(none.undo).toEqual([])
})

test('undo takes back one landed utterance at a time; typing in between is its own step', () => {
  let b = final(begin('', 0, 0), 0, 'Hello.')
  b = final(b, 1, 'how are you?')
  expect(display(b)).toBe('Hello. How are you?')
  b = edit(b, 19, 19, ' ok', { start: 22, end: 22 })!
  b = final(b, 2, 'fine')
  expect(display(b)).toBe('Hello. How are you? ok fine')
  b = undo(b)
  expect(display(b)).toBe('Hello. How are you? ok')
  b = undo(b)
  expect(display(b)).toBe('Hello. How are you?')
  b = undo(b)
  expect(display(b)).toBe('Hello.')
  b = redo(b)
  expect(display(b)).toBe('Hello. How are you?')
  b = undo(undo(b))
  expect(display(b)).toBe('')
  expect(undo(b)).toBe(b)
})

test('undo with a span open keeps the span at its words', () => {
  let b = final(begin('', 0, 0), 0, 'one')
  b = partial(b, 1, 'two')
  expect(display(b)).toBe('One two')
  b = undo(b)
  expect(display(b)).toBe(' two')
  // Re-fitted to where it now sits: the start of the box.
  b = final(b, 1, 'two')
  expect(display(b)).toBe('Two')
})

test('a later utterance closes the open one where it is; "scratch that" rewrites a landed one if intact', () => {
  let b = partial(begin('', 0, 0), 0, 'one')
  b = partial(b, 1, 'two')
  expect(display(b)).toBe('One two')
  expect(volatileRange(b)).toEqual({ start: 3, end: 7 })
  b = final(b, 0, 'one!')
  expect(display(b)).toBe('One! two')
  b = final(b, 1, 'two')
  b = final(b, 1, '')
  expect(display(b)).toBe('One!')
  // Edited by the user: the engine's rewrite leaves it alone.
  let c = final(begin('', 0, 0), 0, 'alpha')
  c = edit(c, 2, 2, 'X', { start: 3, end: 3 })!
  c = final(c, 0, '')
  expect(display(c)).toBe('AlXpha')
})

test('end keeps an open span, Esc takes out the dictated words and keeps the typing; the steps replay to the value', () => {
  let b = final(begin('keep', 4, 4), 0, 'said')
  b = edit(b, 0, 0, 'X', { start: 1, end: 1 })!
  b = partial(select(b, 10, 10), 1, 'more')
  const kept = end(b, false)
  expect(kept.value).toBe('Xkeep said more')
  expect(replay(kept.origin, kept.steps)).toBe(kept.value)
  const gone = end(b, true)
  expect(gone.value).toBe('Xkeep')
  expect(replay(gone.origin, gone.steps)).toBe(gone.value)
  // A selection the dictation replaced comes back on Esc.
  const sel = end(final(begin('a big b', 2, 5), 0, 'small'), true)
  expect(sel.value).toBe('a  b')
  const open = end(partial(begin('a big b', 2, 5), 0, 'small'), true)
  expect(open.value).toBe('a big b')
})

test('inputEdit places a repeated letter where it was typed and reads deletions and replaced selections', () => {
  expect(inputEdit('aa', 'aaa', 2)).toEqual({ start: 1, end: 1, text: 'a' })
  expect(inputEdit('abc', 'ac', 1)).toEqual({ start: 1, end: 2, text: '' })
  expect(inputEdit('abcdef', 'aXef', 2)).toEqual({ start: 1, end: 4, text: 'X' })
  expect(inputEdit('same', 'same', 2)).toBeNull()
})

test('replayInto lands each step as an editing command from the original text', () => {
  let b = final(begin('x', 1, 1), 0, 'one')
  b = final(b, 1, 'two')
  const f = end(b, false)
  const ta = { value: 'stale', readOnly: false, disabled: false, sel: [0, 0], focus() {}, setSelectionRange(s: number, e: number) { this.sel = [s, e] } }
  const commands: string[] = []
  const doc = {
    activeElement: ta,
    execCommand(cmd: string, _ui?: boolean, text = '') {
      commands.push(cmd)
      const [s, e] = ta.sel as [number, number]
      ta.value = ta.value.slice(0, s) + (cmd === 'delete' ? '' : text) + ta.value.slice(e)
      return true
    },
  }
  expect(replayInto(ta, f.origin, f.steps, doc)).toBe(true)
  expect(ta.value).toBe('x one two')
  expect(commands).toEqual(['insertText', 'insertText'])
})
