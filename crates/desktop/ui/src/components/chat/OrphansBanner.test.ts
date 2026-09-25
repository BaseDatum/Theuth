import { expect, test } from 'vitest'
import type { SessionOrphan } from '@/rpc/bindings/SessionOrphan'
import { orphanCount, orphansText } from './OrphansBanner'

const range = (epoch: number, from: number, to: number, at: string): SessionOrphan => ({
  epoch,
  from_seq: BigInt(from),
  to_seq: BigInt(to),
  at,
})

test('orphans banner counts rows across ranges and dates from the oldest one', () => {
  expect(orphansText([], 'laptop')).toBe('')
  expect(orphanCount(range(1, 7, 7, ''))).toBe(1)
  const one = orphansText([range(1, 7, 7, '2026-09-25T10:00:00Z')], 'laptop')
  expect(one).toMatch(/^1 turn from .* was not delivered from laptop$/)
  const many = orphansText(
    [range(3, 20, 24, '2026-09-25T12:00:00Z'), range(1, 7, 9, '2026-09-25T10:00:00Z')],
    'laptop',
  )
  expect(many).toMatch(/^8 turns from .* were not delivered from laptop$/)
  expect(many).toContain(new Date('2026-09-25T10:00:00Z').toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }))
})
