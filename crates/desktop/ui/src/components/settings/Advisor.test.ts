import { expect, test } from 'vitest'
import type { SyncBacklog } from '@/rpc/bindings/SyncBacklog'
import { BACKLOGS, CONSULTS, IMMUNE_RANGE, NOTES_RANGE, asBacklog, asConsults, backlogLabel, consultOptions, consultsLabel } from './Advisor'
import { bounded } from './shared'

test('notes per update accepts 1–32, immune turns 0–10; anything else leaves the field alone', () => {
  expect(bounded('1', ...NOTES_RANGE)).toBe(1)
  expect(bounded('32', ...NOTES_RANGE)).toBe(32)
  expect(bounded('4.9', ...NOTES_RANGE)).toBe(4)
  expect(bounded('0', ...NOTES_RANGE)).toBeUndefined()
  expect(bounded('33', ...NOTES_RANGE)).toBeUndefined()
  expect(bounded('abc', ...NOTES_RANGE)).toBeUndefined()
  expect(bounded('0', ...IMMUNE_RANGE)).toBe(0)
  expect(bounded('10', ...IMMUNE_RANGE)).toBe(10)
  expect(bounded('11', ...IMMUNE_RANGE)).toBeUndefined()
  expect(bounded('-1', ...IMMUNE_RANGE)).toBeUndefined()
})

test('SyncBacklog round-trips through the Catch-up select', () => {
  const all: SyncBacklog[] = ['off', '1', '3', '5']
  expect([...BACKLOGS]).toEqual(all)
  for (const v of all) expect(asBacklog(v)).toBe(v)
  expect(asBacklog('7')).toBeUndefined()
  expect(asBacklog('')).toBeUndefined()
  expect(backlogLabel('off')).toBe('Off')
  expect(backlogLabel('3')).toBe('3')
})

test('R1: Consults per turn offers Off · 2 · 6 · 12 (plus an off-list live value), labelled, and parses back to the cap', () => {
  expect([...CONSULTS]).toEqual([0, 2, 6, 12])
  expect(CONSULTS.map(consultsLabel)).toEqual(['Off', '2', '6', '12'])
  expect(consultOptions(6)).toEqual([0, 2, 6, 12])
  expect(consultOptions(undefined)).toEqual([0, 2, 6, 12])
  expect(consultOptions(32)).toEqual([0, 2, 6, 12, 32])
  for (const v of CONSULTS) expect(asConsults(String(v))).toBe(v)
  expect(asConsults('32')).toBe(32)
  expect(asConsults('33')).toBeUndefined()
  expect(asConsults('-1')).toBeUndefined()
  expect(asConsults('')).toBeUndefined()
})
