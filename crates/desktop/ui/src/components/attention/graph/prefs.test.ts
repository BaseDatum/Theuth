// 208 §10.2: the stored graph preferences parse defensively.
import { expect, test } from 'vitest'
import { defaultGraphHeight, GRAPH_PREFS_DEFAULT, readGraphPrefs } from './prefs'

test('defaults on null and garbage', () => {
  expect(readGraphPrefs(null)).toEqual(GRAPH_PREFS_DEFAULT)
  expect(readGraphPrefs('not json')).toEqual(GRAPH_PREFS_DEFAULT)
  expect(readGraphPrefs('42')).toEqual(GRAPH_PREFS_DEFAULT)
  expect(readGraphPrefs('{"expanded":"yes","height":"tall","ttl":[]}')).toEqual(GRAPH_PREFS_DEFAULT)
})

test('height clamps; stored TTLs that are not finite, positive and within bounds fall back to defaults (review U9)', () => {
  const p = readGraphPrefs(JSON.stringify({ height: 5000, ttl: { child_min: 0, root_min: 99_999, desk_min: 12.6, edge_min: 1000 } }))
  expect(p.height).toBe(2000)
  expect(p.ttl).toEqual({ ...GRAPH_PREFS_DEFAULT.ttl, desk_min: 13 })
  expect(readGraphPrefs('{"height":10}').height).toBe(160)
  expect(readGraphPrefs('{"ttl":{"child_min":5000}}').ttl.child_min).toBe(GRAPH_PREFS_DEFAULT.ttl.child_min)
  expect(readGraphPrefs('{"ttl":{"child_min":-5,"root_min":"60","edge_min":1e308}}').ttl).toEqual(GRAPH_PREFS_DEFAULT.ttl)
  expect(readGraphPrefs('{"ttl":{"child_min":0.4,"edge_min":240}}').ttl).toEqual({ ...GRAPH_PREFS_DEFAULT.ttl, child_min: 1, edge_min: 240 })
})

test('partial objects keep the rest of the defaults', () => {
  const p = readGraphPrefs('{"expanded":false,"ttl":{"edge_min":7}}')
  expect(p).toEqual({ ...GRAPH_PREFS_DEFAULT, expanded: false, ttl: { ...GRAPH_PREFS_DEFAULT.ttl, edge_min: 7 } })
  expect(readGraphPrefs('{"follow":true,"height":400}')).toEqual({ ...GRAPH_PREFS_DEFAULT, follow: true, height: 400 })
})

test('an unset height follows the viewport: 40 %, at least 280, at most 60 %', () => {
  expect(GRAPH_PREFS_DEFAULT.height).toBeNull()
  expect(readGraphPrefs('{"height":null}').height).toBeNull()
  expect(defaultGraphHeight(900)).toBe(360)
  expect(defaultGraphHeight(600)).toBe(280)
  expect(defaultGraphHeight(400)).toBe(240)
  expect(defaultGraphHeight(1600)).toBe(640)
})
