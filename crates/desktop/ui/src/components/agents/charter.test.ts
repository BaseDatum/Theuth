import { expect, test } from 'vitest'
import { marker, parseLine } from '@/lib/attach'
import { charterBinding, charterRef, charterTitle } from './charter'

test('the binding is the same object for the same name, and the engine key for it', () => {
  expect(charterBinding('ops')).toEqual({ agent_charter: 'ops' })
  expect(charterBinding('ops')).toEqual(charterBinding('ops'))
  expect(charterBinding('ops')).not.toEqual(charterBinding('ops2'))
  expect(charterTitle('ops')).toBe('Charter · ops')
})

test('the pinned reference survives the marker grammar with its context intact', () => {
  const ref = charterRef('abcdefghijklmnopqrstuvwxyz', {
    name: 'ops',
    title: 'Ops lead',
    reports_to: 'theuth',
    capabilities: 'incidents [sev1]\n  and paging',
  })
  expect(ref.kind).toBe('note')
  expect(ref.title).toBe('Charter · ops')
  expect(ref.meta).toContain('agent ops (Ops lead, reports to theuth; hand off to it when: incidents (sev1) and paging)')
  expect(ref.meta).toContain("keep the user's words")
  expect(ref.meta).not.toMatch(/[\]\n]/)
  expect(parseLine(marker(ref))).toEqual(ref)
  // No title, no capabilities.
  const bare = charterRef('x', { name: 'qa', title: ' ', reports_to: 'ops', capabilities: null })
  expect(bare.meta).toContain('agent qa (untitled, reports to ops).')
  expect(parseLine(marker(bare))).toEqual(bare)
})
