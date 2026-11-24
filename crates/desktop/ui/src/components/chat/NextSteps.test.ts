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
import { NextSteps, type NextStepsGate, nextSteps } from './NextSteps'

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

const gate = (g: Partial<NextStepsGate>): NextStepsGate => ({ suggestions: ITEMS, running: false, drafting: false, blocked: false, ...g })

test('chips show for an idle root chat and hide while a turn runs, on watch-only and child chats, and while drafting', () => {
  expect(nextSteps(gate({}))).toEqual(ITEMS)
  expect(nextSteps(gate({ kind: 'ask' }))).toEqual(ITEMS)
  expect(nextSteps(gate({ running: true }))).toBeNull()
  expect(nextSteps(gate({ kind: 'observed' }))).toBeNull()
  expect(nextSteps(gate({ kind: 'advisor' }))).toBeNull()
  expect(nextSteps(gate({ parent: 'p' }))).toBeNull()
  expect(nextSteps(gate({ drafting: true }))).toBeNull()
  expect(nextSteps(gate({ blocked: true }))).toBeNull()
  expect(nextSteps(gate({ suggestions: [] }))).toBeNull()
  expect(nextSteps(gate({ suggestions: undefined }))).toBeNull()
})

/** Every element of a rendered function-component tree, depth first (no DOM in these tests). */
function elements(node: ReactNode): ReactElement<Record<string, unknown>>[] {
  if (Array.isArray(node)) return node.flatMap(elements)
  if (!isValidElement<Record<string, unknown>>(node)) return []
  return [node, ...elements(node.props.children as ReactNode)]
}

test('the block: a muted "What\'s next?" label and one button per suggestion, its full text as the tooltip', () => {
  const html = renderToStaticMarkup(NextSteps({ items: ITEMS, onPick: () => {} }))
  expect(html).toContain('>What&#x27;s next?<')
  for (const item of ITEMS) expect(html).toContain(`title="${item}"`)
  expect(html.match(/<button/g)).toHaveLength(2)
  expect(html).toContain('truncate')
})

test('clicking a chip hands its exact text to the pick handler', () => {
  const onPick = vi.fn()
  const buttons = elements(NextSteps({ items: ITEMS, onPick })).filter((e) => e.type === 'button')
  ;(buttons[1]?.props.onClick as () => void)()
  expect(onPick).toHaveBeenCalledWith('Show me the diff')
})

test('the composer sends a pick through its own submit, as typed text, and drops the chips', () => {
  const src = readFileSync(new URL('./Composer.tsx', import.meta.url), 'utf8')
  const pick = src.slice(src.indexOf('const pickStep'), src.indexOf('const pickStep') + 300)
  expect(pick).toContain('dismissSuggestions(session)')
  expect(pick).toContain('setText(item)')
  expect(pick).toContain('void submit(undefined, false, item)')
  expect(src).toContain('const line = spoken ?? typed ?? text.trim()')
  expect(src).toContain('<NextSteps items={steps} onPick={pickStep}')
  // The block sits above the input box.
  expect(src.indexOf('<NextSteps')).toBeLessThan(src.indexOf('ref={box}'))
})

test('dismissSuggestions drops the chips of that session only', () => {
  const a = { ...empty('a'), suggestions: ITEMS }
  const b = { ...empty('b'), suggestions: ITEMS }
  useSessions.setState({ sessions: { a, b } })
  useSessions.getState().dismissSuggestions('a')
  expect(useSessions.getState().sessions.a?.suggestions).toBeUndefined()
  expect(useSessions.getState().sessions.b?.suggestions).toEqual(ITEMS)
})
