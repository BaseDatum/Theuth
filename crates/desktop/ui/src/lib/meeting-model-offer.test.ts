import { expect, test, vi } from 'vitest'
import type { MeetingSettings } from '@/rpc/bindings/MeetingSettings'
import { OFFERED_KEY, offerMeetingDefault } from './meeting-model-offer'

const block: MeetingSettings = {
  model: null,
  effort: null,
  summary_model: null,
  template: 'meeting',
  language: 'en',
  live_diarization: true,
  live_tools: [],
  project: null,
  detect: { enabled: true, ignore: [], poll_secs: 2 },
  calendar: { eventkit: false, prebrief: true, poll_secs: 900 },
}

const fakes = () => {
  const store: Record<string, string> = {}
  const storage = { getItem: (k: string) => store[k] ?? null, setItem: (k: string, v: string) => void (store[k] = v) }
  const message = vi.fn<(msg: string, opts: { action: { onClick(): void } }) => void>()
  const write = vi.fn()
  return { store, storage, notify: { message } as never, message, write }
}

test('§6.4: shown once — the toast names the model; "Use it" writes the whole block with that model; a second visit is silent', () => {
  const f = fakes()
  expect(offerMeetingDefault({ block, saved: 'anthropic/claude-haiku-4-5', label: 'Claude Haiku 4.5', write: f.write, storage: f.storage, notify: f.notify })).toBe(true)
  expect(f.message).toHaveBeenCalledWith('Use Claude Haiku 4.5 as the meeting default?', expect.objectContaining({ action: expect.objectContaining({ label: 'Use it' }) }))
  expect(f.store[OFFERED_KEY]).toBeDefined()
  expect(f.write).not.toHaveBeenCalled()
  f.message.mock.calls[0]![1].action.onClick()
  expect(f.write).toHaveBeenCalledWith({ ...block, model: 'anthropic/claude-haiku-4-5' })
  expect(offerMeetingDefault({ block, saved: 'anthropic/claude-haiku-4-5', write: f.write, storage: f.storage, notify: f.notify })).toBe(false)
  expect(f.message).toHaveBeenCalledTimes(1)
})

test('dismissed (no action) is remembered too; nothing to offer when the engine has a model or the form never picked one', () => {
  const f = fakes()
  expect(offerMeetingDefault({ block, saved: 'm', write: f.write, storage: f.storage, notify: f.notify })).toBe(true)
  expect(offerMeetingDefault({ block, saved: 'm', write: f.write, storage: f.storage, notify: f.notify })).toBe(false)
  expect(f.write).not.toHaveBeenCalled()
  const g = fakes()
  expect(offerMeetingDefault({ block: { ...block, model: 'engine' }, saved: 'm', write: g.write, storage: g.storage, notify: g.notify })).toBe(false)
  expect(offerMeetingDefault({ block, saved: null, write: g.write, storage: g.storage, notify: g.notify })).toBe(false)
  expect(g.store[OFFERED_KEY]).toBeUndefined()
  expect(g.message).not.toHaveBeenCalled()
})
