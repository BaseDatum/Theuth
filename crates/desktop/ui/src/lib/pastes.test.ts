import { expect, test } from 'vitest'
import { attach, drop, expand, label, retext, tokenize, type Pastes } from './pastes'

const none: Pastes = new Map()

test('small pastes come back verbatim, large ones are held as a chip', () => {
  expect(tokenize('a\r\nb\rc', none)).toEqual({ text: 'a\nb\nc', pastes: none })
  expect(tokenize('1\n2\n3\n4\n5\n', none).text).toBe('1\n2\n3\n4\n5\n')
  expect(tokenize('x'.repeat(400), none).text).toBe('x'.repeat(400))
  const long = tokenize('x'.repeat(401), none)
  expect(long.text).toBe('')
  expect([...long.pastes.values()].map(label)).toEqual(['401 chars'])
  const two = tokenize('1\n2\n3\n4\n5\n6', long.pastes)
  expect(two.text).toBe('')
  expect([...two.pastes.values()].map(label)).toEqual(['401 chars', '6 lines'])
})

test('file labels: size when known, name otherwise', () => {
  const a = attach('/d/att/ab12.png', 1440, 900, none)
  const b = attach('/d/att/spec.pdf', 0, 0, a)
  expect([...b.values()].map(label)).toEqual(['1440×900', 'spec.pdf'])
})

test('expand appends pasted blocks after the typed text and lists files in order', () => {
  const t = tokenize('x'.repeat(401), none).pastes
  const g = attach('/d/b.pdf', 0, 0, attach('/d/a.png', 2, 2, t))
  expect(expand('describe', g)).toEqual({ text: `describe\n\n${'x'.repeat(401)}`, files: ['/d/a.png', '/d/b.pdf'] })
  expect(expand('', g).text).toBe('x'.repeat(401))
  expect(expand('nothing held', none)).toEqual({ text: 'nothing held', files: [] })
})

test('drop keeps numbering; retext edits in place', () => {
  const t = tokenize('x'.repeat(401), none).pastes
  const f = attach('/d/a.png', 2, 2, t)
  const kept = drop(0, f)
  expect([...kept.keys()]).toEqual([1])
  expect([...attach('/d/c.png', 1, 1, kept).keys()]).toEqual([1, 2])
  expect(expand('', retext(0, 'edited', f)).text).toBe('edited')
})
