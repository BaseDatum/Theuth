import { expect, test } from 'vitest'
import type { SessionSummary } from "@/rpc/bindings/method.agent.output"
import type { Usage } from "@/rpc/bindings/event.agent_2eevents.output"
import { runningSubagents } from '@/components/sidebar/SessionItem'
import { createSessions } from './sessions'

const usage: Usage = { prompt: 1, completion: 1, cached: 0 }
const summary = (id: string, running: boolean) => ({ id, running }) as SessionSummary

/** A desk with two subagents, all three mid-turn as far as the events this client saw go. */
function liveStore() {
  const store = createSessions()
  const st = store.getState()
  st.ensure('desk')
  st.applyEvents([
    { type: 'TurnStarted', session: 'desk', turn: 't0', text: 'go', model: 'm', turn_seq: 1 },
    { type: 'SubagentSpawned', parent: 'desk', child: 'a', name: 'a', cap: null },
    { type: 'SubagentSpawned', parent: 'desk', child: 'b', name: 'b', cap: null },
    { type: 'TurnStarted', session: 'a', turn: 't1', text: 'x', model: 'm', turn_seq: 1 },
    { type: 'TurnStarted', session: 'b', turn: 't2', text: 'y', model: 'm', turn_seq: 1 },
  ])
  return store
}
const kids = [summary('a', false), summary('b', false)]

test('142 F4: seedRunning clears the marks the engine says are idle, keeps the one it says runs, leaves unknown sessions alone', () => {
  const store = liveStore()
  let s = store.getState().sessions
  expect(runningSubagents(kids, s)).toBe(2)
  expect(Object.values(s.desk!.children).map((r) => r.state.state)).toEqual(['running', 'running'])

  // The overnight `TurnFinished`s for `desk` and `a` were lost; `b` is still working.
  store.getState().seedRunning([summary('desk', false), summary('a', false), summary('b', true)])
  s = store.getState().sessions
  expect(s.desk).toMatchObject({ running: false, waiting: false, activity: 'idle' })
  expect(s.a).toMatchObject({ running: false, activity: 'idle' })
  expect(s.b).toMatchObject({ running: true, activity: 'thinking' })
  expect(s.desk!.children.a).toMatchObject({ state: { state: 'idle' }, started_at: null })
  expect(s.desk!.children.a!.finished_at).not.toBeNull()
  expect(s.desk!.children.b).toMatchObject({ state: { state: 'running' } })
  expect(runningSubagents(kids, s)).toBe(1)

  // A lost `TurnStarted`: the engine says `a` runs again → marked running; `b` not in the list → untouched.
  const before = s.b
  store.getState().seedRunning([summary('a', true)])
  s = store.getState().sessions
  expect(s.a).toMatchObject({ running: true, waiting: true, activity: 'thinking' })
  expect(s.desk!.children.a).toMatchObject({ state: { state: 'running' } })
  expect(s.b).toBe(before)

  // Nothing to change: the record keeps its identity (no re-render).
  const same = store.getState().sessions
  store.getState().seedRunning([summary('a', true), summary('b', true)])
  expect(store.getState().sessions).toBe(same)
})

test('142 F4: a whole-store markStale drops every running mark; one session leaving focus keeps its marks', () => {
  const store = liveStore()
  store.getState().applyEvents([{ type: 'TurnFinished', session: 'b', turn: 't2', stop_reason: 'stop', usage, context: 1 }])
  store.getState().markStale('a')
  let s = store.getState().sessions
  expect(s.a).toMatchObject({ running: true, activity: 'thinking' })
  expect(s.desk!.children.a).toMatchObject({ state: { state: 'running' } })

  store.getState().markStale()
  s = store.getState().sessions
  expect(s.desk).toMatchObject({ running: false, activity: 'idle' })
  expect(s.a).toMatchObject({ running: false, activity: 'idle' })
  expect(s.desk!.children.a).toMatchObject({ state: { state: 'idle' }, started_at: null })
  // Already finished: left as the event wrote it.
  expect(s.desk!.children.b).toMatchObject({ state: { state: 'idle' } })
  expect(runningSubagents(kids, s)).toBe(0)
})
