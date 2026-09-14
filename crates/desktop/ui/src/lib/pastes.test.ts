import { expect, test } from 'vitest'
import { attach, expand, prune, tokenize, type Pastes } from './pastes'

const none: Pastes = new Map()

test('small pastes come back verbatim, large ones as a token', () => {
  expect(tokenize('a\r\nb\rc', none)).toEqual({ text: 'a\nb\nc', pastes: none })
  expect(tokenize('1\n2\n3\n4\n5\n', none).text).toBe('1\n2\n3\n4\n5\n')
  expect(tokenize('1\n2\n3\n4\n5\n6', none).text).toBe('[⎘ paste 1: 6 lines]')
  expect(tokenize('x'.repeat(400), none).text).toBe('x'.repeat(400))
  const long = tokenize('x'.repeat(401), none)
  expect(long.text).toBe('[⎘ paste 1: 401 chars]')
  expect(tokenize('1\n2\n3\n4\n5\n6', long.pastes).text).toBe('[⎘ paste 2: 6 lines]')
})

test('file tokens: size when known, name otherwise, file for pdf', () => {
  const a = attach('/d/att/ab12.png', 1440, 900, none)
  expect(a.token).toBe('[⎘ image 1: 1440×900]')
  const b = attach('/d/att/spec.pdf', 0, 0, a.pastes)
  expect(b.token).toBe('[⎘ file 2: spec.pdf]')
  expect(attach('/d/shot.jpg', 0, 0, b.pastes).token).toBe('[⎘ image 3: shot.jpg]')
})

test('expand replaces text tokens, removes file tokens with one space, lists files', () => {
  const t = tokenize('x'.repeat(401), none)
  const f = attach('/d/a.png', 2, 2, t.pastes)
  const g = attach('/d/b.pdf', 0, 0, f.pastes)
  const line = `describe ${f.token} and ${t.text} ${g.token}`
  expect(expand(line, g.pastes)).toEqual({ text: `describe and ${'x'.repeat(401)}`, files: ['/d/a.png', '/d/b.pdf'] })
  expect(expand('nothing left', g.pastes)).toEqual({ text: 'nothing left', files: [] })
})

test('prune drops pastes whose token is gone and keeps numbering', () => {
  const t = tokenize('x'.repeat(401), none)
  const f = attach('/d/a.png', 2, 2, t.pastes)
  expect(prune(`${t.text} ${f.token}`, f.pastes)).toBe(f.pastes)
  const kept = prune(f.token, f.pastes)
  expect([...kept.keys()]).toEqual([1])
  expect(attach('/d/c.png', 1, 1, kept).token).toBe('[⎘ image 3: 1×1]')
})
