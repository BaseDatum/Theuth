import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { SessionSummary } from '@/rpc/bindings/method.agent.output'
import type { Entry } from '@/store/sessions'
import { filterOtherSessions, shortName } from './SessionChip'
import { bubbleTimer, COLLAPSE_MS, lastReply, nextTurnTransition, oneLine } from './useTurnReply'

const entry = (kind: 'user' | 'assistant', text: string) => ({ id: text, kind, text, open: false }) as unknown as Entry

const sess = (id: string, over: Partial<SessionSummary> = {}): SessionSummary =>
  ({
    id,
    name: `name-${id}`,
    title: `Title ${id}`,
    project: null,
    archived: false,
    parent: null,
    kind: 'chat',
    updated_at: '2026-11-28T00:00:00Z',
    ...over,
  }) as SessionSummary

describe('floating/SessionChip helpers', () => {
  it('truncates long titles and falls back to Untitled session', () => {
    expect(shortName(null)).toBe('Untitled session')
    expect(shortName('   ')).toBe('Untitled session')
    expect(shortName('Short')).toBe('Short')
    expect(shortName('A very long session title that exceeds the limit', 12)).toBe('A very long…')
  })

  it('filters archived, child, observed, and excluded sessions and sorts newest first', () => {
    const items = [
      sess('old', { updated_at: '2026-11-28T01:00:00Z' }),
      sess('archived', { archived: true, updated_at: '2026-11-28T09:00:00Z' }),
      sess('child', { parent: 'old', updated_at: '2026-11-28T09:00:00Z' }),
      sess('obs', { kind: 'observed', updated_at: '2026-11-28T09:00:00Z' }),
      sess('bound', { updated_at: '2026-11-28T09:00:00Z' }),
      sess('newest', { title: '', name: 'fallback-name', updated_at: '2026-11-28T08:00:00Z' }),
      sess('mid', { updated_at: '2026-11-28T04:00:00Z' }),
    ]
    const out = filterOtherSessions(items, new Set(['bound']), 2)
    expect(out).toEqual([
      { session: 'newest', title: 'fallback-name' },
      { session: 'mid', title: 'Title mid' },
    ])
  })
})

describe('floating/useTurnReply helpers', () => {
  it('extracts the last non-empty assistant text after the last user entry', () => {
    expect(lastReply([entry('user', 'hi'), entry('assistant', 'first'), entry('assistant', '  final answer  ')])).toBe('final answer')
    expect(lastReply([entry('assistant', 'old'), entry('user', 'new prompt')])).toBeNull()
    expect(lastReply([entry('user', 'hi'), entry('assistant', '   ')])).toBeNull()
  })

  it('folds markdown and whitespace into one line', () => {
    expect(oneLine('## Added **3** tasks\n\nwith `blockers`')).toBe('Added 3 tasks with blockers')
    expect(oneLine('abcdefghij', 6)).toBe('abcde…')
  })

  it('detects running true -> false only for the same session', () => {
    expect(nextTurnTransition(true, 's1', false, 's1')).toBe('ended')
    // Switching from a running session s1 to an idle session s2 must NOT trigger a reply for s2.
    expect(nextTurnTransition(true, 's1', false, 's2')).toBe('noop')
    expect(nextTurnTransition(true, 's1', false, null)).toBe('noop')
    expect(nextTurnTransition(false, 's1', true, 's1')).toBe('started')
    expect(nextTurnTransition(false, 's1', false, 's1')).toBe('noop')
  })

  describe('bubbleTimer', () => {
    beforeEach(() => vi.useFakeTimers())
    afterEach(() => vi.useRealTimers())

    it('collapses after COLLAPSE_MS and resets on expand', () => {
      const states: boolean[] = []
      const t = bubbleTimer((c) => states.push(c))
      t.start()
      expect(states).toEqual([false])
      vi.advanceTimersByTime(COLLAPSE_MS - 1)
      expect(states).toEqual([false])
      vi.advanceTimersByTime(1)
      expect(states).toEqual([false, true])
      t.expand()
      vi.advanceTimersByTime(COLLAPSE_MS)
      expect(states).toEqual([false, true, false, true])
      t.dispose()
    })
  })
})
