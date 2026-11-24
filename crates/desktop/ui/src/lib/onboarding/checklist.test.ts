import { expect, test } from 'vitest'
import type { SignalStatus } from '@/rpc/bindings/method.signal.output'
import type { SlackStatus } from '@/rpc/bindings/method.slack.output'
import type { TelegramStatus } from '@/rpc/bindings/method.telegram.output'
import { CHECKLIST_ITEMS, type ChecklistInputs, checklistDone, checklistView, phoneLinked, progressLabel } from './checklist'

const NONE: ChecklistInputs = { models: undefined, sessions: 0, visited: {}, devices: undefined }
const slack = (type: string | null) => ({ state: type ? { type } : null }) as unknown as SlackStatus
const telegram = (type: string | null) => ({ state: type ? { type } : null }) as unknown as TelegramStatus
const signal = (registered: boolean, type: string | null = null) => ({ registered, state: type ? { type } : null }) as unknown as SignalStatus

test('the contract ids, in order, channel optional', () => {
  expect(CHECKLIST_ITEMS.map((i) => i.id)).toEqual(['model', 'chat', 'inbox', 'costs', 'library', 'phone', 'channel'])
  expect(CHECKLIST_ITEMS.filter((i) => i.optional).map((i) => i.id)).toEqual(['channel'])
})

test('nothing is done for a fresh install', () => {
  expect(Object.values(checklistDone(NONE)).some(Boolean)).toBe(false)
  expect(checklistDone({ ...NONE, models: 0 }).model).toBe(false)
})

test('each item completes from its own source', () => {
  const d = checklistDone({ models: 3, sessions: 1, visited: { inbox: true, costs: true, library: true }, devices: [{ platform: 'ios', revoked: false }], slack: slack('Connected') })
  expect(d).toEqual({ model: true, chat: true, inbox: true, costs: true, library: true, phone: true, channel: true })
})

test('phone: a revoked device or another node does not count', () => {
  expect(phoneLinked([{ platform: 'ios', revoked: true }, { platform: 'theuth', revoked: false }])).toBe(false)
  expect(phoneLinked([{ platform: 'web', revoked: false }])).toBe(true)
  expect(phoneLinked(undefined)).toBe(false)
})

test('channel: connected or registered on any of the three', () => {
  const ch = (x: Partial<ChecklistInputs>) => checklistDone({ ...NONE, ...x }).channel
  expect(ch({ slack: slack('Unconfigured'), telegram: telegram('Disconnected'), signal: signal(false, 'Unlinked') })).toBe(false)
  expect(ch({ slack: slack('StreamFallback') })).toBe(true)
  expect(ch({ telegram: telegram('Identity') })).toBe(true)
  expect(ch({ telegram: telegram('Unconfigured') })).toBe(false)
  expect(ch({ signal: signal(true) })).toBe(true)
  expect(ch({ signal: signal(false, 'Linked') })).toBe(true)
  expect(ch({ slack: null, telegram: null, signal: null })).toBe(false)
})

test('view: unavailable items drop out of the count; complete only when every shown item is done', () => {
  const done = checklistDone({ models: 1, sessions: 2, visited: { inbox: true, costs: true, library: true }, devices: [] })
  const all = checklistView(done)
  expect(progressLabel(all)).toBe('5 of 7')
  expect(all.complete).toBe(false)
  const noLink = checklistView(done, { phone: false, channel: false })
  expect(noLink.rows.map((r) => r.item.id)).toEqual(['model', 'chat', 'inbox', 'costs', 'library'])
  expect(progressLabel(noLink)).toBe('5 of 5')
  expect(noLink.complete).toBe(true)
  expect(checklistView(done, Object.fromEntries(CHECKLIST_ITEMS.map((i) => [i.id, false]))).complete).toBe(false)
})
