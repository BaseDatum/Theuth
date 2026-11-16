import { expect, test } from 'vitest'
import type { Todo } from "@/rpc/bindings/method.agent.output"
import type { TodoSnapshot } from "@/rpc/bindings/method.agent.output"
import { note, summarize } from './TodoCard'

const t = (status: Todo['status'], text: string = status): Todo => ({ text, status, reason: null })

test('compact row: done/dropped/blocked counted apart, in-progress wins over pending', () => {
  expect(summarize([t('done'), t('dropped'), t('pending', 'later'), t('in_progress', 'now'), t('blocked')])).toEqual({
    counts: '1/5 done · 1 dropped · 1 blocked',
    now: 'Now: now',
  })
})

test('compact row: first pending when nothing runs; blocked-only waits; settled list has no task', () => {
  expect(summarize([t('done'), t('pending', 'a'), t('pending', 'b')]).now).toBe('Now: a')
  expect(summarize([t('done'), t('blocked'), t('blocked')]).now).toBe('Waiting: 2 blocked')
  expect(summarize([t('done'), t('dropped')])).toEqual({ counts: '1/2 done · 1 dropped', now: null })
})

test('no list: unknown says loading or unavailable, a stale empty says so; only an authoritative empty hides', () => {
  const empty: TodoSnapshot = { version: 3, source: 'native', items: [] }
  expect(note(null, false)).toBe('loading…')
  expect(note(null, true)).toBe('unavailable')
  expect(note(empty, true)).toBe('empty · Stale')
  expect(note(empty, false)).toBeNull()
})

