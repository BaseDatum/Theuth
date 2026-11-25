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
  expect(st().history[0]?.at).toBe(Date.now())
  notify.error('Events dropped')
  expect(st().history).toHaveLength(2)
  vi.advanceTimersByTime(2_000)
  notify.error('Events dropped')
  expect(st().history).toHaveLength(3)
})

test('dedupe needs the same description and action too, and never touches an id\'d call', () => {
  const open = { label: 'Open', onClick: () => {} }
  notify('Saved', { description: 'a' })
  notify('Saved', { description: 'b' })
  notify('Saved', { description: 'b', action: open })
  notify('Saved', { description: 'b', action: open })
  expect(st().history).toHaveLength(3)
  expect(notify('Saved', { id: 'mine' })).toBe('mine')
  expect(notify('Saved', { id: 'theirs' })).toBe('theirs')
  expect(st().history).toHaveLength(5)
})

test('a superseded notice never returns to the slot; it stays in history with its action', () => {
  const open = vi.fn()
  notify.loading('Downloading speech models… 40%', { id: 'models' })
  notify('cancelled 2 sessions')
  st().expire(Date.now() + 4_000)
  expect(st().current).toBeNull()
  notify.error('boom', { action: { label: 'Open', onClick: open } })
  notify('hello')
  st().expire(Date.now() + 4_000)
  expect(st().current).toBeNull()
  const boom = st().history.find((n) => n.text === 'boom')!
  boom.action!.onClick()
  expect(open).toHaveBeenCalledOnce()
  // A progress update is current state again: it takes the slot.
  notify.loading('Downloading speech models… 41%', { id: 'models' })
  expect(st().current?.id).toBe('models')
})

test('default lifetimes: every notice leaves the slot; held ones (action, Infinity, progress) carry a ×', () => {
  const now = Date.now()
  const life = (fn: () => unknown) => (fn(), st().current && { sticky: st().current!.sticky, expiresAt: st().current!.expiresAt })
  expect(life(() => notify.info('i'))).toEqual({ sticky: false, expiresAt: now + 4_000 })
  expect(life(() => notify.success('d'))).toEqual({ sticky: false, expiresAt: now + 6_000 })
  expect(life(() => notify.warning('w'))).toEqual({ sticky: false, expiresAt: now + 8_000 })
  expect(life(() => notify.error('e'))).toEqual({ sticky: false, expiresAt: now + 8_000 })
  expect(life(() => notify.loading('p'))).toEqual({ sticky: true, expiresAt: now + 30_000 })
  expect(life(() => notify('inf', { duration: Infinity }))).toEqual({ sticky: true, expiresAt: now + 10_000 })
  expect(life(() => notify('act', { action: { label: 'Open', onClick: () => {} } }))).toEqual({ sticky: true, expiresAt: now + 10_000 })
  expect(life(() => notify.error('short', { duration: 2_000 }))).toEqual({ sticky: false, expiresAt: now + 2_000 })
  expect(life(() => notify('long', { duration: 60_000 }))).toEqual({ sticky: false, expiresAt: now + 10_000 })
})

test('expire drops the slot only once the lifetime is over; an error does not linger', () => {
  notify('hello')
  st().expire(Date.now() + 3_999)
  expect(st().current?.text).toBe('hello')
  st().expire(Date.now() + 4_000)
  expect(st().current).toBeNull()
  expect(st().history).toHaveLength(1)
  // The owner's report: a call that failed during a reconnect said "not connected", and it stayed in the slot.
  notify.error('not connected')
  st().expire(Date.now() + 7_999)
  expect(st().current?.text).toBe('not connected')
  st().expire(Date.now() + 8_000)
  expect(st().current).toBeNull()
  expect(st().unreadErrors).toBe(1)
})

test('progress holds the slot while updated, and leaves 30 s after its last update', () => {
  notify.loading('Downloading… 10%', { id: 'dl' })
  vi.advanceTimersByTime(25_000)
  notify.loading('Downloading… 90%', { id: 'dl' })
  st().expire(Date.now() + 29_999)
  expect(st().current?.text).toBe('Downloading… 90%')
  st().expire(Date.now() + 30_000)
  expect(st().current).toBeNull()
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
  expect(st().history[0]?.text).toBe(`n${HISTORY_CAP + 4}`)
})
