import { readFileSync } from 'node:fs'
import { isValidElement, type ReactElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { Usage } from '@/rpc/bindings/event.agent_2eevents.output'
import type { AgentEvent as ReceiverAgentEvent } from '@/rpc/bindings/event.agent_2eevents.receiver'
import { receiveTopic } from '@/rpc/bindings/receive'
import { apply as applyWire, empty, type Ctx } from '@/store/reduce'
import type { SessionSlice } from '@/store/sessions'
import { useSessions } from '@/store/sessions'
import type { UiEvent as WireEvent } from '@/store/ui-events'
import { NextSteps, type NextStepsGate, nextSteps, useNextPick } from './NextSteps'

const S = 'sess'
const T = 'turn'
const usage: Usage = { prompt: 10, completion: 5, cached: 0 }
const ctx = (): Ctx => ({ sent: new Set(), now: 0 })
const apply = (s: SessionSlice, e: ReceiverAgentEvent) => applyWire(s, receiveTopic('agent.events@1', e) as WireEvent, ctx())
const ITEMS = ['Add a test for the empty case', 'Show me the diff']

const started = (s: SessionSlice) => apply(s, { type: 'TurnStarted', session: S, turn: T, text: 'hi', model: 'm', turn_seq: 1 })
const finished = (s: SessionSlice) => apply(s, { type: 'TurnFinished', session: S, turn: T, stop_reason: 'stop', usage, context: 1 })
const suggested = (s: SessionSlice, items = ITEMS) => apply(s, { type: 'Suggestions', session: S, turn: T, items })

test('Suggestions after a turn land on the slice without a transcript row; the next turn clears them', () => {
  const done = finished(started(empty(S)))
  const s = suggested(done)
  expect(s.suggestions).toEqual(ITEMS)
  expect(s.entries).toBe(done.entries)
  expect(s.live).toBe(done.live)
  expect(started(s).suggestions).toBeUndefined()
})

test('Suggestions that arrive while a turn runs are stale and dropped; an empty list changes nothing', () => {
  const running = started(empty(S))
  expect(suggested(running)).toBe(running)
  const idle = finished(running)
  expect(suggested(idle, [])).toBe(idle)
})

test('TurnResumed (a turn running again after a restart) clears them too', () => {
  const s = suggested(finished(started(empty(S))))
  const resumed = apply(s, {
    type: 'TurnResumed',
    session: S,
    turn: T,
    kind: 'interrupted',
    interrupted_at: new Date(0).toISOString(),
    resumed_at: new Date(0).toISOString(),
    partial_dropped: false,
  } as ReceiverAgentEvent)
  expect(resumed.suggestions).toBeUndefined()
})

const gate = (g: Partial<NextStepsGate>): NextStepsGate => ({ suggestions: ITEMS, running: false, ...g })

test('the block shows for an idle root chat and hides while a turn runs, on watch-only, advisor and child chats', () => {
  expect(nextSteps(gate({}))).toEqual(ITEMS)
  expect(nextSteps(gate({ kind: 'ask' }))).toEqual(ITEMS)
  expect(nextSteps(gate({ running: true }))).toBeNull()
  expect(nextSteps(gate({ watching: true }))).toBeNull()
  expect(nextSteps(gate({ kind: 'observed' }))).toBeNull()
  expect(nextSteps(gate({ kind: 'advisor' }))).toBeNull()
  expect(nextSteps(gate({ parent: 'p' }))).toBeNull()
  expect(nextSteps(gate({ suggestions: [] }))).toBeNull()
  expect(nextSteps(gate({ suggestions: undefined }))).toBeNull()
})

test('typing, a held paste or a parked skill keeps them: the gate has no draft input, and the slice keeps them', () => {
  // Composer state never reaches the gate; only a send (dismissSuggestions) or a turn start drops them.
  expect(Object.keys(gate({}))).toEqual(['suggestions', 'running'])
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  expect(src).not.toMatch(/drafting:/)
  const s = suggested(finished(started(empty(S))))
  const queued = apply(s, { type: 'InputQueued', session: S, id: 'q', text: 'later', defer: true })
  expect(queued.suggestions).toEqual(ITEMS)
})

/** Every element of a rendered function-component tree, depth first (no DOM in these tests). */
function elements(node: ReactNode): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(elements)
  if (!isValidElement<Record<string, unknown>>(node)) return []
  return [node, ...elements(node.props.children as ReactNode)]
}

test('the block: a faint "Next:" label and one plain text row per suggestion, full text as the tooltip', () => {
  const html = renderToStaticMarkup(NextSteps({ items: ITEMS, onPick: () => {}, hint: true }))
  expect(html).toContain('>Next:<')
  expect(html).not.toContain('What&#x27;s next?')
  for (const item of ITEMS) expect(html).toContain(`title="${item}"`)
  expect(html.match(/<button/g)).toHaveLength(2)
  // No filled bubble: no border, no resting background; hover/focus tints and the text truncates on one line.
  expect(html).not.toMatch(/class="[^"]*\bborder\b/)
  expect(html).not.toMatch(/class="[^"]*(?<!:)bg-layer-2/)
  expect(html).toContain('hover:bg-foreground/6')
  expect(html).toContain('focus-visible:bg-foreground/6')
  expect(html).toContain('truncate')
  expect(html.match(/↵ send/g)).toHaveLength(2)
  expect(html).toContain('opacity-0')
})

test('the ↵ send hint is never rendered on touch', () => {
  const html = renderToStaticMarkup(NextSteps({ items: ITEMS, onPick: () => {}, hint: false }))
  expect(html).not.toContain('↵ send')
})

test('clicking a chip hands its exact text to the pick handler', () => {
  const onPick = vi.fn()
  const buttons = elements(NextSteps({ items: ITEMS, onPick })).filter((e) => e.type === 'button')
  ;(buttons[1]?.props.onClick as () => void)()
  expect(onPick).toHaveBeenCalledWith('Show me the diff')
})

test('a pick is handed to the session composer once', () => {
  useNextPick.getState().pick(S, 'Show me the diff')
  expect(useNextPick.getState().take(S)).toBe('Show me the diff')
  expect(useNextPick.getState().take(S)).toBeUndefined()
})

test('the composer sends a pick at once through its own submit, and any send drops the suggestions', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const effect = src.slice(src.indexOf('const item = useNextPick.getState().take(session)'), src.indexOf('const item = useNextPick.getState().take(session)') + 200)
  expect(effect).toContain('void submit(undefined, false, item)')
  expect(src).toContain('const line = spoken ?? typed ?? text.trim()')
  // Every submit (typed, spoken or a pick) dismisses before it sends.
  const submit = src.slice(src.indexOf('const submit = async'), src.indexOf('const submit = async') + 900)
  expect(submit).toContain('dismissSuggestions(session)')
})

test('the chat route puts the block above the background area and the composer', () => {
  const src = readFileSync(new URL('../../routes/c.$session.tsx', import.meta.url), 'utf8')
  expect(src.indexOf('<Transcript')).toBeLessThan(src.indexOf('<NextStepsSlot'))
  expect(src.indexOf('<NextStepsSlot')).toBeLessThan(src.indexOf('<BackgroundArea'))
  expect(src.indexOf('<BackgroundArea')).toBeLessThan(src.indexOf('<Composer'))
})

test('dismissSuggestions drops the chips of that session only', () => {
  const a = { ...empty('a'), suggestions: ITEMS }
  const b = { ...empty('b'), suggestions: ITEMS }
  useSessions.setState({ sessions: { a, b } })
  useSessions.getState().dismissSuggestions('a')
  expect(useSessions.getState().sessions.a?.suggestions).toBeUndefined()
  expect(useSessions.getState().sessions.b?.suggestions).toEqual(ITEMS)
})
