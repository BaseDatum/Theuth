import { expect, test } from 'vitest'
import type { AttentionItem } from '@/rpc/bindings/AttentionItem'
import type { Pointer } from '@/rpc/bindings/Pointer'
import type { HistoryEntry } from '@/rpc/extra'
import { clampText, groupItems, lastAssistantBefore, nextAfter, parseCodeRef, pointerRoute, urlHost } from './model'

const item = (id: string, kind: AttentionItem['kind'] = 'decision', at_ms = 1, priority: AttentionItem['priority'] = null): AttentionItem => ({ id, kind, title: id, detail: null, at_ms, task: null, agent: null, session: null, approval: null, decision: null, packet: null, ask: null, turn: null, priority, rank: 5 })
const ptr = (kind: Pointer['kind'], ref: string): Pointer => ({ kind, ref, title: ref, note: null, text: null, root: null })

test('groupItems: Sky’s order, empty groups omitted, the engine’s order kept inside (132 D4: no at_ms re-sort), hand-offs and secrets share a group', () => {
  const groups = groupItems([item('h', 'handoff', 5), item('a1', 'ask', 10), item('s', 'secret', 50), item('a2', 'ask', 30), item('d', 'decision', 1), item('n', 'node', 2)])
  expect(groups.map((g) => g.key)).toEqual(['decisions', 'questions', 'hand', 'mesh'])
  expect(groups.map((g) => g.label)).toEqual(['Decisions', 'Questions', 'Needs a hand', 'Mesh'])
  expect(groups[1]?.items.map((i) => i.id)).toEqual(['a1', 'a2'])
  expect(groups[2]?.items.map((i) => i.id)).toEqual(['h', 's'])
  // 109 §8.3: a resume that needs a human sits with the hand-offs.
  expect(groupItems([item('r', 'resume', 7)]).map((g) => g.key)).toEqual(['hand'])
  // 130 E3: finished turns are the last group, "Done", in the engine's order.
  const done = groupItems([item('f1', 'finished', 3), item('d', 'decision', 1), item('f2', 'finished', 9)])
  expect(done.map((g) => [g.key, g.label])).toEqual([['decisions', 'Decisions'], ['done', 'Done']])
  expect(done[1]?.items.map((i) => i.id)).toEqual(['f1', 'f2'])
  expect(groupItems([])).toEqual([])
})

test('groupItems: 132 D4 — urgent items of every kind form the first group, leaving their kind groups; other priorities stay put', () => {
  const groups = groupItems([item('d1', 'decision', 9, 'urgent'), item('r1', 'review', 8, 'high'), item('a1', 'ask', 7, 'urgent'), item('d2', 'decision', 6, 'low'), item('b1', 'budget', 5, 'urgent'), item('r2', 'review', 4)])
  expect(groups.map((g) => [g.key, g.label])).toEqual([['urgent', 'Urgent'], ['decisions', 'Decisions'], ['reviews', 'Reviews']])
  expect(groups[0]?.items.map((i) => i.id)).toEqual(['d1', 'a1', 'b1'])
  expect(groups[1]?.items.map((i) => i.id)).toEqual(['d2'])
  expect(groups[2]?.items.map((i) => i.id)).toEqual(['r1', 'r2'])
  // Every urgent item → only the Urgent group.
  expect(groupItems([item('x', 'ask', 1, 'urgent')]).map((g) => g.key)).toEqual(['urgent'])
})

test('clampText: short text verbatim, long text cut at a word boundary with an ellipsis within the limit', () => {
  expect(clampText('  hello  ')).toBe('hello')
  const long = `${'word '.repeat(200)}end`
  const cut = clampText(long)
  expect(cut.length).toBeLessThanOrEqual(601)
  expect(cut.endsWith('…')).toBe(true)
  expect(cut.endsWith('word…')).toBe(true)
  expect(clampText('abcdefghij', 4)).toBe('abcd…')
})

test('lastAssistantBefore: the newest assistant row at or before the instant; undated rows count; user/tool rows do not', () => {
  const row = (role: 'assistant' | 'user', text: string, at?: string): HistoryEntry =>
    ({ role, text, turn: null, context: at === undefined ? undefined : { at } }) as unknown as HistoryEntry
  const t = Date.parse('2026-10-06T00:00:00Z')
  const history = [row('assistant', 'one', '2026-10-05T23:00:00Z'), row('user', 'q'), row('assistant', 'two', '2026-10-05T23:30:00Z'), row('assistant', 'late', '2026-10-06T01:00:00Z')]
  expect(lastAssistantBefore(history, t)).toBe('two')
  expect(lastAssistantBefore([row('assistant', 'undated'), row('assistant', 'late', '2026-10-06T01:00:00Z')], t)).toBe('undated')
  expect(lastAssistantBefore([row('user', 'only'), row('assistant', '   ')], t)).toBeNull()
  expect(lastAssistantBefore([], t)).toBeNull()
})

test('parseCodeRef reads path#L<a>-L<b> and path#L<a>; rejects the rest', () => {
  expect(parseCodeRef('src/a.rs#L10-L24')).toEqual({ path: 'src/a.rs', start: 10, end: 24 })
  expect(parseCodeRef('src/a.rs#L10')).toEqual({ path: 'src/a.rs', start: 10, end: 10 })
  expect(parseCodeRef('crates/x/src/lib.rs#L1-L1')).toEqual({ path: 'crates/x/src/lib.rs', start: 1, end: 1 })
  expect(parseCodeRef('src/a.rs')).toBeNull()
  expect(parseCodeRef('src/a.rs#L24-L10')).toBeNull()
  expect(parseCodeRef('src/a.rs#L0')).toBeNull()
  expect(parseCodeRef('#L3')).toBeNull()
})

test('nextAfter: the item below the resolved one, else the one above, else null', () => {
  const items = [item('a'), item('b'), item('c')]
  expect(nextAfter(items, 'a')?.id).toBe('b')
  expect(nextAfter(items, 'b')?.id).toBe('c')
  expect(nextAfter(items, 'c')?.id).toBe('b')
  expect(nextAfter([item('a')], 'a')).toBeNull()
  expect(nextAfter(items, 'zz')).toBeNull()
  expect(nextAfter([], 'a')).toBeNull()
})

test('pointerRoute maps viewer kinds to their routes and leaves url/code/diff inline', () => {
  expect(pointerRoute(ptr('artifact', 'call-1'))).toEqual({ to: '/artifacts/$callId', params: { callId: 'call-1' } })
  expect(pointerRoute(ptr('note', 'doc-1'))).toEqual({ to: '/notes/$id', params: { id: 'doc-1' } })
  expect(pointerRoute(ptr('knowledge', 'a/b.md'))).toEqual({ to: '/knowledge/$', params: { _splat: 'a/b.md' } })
  expect(pointerRoute(ptr('session', 's1'))).toEqual({ to: '/c/$session', params: { session: 's1' } })
  expect(pointerRoute(ptr('meeting', 'm1'))).toEqual({ to: '/meetings/$id', params: { id: 'm1' } })
  expect(pointerRoute(ptr('task', 't1'))).toEqual({ to: '/tasks/$id', params: { id: 't1' } })
  expect(pointerRoute(ptr('url', 'https://x.y/z'))).toBeNull()
  expect(pointerRoute(ptr('code', 'a#L1'))).toBeNull()
  expect(pointerRoute(ptr('diff', 'main..head'))).toBeNull()
})

test('urlHost', () => {
  expect(urlHost('https://github.com/x/y/pull/1')).toBe('github.com')
  expect(urlHost('not a url')).toBe('not a url')
})
