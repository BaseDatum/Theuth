import { expect, test } from 'vitest'
import { window } from './lookahead'

test('window: two viewports ahead, one behind, in the scroll direction', () => {
  expect(window(10_000, 800, 'forward', 100_000)).toEqual([9_200, 12_400])
  expect(window(10_000, 800, 'backward', 100_000)).toEqual([8_400, 11_600])
})

test('window: 1.5 viewports each side at rest', () => {
  expect(window(10_000, 800, null, 100_000)).toEqual([8_800, 12_000])
})

test('window: clamped to the content', () => {
  expect(window(300, 800, 'backward', 100_000)).toEqual([0, 1_900])
  expect(window(99_500, 800, 'forward', 100_000)).toEqual([98_700, 100_000])
  expect(window(0, 800, null, 500)).toEqual([0, 500])
})
