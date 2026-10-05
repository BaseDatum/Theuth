import { expect, test } from 'vitest'
import type { AgentInfo } from '@/rpc/extra'
import { LINGER_MS, NO_LINGERING, linger, liveRows } from './sessions'

const row = (id: string, state: AgentInfo['state']['state'], started_at: string | null = null): AgentInfo => ({
  id,
  name: id,
  parent: 'desk',
  depth: 1,
  agent: null,
  model: 'm',
  state: { state } as AgentInfo['state'],
  started_at,
  finished_at: null,
  last: '',
})
const rows = (...list: AgentInfo[]): Record<string, AgentInfo> => Object.fromEntries(list.map((r) => [r.id, r]))

test('liveRows keeps running/awaiting/approval rows in spawn order: started_at ascending, unknown last, name tiebreak', () => {
  const children = rows(
    row('b-late', 'running', '2026-01-01T00:00:09Z'),
    row('idle', 'idle', '2026-01-01T00:00:01Z'),
    row('err', 'error', '2026-01-01T00:00:01Z'),
    row('done', 'cancelled', '2026-01-01T00:00:01Z'),
    row('z-unknown', 'awaiting'),
    row('a-unknown', 'approval'),
    row('a-early', 'running', '2026-01-01T00:00:02Z'),
  )
  expect(liveRows(children).map((r) => r.id)).toEqual(['a-early', 'b-late', 'a-unknown', 'z-unknown'])
  expect(liveRows({})).toEqual([])
})

test('linger: a row that leaves is kept with its leave time, dropped once LINGER_MS has passed, un-marked when it returns', () => {
  const t0 = 1_000
  const a = linger(NO_LINGERING, ['x', 'y'], t0)
  expect(a).toEqual({ ids: ['x', 'y'], leaving: {} })

  const b = linger(a, ['y'], t0 + 100)
  expect(b).toEqual({ ids: ['x', 'y'], leaving: { x: t0 + 100 } })

  // Still within the window: listed, leave time unchanged.
  const c = linger(b, ['y'], t0 + 100 + LINGER_MS - 1)
  expect(c).toEqual({ ids: ['x', 'y'], leaving: { x: t0 + 100 } })

  // Window over: dropped. A new live row appends after the survivors.
  const d = linger(c, ['y', 'z'], t0 + 100 + LINGER_MS)
  expect(d).toEqual({ ids: ['y', 'z'], leaving: {} })

  // Came back before the window closed: un-marked, position kept.
  const e = linger(b, ['x', 'y'], t0 + 200)
  expect(e).toEqual({ ids: ['x', 'y'], leaving: {} })
})

test('linger: empty in, empty out; the same object comes back when nothing changed', () => {
  expect(linger(NO_LINGERING, [], 5)).toBe(NO_LINGERING)
  const a = linger(NO_LINGERING, ['x'], 5)
  expect(linger(a, ['x'], 6)).toBe(a)
  const b = linger(a, [], 10)
  expect(linger(b, [], 10 + LINGER_MS - 1)).toBe(b)
})
