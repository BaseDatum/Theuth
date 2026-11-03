import { expect, test } from 'vitest'
import { type KeyInput, macKeyBytes } from './terminal-keys'

const key = (code: string, mods: Partial<KeyInput> = {}): KeyInput => ({
  type: 'keydown',
  code,
  key: code,
  altKey: false,
  ctrlKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
})

test('macOS editing keys map to readline motions', () => {
  expect(macKeyBytes(key('ArrowLeft', { altKey: true }), true)).toBe('\x1bb')
  expect(macKeyBytes(key('ArrowRight', { altKey: true }), true)).toBe('\x1bf')
  expect(macKeyBytes(key('ArrowLeft', { metaKey: true }), true)).toBe('\x01')
  expect(macKeyBytes(key('ArrowRight', { metaKey: true }), true)).toBe('\x05')
  expect(macKeyBytes(key('Backspace', { metaKey: true }), true)).toBe('\x15')
})

test('everything else stays with xterm', () => {
  // Not macOS.
  expect(macKeyBytes(key('ArrowLeft', { altKey: true }), false)).toBeNull()
  expect(macKeyBytes(key('Backspace', { metaKey: true }), false)).toBeNull()
  // Keyup, extra or mixed modifiers, bare keys, ⌥⌫ (xterm already sends ESC DEL), unmapped codes.
  expect(macKeyBytes(key('ArrowLeft', { type: 'keyup', altKey: true }), true)).toBeNull()
  expect(macKeyBytes(key('ArrowLeft', { altKey: true, shiftKey: true }), true)).toBeNull()
  expect(macKeyBytes(key('ArrowLeft', { altKey: true, metaKey: true }), true)).toBeNull()
  expect(macKeyBytes(key('ArrowRight', { metaKey: true, ctrlKey: true }), true)).toBeNull()
  expect(macKeyBytes(key('ArrowUp'), true)).toBeNull()
  expect(macKeyBytes(key('ArrowLeft'), true)).toBeNull()
  expect(macKeyBytes(key('Backspace', { altKey: true }), true)).toBeNull()
  expect(macKeyBytes(key('KeyC', { ctrlKey: true }), true)).toBeNull()
  expect(macKeyBytes(key('ArrowUp', { metaKey: true }), true)).toBeNull()
})
