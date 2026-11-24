import { readFileSync } from 'node:fs'
import { expect, test } from 'vitest'
import { followViewport } from './viewport-follow'

// A scroller at the bottom: 2000 px of content in a 600 px viewport.
const at = (clientHeight: number, scrollTop = 1400) => ({ clientHeight, scrollHeight: 2000, scrollTop })

test('the "Next:" block takes room under a pinned transcript: the view lands on the bottom again', () => {
  const el = at(520)
  expect(followViewport(el, true, 600)).toBe(520)
  expect(el.scrollTop).toBe(2000) // the browser clamps it to scrollHeight - clientHeight = 1480
})

test('scrolled up, the position is left alone', () => {
  const el = at(520, 300)
  expect(followViewport(el, false, 600)).toBe(520)
  expect(el.scrollTop).toBe(300)
})

test('the block goes (the viewport grows) or nothing changed: no write — the browser keeps the bottom by clamping', () => {
  const grown = at(600, 1480)
  expect(followViewport(grown, true, 520)).toBe(600)
  expect(grown.scrollTop).toBe(1480)
  const same = at(600, 1400)
  expect(followViewport(same, true, 600)).toBe(600)
  expect(same.scrollTop).toBe(1400)
})

test('the transcript observes its own scroller height, not only the column width', () => {
  const src = readFileSync(new URL('./Transcript.tsx', import.meta.url), 'utf8')
  expect(src).toMatch(/followViewport\(el, stuck\.current, last\)/)
  expect(src).toMatch(/ro\.observe\(el\)/)
})
