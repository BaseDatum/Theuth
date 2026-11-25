import { describe, expect, test } from 'vitest'
import { isFullSwipe, swipeAxis } from './swipe'

describe('swipeAxis', () => {
  test('waits through the initial touch slop', () => {
    expect(swipeAxis(8, 8)).toBeNull()
    expect(swipeAxis(-12, 12)).toBeNull()
  })

  test('accepts horizontal swipes with substantial vertical drift', () => {
    expect(swipeAxis(40, 30)).toBe('x')
    expect(swipeAxis(40, 60)).toBe('x')
    expect(swipeAxis(-40, -60)).toBe('x')
  })

  test('leaves clearly vertical movement to scrolling', () => {
    expect(swipeAxis(8, 20)).toBe('y')
    expect(swipeAxis(40, 61)).toBe('y')
    expect(swipeAxis(-40, -61)).toBe('y')
  })
})

describe('isFullSwipe', () => {
  test('does not fire for a partial swipe, including the old 88px threshold', () => {
    expect(isFullSwipe(88, 400)).toBe(false)
    expect(isFullSwipe(399, 400)).toBe(false)
    expect(isFullSwipe(-399, 400)).toBe(false)
  })

  test('fires at one full card width in either direction', () => {
    expect(isFullSwipe(400, 400)).toBe(true)
    expect(isFullSwipe(440, 400)).toBe(true)
    expect(isFullSwipe(-400, 400)).toBe(true)
  })

  test('never fires when the card has no measurable width', () => {
    expect(isFullSwipe(100, 0)).toBe(false)
  })
})
