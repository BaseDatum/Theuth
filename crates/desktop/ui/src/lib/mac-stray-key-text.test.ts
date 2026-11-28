import '@/components/settings/test-dom'
import { afterEach, describe, expect, test, vi } from 'vitest'

const mockContainer = vi.fn(() => 'macos')
vi.mock('@/lib/platform', () => ({ container: () => mockContainer() }))

import { installMacStrayKeyGuard, isStrayControlText, stripStrayControlText } from './mac-stray-key-text'

afterEach(() => {
  mockContainer.mockReturnValue('macos')
})

describe('mac-stray-key-text', () => {
  test('recognises C0 arrow/Home/End control chars, DEL and Apple NSFunctionKey PUA chars', () => {
    for (const ch of ['\u001c', '\u001d', '\u001e', '\u001f', '\u0001', '\u0004', '\u007f', '\uF702', '\uF703', '\uF700', '\uF74F']) {
      expect(isStrayControlText(ch)).toBe(true)
    }
    expect(isStrayControlText('\u001c\u001d')).toBe(true)
  })

  test('keeps normal text, accents, emoji, tabs, newlines and mixed strings', () => {
    for (const s of ['', null, undefined, 'a', 'é', '🎉', '\t', '\n', '\r', '\r\n', 'a\u001cb']) { // no-emoji: allow
      expect(isStrayControlText(s)).toBe(false)
    }
    expect(stripStrayControlText('hi\u001c\t\nthere\uF703')).toBe('hi\t\nthere')
  })

  test('cancels stray insertText in capture phase on macOS desktop and leaves legitimate edits alone', () => {
    const off = installMacStrayKeyGuard(window)
    const ta = document.createElement('textarea')
    document.body.appendChild(ta)
    try {
      const fire = (inputType: string, data: string | null) => {
        const ev = new InputEvent('beforeinput', { bubbles: true, cancelable: true, inputType, data })
        ta.dispatchEvent(ev)
        return ev.defaultPrevented
      }

      expect(fire('insertText', '\u001c')).toBe(true)
      expect(fire('insertText', '\u001d')).toBe(true)
      expect(fire('insertText', '\uF702')).toBe(true)
      expect(fire('insertReplacementText', '\u0001')).toBe(true)

      expect(fire('insertText', 'x')).toBe(false)
      expect(fire('insertText', '\n')).toBe(false)
      expect(fire('insertText', '\t')).toBe(false)
      expect(fire('insertFromPaste', '\u001c')).toBe(false)
      expect(fire('insertFromDrop', '\u001c')).toBe(false)
      expect(fire('insertCompositionText', '\u001c')).toBe(false)
      expect(fire('deleteContentBackward', null)).toBe(false)
    } finally {
      off()
      ta.remove()
    }
  })

  test('strips stray chars in input fallback and keeps caret position', () => {
    const off = installMacStrayKeyGuard(window)
    const ta = document.createElement('textarea')
    document.body.appendChild(ta)
    try {
      ta.value = 'ab\u001ccd'
      ta.setSelectionRange(3, 3)
      ta.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: '\u001c' }))
      expect(ta.value).toBe('abcd')
      expect(ta.selectionStart).toBe(2)
      expect(ta.selectionEnd).toBe(2)
    } finally {
      off()
      ta.remove()
    }
  })

  test('does not install off macOS desktop', () => {
    mockContainer.mockReturnValue('web')
    const off = installMacStrayKeyGuard(window)
    const ta = document.createElement('textarea')
    document.body.appendChild(ta)
    try {
      const ev = new InputEvent('beforeinput', { bubbles: true, cancelable: true, inputType: 'insertText', data: '\u001c' })
      ta.dispatchEvent(ev)
      expect(ev.defaultPrevented).toBe(false)
    } finally {
      off()
      ta.remove()
    }
  })
})
