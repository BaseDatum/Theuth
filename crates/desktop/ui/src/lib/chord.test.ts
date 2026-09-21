import { expect, test } from 'vitest'
import { type Key, chordOf, label, matches } from './chord'

const key = (code: string, mods: Partial<Key> = {}): Key => ({
  code,
  key: code,
  altKey: false,
  ctrlKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
})

test('matches accounts for every modifier', () => {
  expect(matches('Super+Shift+O', key('KeyO', { metaKey: true, shiftKey: true }))).toBe(true)
  expect(matches('Super+Shift+O', key('KeyO', { metaKey: true }))).toBe(false)
  expect(matches('Super+K', key('KeyK', { metaKey: true, altKey: true }))).toBe(false)
  expect(matches('Enter', key('Enter'))).toBe(true)
  expect(matches('Enter', key('Enter', { shiftKey: true }))).toBe(false)
  expect(matches('Alt+ArrowUp', key('ArrowUp', { altKey: true }))).toBe(true)
})

test('chordOf reads the physical key and needs a modifier unless bare', () => {
  expect(chordOf(key('Semicolon', { metaKey: true, shiftKey: true }))).toBe('Shift+Super+Semicolon')
  expect(chordOf(key('Digit1', { altKey: true }))).toBe('Alt+1')
  expect(chordOf(key('Enter'))).toBeNull()
  expect(chordOf(key('Enter'), true)).toBe('Enter')
  expect(chordOf({ ...key('ShiftLeft', { shiftKey: true }), key: 'Shift' })).toBeNull()
})

test('a chord round-trips through chordOf and matches', () => {
  const e = key('Space', { altKey: true })
  const chord = chordOf(e) as string
  expect(chord).toBe('Alt+Space')
  expect(matches(chord, e)).toBe(true)
})

test('label spells the platform', () => {
  const apple = /Mac|iPhone|iPad/.test(navigator.platform) || /Mac|iPhone|iPad/.test(navigator.userAgent)
  expect(label('Shift+Super+O')).toBe(apple ? '⇧⌘O' : 'Shift+Win+O')
  expect(label('Alt+Escape')).toBe(apple ? '⌥Esc' : 'Alt+Esc')
})
