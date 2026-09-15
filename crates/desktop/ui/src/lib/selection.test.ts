import { expect, test } from 'vitest'
import { nextSelection } from './selection'

const order = ['a', 'b', 'c', 'd', 'e']
const none: ReadonlySet<string> = new Set()

test('toggle flips one item; range spans anchor..id in either direction; clear empties', () => {
  const one = nextSelection(none, order, null, 'b', 'toggle')
  expect([...one]).toEqual(['b'])
  expect([...nextSelection(one, order, 'b', 'b', 'toggle')]).toEqual([])
  expect([...nextSelection(one, order, 'b', 'd', 'range')].sort()).toEqual(['b', 'c', 'd'])
  expect([...nextSelection(one, order, 'd', 'b', 'range')].sort()).toEqual(['b', 'c', 'd'])
  // No anchor yet, or an anchor that scrolled out of the list: a range is just a toggle.
  expect([...nextSelection(none, order, null, 'c', 'range')]).toEqual(['c'])
  expect([...nextSelection(one, order, 'gone', 'd', 'range')].sort()).toEqual(['b', 'd'])
  expect(nextSelection(none, order, null, '', 'clear')).toBe(none)
  expect(nextSelection(one, order, 'b', '', 'clear').size).toBe(0)
})
