import { expect, test } from 'vitest'
import { type QuitEvent, type QuitSlice, type QuitState, type QuitSummary, isolate, quitSteps, quitTitle, reduce } from './quit-progress'

const AT = 1_000
const row = (id: string, over: Partial<QuitSummary> = {}): QuitSummary => ({
  id,
  title: `chat ${id}`,
  name: null,
  parent: null,
  running: false,
  archived: false,
  ...over,
})
const slice = (over: Partial<QuitSlice> = {}): QuitSlice => ({ running: true, ...over })
const suspended = (at: number): QuitSlice['resume'] => ({ turn: 't', kind: 'suspended', at, resumed: false, here: true })

const requested = (over: Partial<Extract<QuitEvent, { type: 'requested' }>> = {}): QuitState => {
  const s = reduce(null, { type: 'requested', restart: false, owned: true, at: AT, slices: {}, summaries: [], ...over })
  if (!s) throw new Error('requested gave no state')
  return s
}
const run = (s: QuitState, ...events: QuitEvent[]): QuitState => events.reduce<QuitState>((acc, e) => reduce(acc, e) ?? acc, s)
const status = (s: QuitState) => Object.fromEntries(quitSteps(s).map((st) => [st.id, st.status]))

test('two running chats: listed by title, ticked as their turns finish or are suspended, the bar until both are', () => {
  let s = requested({
    slices: { a: slice({ title: 'Refactor the parser' }), idle: slice({ running: false }) },
    summaries: [row('b', { running: true, title: 'Weekly report' }), row('idle', { running: true })],
  })
  expect(quitTitle(s)).toBe('Shutting down theuth')
  let saving = quitSteps(s)[0]!
  expect(saving).toMatchObject({ id: 'saving', label: 'Saving your conversations', status: 'active', bar: true })
  // The slice is the truth for its session: `idle`'s list row is stale.
  expect(saving.items).toEqual([
    { id: 'b', label: 'Weekly report', done: false },
    { id: 'a', label: 'Refactor the parser', done: false },
  ])

  s = run(s, { type: 'slices', slices: { a: slice({ title: 'Refactor the parser', running: false }) } })
  saving = quitSteps(s)[0]!
  expect(saving.items?.map((i) => i.done)).toEqual([false, true])
  expect(saving.status).toBe('active')

  // `TurnSuspended` keeps `running` but opens a suspended banner after the request.
  s = run(s, { type: 'slices', slices: { b: slice({ resume: suspended(AT + 5) }) } })
  expect(quitSteps(s)[0]).toMatchObject({ status: 'done', bar: false })
})

test('a suspension from before the quit, subagents and archived chats do not count', () => {
  let s = requested({
    slices: { old: slice({ resume: suspended(AT - 50) }), child: slice({ parent: 'old' }) },
    summaries: [row('gone', { running: true, archived: true }), row('kid', { running: true, parent: 'old' })],
  })
  expect(quitSteps(s)[0]!.items?.map((i) => i.id)).toEqual(['old'])
  s = run(s, { type: 'slices', slices: { old: slice({ resume: suspended(AT - 50) }) } })
  expect(quitSteps(s)[0]!.items?.[0]!.done).toBe(false)
})

test('nothing running: saving is ticked at once with "Nothing to save"', () => {
  const s = requested({ summaries: [row('x')] })
  expect(quitSteps(s)[0]).toMatchObject({ status: 'done', detail: 'Nothing to save', items: undefined, bar: false })
  expect(status(s)).toEqual({ saving: 'done', engine: 'pending', closing: 'pending' })
})

test('a reused engine keeps running: no turns, no engine step', () => {
  const s = requested({ owned: false, summaries: [row('a', { running: true })] })
  expect(quitSteps(s)[0]).toMatchObject({ status: 'done', detail: 'The engine keeps running', items: undefined })
  expect(quitSteps(run(s, { type: 'progress', step: 'engine' }, { type: 'progress', step: 'closing' })).map((st) => st.id)).toEqual(['saving', 'closing'])
})

test('worker steps: the companion only when one runs, the engine until closing, closing last', () => {
  let s = requested({ summaries: [row('a', { running: true })] })
  expect(status(s)).toEqual({ saving: 'active', engine: 'pending', closing: 'pending' })
  s = run(s, { type: 'progress', step: 'companion' })
  expect(quitSteps(s).map((st) => st.label)).toEqual(['Saving your conversations', 'Closing the companion', 'Stopping the engine', 'Closing'])
  expect(status(s)).toEqual({ saving: 'active', companion: 'active', engine: 'pending', closing: 'pending' })
  s = run(s, { type: 'progress', step: 'engine' })
  expect(status(s)).toEqual({ saving: 'active', companion: 'done', engine: 'active', closing: 'pending' })
  // The engine has exited: whatever was still draining is over.
  s = run(s, { type: 'progress', step: 'closing' })
  expect(status(s)).toEqual({ saving: 'done', companion: 'done', engine: 'done', closing: 'active' })
  expect(quitSteps(s)[0]!.items?.every((i) => i.done)).toBe(true)
})

test('a forced engine stop says so', () => {
  const s = run(requested(), { type: 'progress', step: 'engine' }, { type: 'progress', step: 'engine', forced: true }, { type: 'progress', step: 'closing' })
  expect(quitSteps(s).find((st) => st.id === 'engine')).toMatchObject({ status: 'done', detail: 'stopped after the time limit' })
})

test('restart: the title, also when the shell request follows the webview own one', () => {
  expect(quitTitle(requested({ restart: true }))).toBe('Restarting to update')
  const own = requested({ summaries: [row('a', { running: true })] })
  const s = run(own, { type: 'requested', restart: true, owned: true, at: AT + 9, slices: {}, summaries: [] })
  expect(quitTitle(s)).toBe('Restarting to update')
  expect(s.turns).toBe(own.turns)
})

test('nothing happens before a request', () => {
  expect(reduce(null, { type: 'progress', step: 'closing' })).toBeNull()
  expect(reduce(null, { type: 'slices', slices: {} })).toBeNull()
})

test('isolate: every sibling but the overlay goes inert; already-inert ones are not taken over for the undo', () => {
  const el = (name: string, inert = false) => ({ name, inert })
  const [overlay, app, dialog, hidden] = [el('overlay'), el('app'), el('dialog'), el('hidden', true)]
  const names = (xs: { name: string }[]) => xs.map((x) => x.name)
  expect(names(isolate([app, overlay, dialog, hidden], overlay))).toEqual(['app', 'dialog'])
  expect([app.inert, dialog.inert, hidden.inert, overlay.inert]).toEqual([true, true, true, false])
  // A portal that opens later: only the newcomer changes.
  const popover = el('popover')
  expect(names(isolate([app, overlay, dialog, popover], overlay))).toEqual(['popover'])
})
