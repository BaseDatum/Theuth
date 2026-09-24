import { expect, test } from 'vitest'
import { attach, attachDoc, drop, expand, label, type Pastes, reconcile, retext, token, tokenize, untoken } from './pastes'

const none: Pastes = new Map()
const X = 'x'.repeat(401)
const SIX = '1\n2\n3\n4\n5\n6'

test('small pastes come back verbatim, large ones are held and return their token', () => {
  expect(tokenize('a\r\nb\rc', none)).toEqual({ text: 'a\nb\nc', pastes: none })
  expect(tokenize('1\n2\n3\n4\n5\n', none).text).toBe('1\n2\n3\n4\n5\n')
  expect(tokenize('x'.repeat(400), none).text).toBe('x'.repeat(400))
  const long = tokenize(X, none)
  expect(long.text).toBe('⟦paste 1⟧')
  expect(long.text).toBe(token(1))
  expect([...long.pastes.values()].map(label)).toEqual(['401 chars'])
  const two = tokenize(SIX, long.pastes)
  expect(two.text).toBe('⟦paste 2⟧')
  expect([...two.pastes.values()].map(label)).toEqual(['401 chars', '6 lines'])
  // One trailing newline follows the token instead of ending the block.
  const nl = tokenize(`${SIX}\n`, none)
  expect(nl.text).toBe('⟦paste 1⟧\n')
  expect(nl.pastes.get(1)).toEqual({ kind: 'text', text: SIX })
  expect(expand(`${nl.text}next`, nl.pastes).text).toBe(`${SIX}\nnext`)
})

test('file labels: size when known, name otherwise', () => {
  const a = attach('/d/att/ab12.png', 1440, 900, none)
  const b = attach('/d/att/spec.pdf', 0, 0, a)
  expect([...b.values()].map(label)).toEqual(['1440×900', 'spec.pdf'])
})

test('expand puts a block where its token stands and lists files in order', () => {
  const t = tokenize(X, none).pastes
  const g = attach('/d/b.pdf', 0, 0, attach('/d/a.png', 2, 2, t))
  expect(expand('a ⟦paste 1⟧ b', g)).toEqual({ text: `a ${X} b`, files: ['/d/a.png', '/d/b.pdf'] })
  expect(expand('⟦paste 1⟧', g).text).toBe(X)
  expect(expand('see:\n⟦paste 1⟧\ndone', g).text).toBe(`see:\n${X}\ndone`)
  expect(expand('nothing held', none)).toEqual({ text: 'nothing held', files: [] })
})

test('blocks come out in typed order, not arrival order', () => {
  const p = tokenize(SIX, tokenize(X, none).pastes).pastes
  expect(expand('compare ⟦paste 2⟧ with ⟦paste 1⟧ ok', p).text).toBe(`compare ${SIX} with ${X} ok`)
})

test('a deleted token drops its block: expand omits it, reconcile removes the chip', () => {
  const p = attach('/d/a.png', 2, 2, tokenize(SIX, tokenize(X, none).pastes).pastes)
  expect(expand('only ⟦paste 2⟧', p)).toEqual({ text: `only ${SIX}`, files: ['/d/a.png'] })
  expect([...reconcile('only ⟦paste 2⟧', p).keys()]).toEqual([2, 3])
  expect(reconcile('⟦paste 1⟧ ⟦paste 2⟧', p)).toBe(p)
  // A paste over a selection that held a token: the spliced text loses it, reconcile follows.
  const spliced = 'compare ⟦paste 1⟧ with ⟦paste 2⟧'.replace('⟦paste 1⟧ with', 'small')
  expect([...reconcile(spliced, p).keys()]).toEqual([2, 3])
})

test('an unknown token is left verbatim; blocks are not re-interpreted as replacement patterns', () => {
  const p = tokenize('$& $1 $$\n2\n3\n4\n5\n6', none).pastes
  expect(expand('⟦paste 7⟧ ⟦paste 1⟧', p).text).toBe('⟦paste 7⟧ $& $1 $$\n2\n3\n4\n5\n6')
  expect(expand('⟦paste 1⟧', none).text).toBe('⟦paste 1⟧')
})

test('untoken removes the token, its own line, and a blank line it doubled', () => {
  expect(untoken('a ⟦paste 1⟧ b', 1)).toBe('a  b')
  expect(untoken('a\n⟦paste 1⟧\nb', 1)).toBe('a\nb')
  expect(untoken('a\n\n⟦paste 1⟧\n\nb', 1)).toBe('a\n\nb')
  expect(untoken('a\n  ⟦paste 2⟧  \n⟦paste 1⟧', 2)).toBe('a\n⟦paste 1⟧')
  expect(untoken('⟦paste 1⟧\n', 1)).toBe('')
  expect(untoken('no token', 1)).toBe('no token')
})

test('drop keeps numbering; retext edits in place', () => {
  const t = tokenize(X, none).pastes
  const f = attach('/d/a.png', 2, 2, t)
  const kept = drop(1, f)
  expect([...kept.keys()]).toEqual([2])
  expect([...attach('/d/c.png', 1, 1, kept).keys()]).toEqual([2, 3])
  expect(expand('⟦paste 1⟧', retext(1, 'edited', f)).text).toBe('edited')
})

test('doc chips expand to marker lines ahead of the text; the same ref attaches once', () => {
  const a = attachDoc({ ref: 'abc', title: 'Plan "B"' }, none)
  expect([...a.values()].map(label)).toEqual(['Plan "B"'])
  expect(attachDoc({ ref: 'abc', title: 'Plan "B"' }, a)).toBe(a)
  const k = attachDoc({ ref: 'knowledge:facts/x', title: 'Facts' }, a)
  expect(expand('add a bullet', k)).toEqual({
    text: '[doc abc "Plan \\"B\\""]\n[doc knowledge:facts/x "Facts"]\nadd a bullet',
    files: [],
  })
  // Dropping the chip drops its marker; a pasted block still lands at its token.
  const t = tokenize(X, drop(1, k))
  expect(expand(`hi ${t.text}`, t.pastes).text).toBe(`[doc knowledge:facts/x "Facts"]\nhi ${X}`)
})
