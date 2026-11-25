import { expect, test } from 'vitest'
import { beside } from './beside'

const frame = { w: 1366, h: 1024, right: 24 }
const size = { w: 262, h: 120 }
const card = { x: 600, y: 400, w: 220, h: 150 }

test('right of the card when that side is free', () => {
  expect(beside(card, size, [], frame)).toEqual({ x: 832, y: 400 })
})

test('a neighbour on the right sends it left; neighbours both sides send it below', () => {
  const rightCard = { x: 844, y: 400, w: 220, h: 150 }
  expect(beside(card, size, [rightCard], frame)).toEqual({ x: 326, y: 400 })
  const leftCard = { x: 356, y: 400, w: 220, h: 150 }
  expect(beside(card, size, [rightCard, leftCard], frame)).toEqual({ x: 600, y: 562 })
})

test('the raised cluster counts as the edge; nothing free falls back to a side inside the frame', () => {
  const edge = { x: 1000, y: 400, w: 220, h: 150 }
  expect(beside(edge, size, [], { ...frame, right: 94 })).toEqual({ x: 726, y: 400 })
  const big = { x: 0, y: 0, w: 1366, h: 1024 }
  expect(beside(card, size, [big], frame)).toEqual({ x: 832, y: 400 })
})
