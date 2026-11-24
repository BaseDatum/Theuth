import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { HISTORY_CAP, useNotices } from '@/store/notices'
import { notify } from './notify'

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(1_000_000)
  useNotices.getState().clear()
})
afterEach(() => vi.useRealTimers())

const st = () => useNotices.getState()

test('each entry point maps to its kind', () => {
  const kinds = [
    [notify, 'info'],
    [notify.message, 'info'],
    [notify.info, 'info'],
    [notify.success, 'done'],
    [notify.warning, 'warn'],
    [notify.error, 'error'],
    [notify.loading, 'progress'],
  ] as const
  for (const [fn, kind] of kinds) {
    fn(`as ${kind} ${st().history.length}`)
    expect(st().current?.kind).toBe(kind)
  }
  expect(st().history).toHaveLength(kinds.length)
})

test('the newest takes the slot; the replaced one stays in history', () => {
  notify.error('first')
  notify.success('second')
  expect(st().current?.text).toBe('second')
  expect(st().history.map((n) => n.text)).toEqual(['second', 'first'])
})

test('same id replaces in place: progress → done, one history row', () => {
  const id = notify.loading('Loading…', { id: 'dl' })
  expect(id).toBe('dl')
  notify('unrelated')
  vi.advanceTimersByTime(5_000)
  notify.success('Loaded', { id: 'dl' })
  expect(st().history.map((n) => [n.id, n.kind, n.text])).toEqual([
    ['dl', 'done', 'Loaded'],
    [expect.any(String), 'info', 'unrelated'],
  ])
  expect(st().current?.expiresAt).toBe(Date.now() + 6_000)
})

test('identical text and kind within 2 s bumps the time instead of adding a row', () => {
  notify.warning('Events dropped')
  vi.advanceTimersByTime(1_999)
  notify.warning('Events dropped')
  expect(st().history).toHaveLength(1)
  expect(st().history[0].at).toBe(Date.now())
  notify.error('Events dropped')
  expect(st().history).toHaveLength(2)
  vi.advanceTimersByTime(2_000)
  notify.error('Events dropped')
  expect(st().history).toHaveLength(3)
})

test('default lifetimes, and what is sticky', () => {
  const now = Date.now()
  const life = (fn: () => unknown) => (fn(), st().current && { sticky: st().current!.sticky, expiresAt: st().current!.expiresAt })
  expect(life(() => notify.info('i'))).toEqual({ sticky: false, expiresAt: now + 4_000 })
  expect(life(() => notify.success('d'))).toEqual({ sticky: false, expiresAt: now + 6_000 })
  expect(life(() => notify.warning('w'))).toEqual({ sticky: false, expiresAt: now + 8_000 })
  expect(life(() => notify.error('e'))).toEqual({ sticky: true, expiresAt: null })
  expect(life(() => notify.loading('p'))).toEqual({ sticky: true, expiresAt: null })
  expect(life(() => notify('inf', { duration: Infinity }))).toEqual({ sticky: true, expiresAt: null })
  expect(life(() => notify('act', { action: { label: 'Open', onClick: () => {} } }))).toEqual({ sticky: true, expiresAt: null })
  expect(life(() => notify.error('short', { duration: 2_000 }))).toEqual({ sticky: false, expiresAt: now + 2_000 })
})

test('expire drops the slot only once the lifetime is over', () => {
  notify('hello')
  st().expire(Date.now() + 3_999)
  expect(st().current?.text).toBe('hello')
  st().expire(Date.now() + 4_000)
  expect(st().current).toBeNull()
  expect(st().history).toHaveLength(1)
  notify.error('stuck')
  st().expire(Date.now() + 1e9)
  expect(st().current?.text).toBe('stuck')
})

test('dismiss: no id clears the slot; a dismissed progress leaves history', () => {
  notify.error('boom')
  notify.dismiss()
  expect(st().current).toBeNull()
  expect(st().history).toHaveLength(1)
  notify.loading('Loading voice model…', { id: 'voice' })
  notify.dismiss('voice')
  expect(st().current).toBeNull()
  expect(st().history.map((n) => n.text)).toEqual(['boom'])
  notify('a')
  notify.dismiss('nope')
  expect(st().current?.text).toBe('a')
})

test('unread errors count fresh errors until the history is opened', () => {
  notify.error('one')
  notify.error('two', { id: 'x' })
  notify.error('two again', { id: 'x' })
  notify.warning('meh')
  expect(st().unreadErrors).toBe(2)
  st().markRead()
  expect(st().unreadErrors).toBe(0)
  notify.loading('p', { id: 'y' })
  notify.error('failed', { id: 'y' })
  expect(st().unreadErrors).toBe(1)
  st().clear()
  expect(st()).toMatchObject({ current: null, history: [], unreadErrors: 0 })
})

test('history keeps the newest 50', () => {
  for (let i = 0; i < HISTORY_CAP + 5; i++) notify(`n${i}`)
  expect(st().history).toHaveLength(HISTORY_CAP)
  expect(st().history[0].text).toBe(`n${HISTORY_CAP + 4}`)
})
