import { beforeEach, expect, test } from 'vitest'
import type { PendingReminderView, TerminalSummary } from '@/rpc/bindings/method.agent.output'
import { active } from '@/store/engine-scope'
import { dispatch, useSessions } from '@/store/sessions'
import { relistReminders, relistTerminals } from './background'

beforeEach(() => useSessions.setState({ sessions: {} }))
const reminder: PendingReminderView = { id: 'r1', message: 'survives reload', set_at: 1, fires_at: 61 }

test('reload hydrates details; events racing a snapshot cannot resurrect a fired reminder or lose a new one', async () => {
  const { promise, resolve } = Promise.withResolvers<PendingReminderView[]>()
  const loaded = relistReminders({ list_reminders: () => promise }, active().sessions, 's1', new AbortController().signal)
  dispatch([
    { key: 'tools.reminders@1', event: { type: 'ReminderFired', session: 's1', id: 'r1' } },
    { key: 'tools.reminders@1', event: { type: 'ReminderSet', session: 's1', id: 'r2', message: 'new reminder', set_at: 2, fires_at: 62, seconds: 60 } },
  ])
  resolve([reminder])
  await loaded
  const slice = useSessions.getState().sessions.s1!
  expect([...slice.reminders]).toEqual(['r2'])
  expect([...slice.reminderDetails.values()]).toEqual([{ id: 'r2', message: 'new reminder', set_at: 2, fires_at: 62 }])
  await relistReminders({ list_reminders: async () => [reminder] }, active().sessions, 's1', new AbortController().signal)
  expect([...useSessions.getState().sessions.s1!.reminderDetails.values()]).toEqual([reminder])
})

test('unmounted or switched sessions ignore late replies; a failed read keeps known details', async () => {
  const store = active().sessions
  store.getState().setReminders('s1', [reminder])
  const ctl = new AbortController()
  ctl.abort()
  await relistReminders({ list_reminders: async () => [] }, store, 's1', ctl.signal)
  await expect(relistReminders({ list_reminders: async () => { throw new Error('offline') } }, store, 's1', new AbortController().signal)).rejects.toThrow('offline')
  expect(store.getState().sessions.s1?.reminderDetails.get('r1')).toEqual(reminder)
})

test('quiet terminals hydrate as running; racing lifecycle events do not replay transcript output', async () => {
  const { promise, resolve } = Promise.withResolvers<TerminalSummary[]>()
  const store = active().sessions
  const loaded = relistTerminals({ list_terminals: () => promise }, store, 's1', new AbortController().signal)
  dispatch([
    { key: 'tools.jobs@1', event: { type: 'JobFinished', session: 's1', id: 'j1', command: 'old', kind: 'pty', exit: 'killed', secs: 1, output: '', inline: false, pty: true } },
    { key: 'tools.jobs@1', event: { type: 'JobStarted', session: 's1', id: 'j2', command: 'new', kind: 'pty', call_id: 'c2', log: '/log', name: null, pty: true } },
  ])
  const entries = store.getState().sessions.s1?.entries
  resolve([
    { session: 's1', id: 'j1', command: 'old', state: 'running', name: null, rows: 40, cols: 120, revision: 1, epoch: null },
    { session: 's1', id: 'quiet', command: 'python', state: 'running', name: null, rows: 40, cols: 120, revision: 1, epoch: null },
    { session: 's1', id: 'done', command: 'exit', state: 'exited 0', name: null, rows: 40, cols: 120, revision: 1, epoch: null },
  ])
  await loaded
  expect([...store.getState().sessions.s1!.terminals]).toEqual(['quiet', 'j2'])
  expect(store.getState().sessions.s1?.entries).toBe(entries)
})
