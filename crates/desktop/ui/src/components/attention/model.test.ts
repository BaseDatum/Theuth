import { receiveComponent } from '@/rpc/bindings/receive'
import { expect, test } from 'vitest'
import { ATTENTION_FIXTURE, ATTENTION_FIXTURE_NOW as NOW } from '@/components/settings/attention-fixtures'
import type { AttentionChanged } from '@/rpc/bindings/event.roster_2eattention.output'
import type { AttentionItem, AttentionOutcome } from '@/rpc/bindings/method.roster.output'
import type { Pointer } from '@/rpc/bindings/method.roster.output'
import type { HistoryEntry } from '@/rpc/bindings/method.agent.output'
import {
  EMPTY_FILTER,
  type ListFilter,
  allApprovals,
  applyChange,
  bulkTarget,
  clampText,
  countdown,
  deadlineText,
  entryHead,
  isExpired,
  kindCounts,
  lastAssistantBefore,
  matchesFilter,
  matchesQuery,
  moveFocus,
  nextAfter,
  outcomeSummary,
  parseCodeRef,
  pathText,
  pointerRoute,
  projectsOf,
  reasonLine,
  selectedIds,
  sortItems,
  stackEntries,
  stackKey,
  urlHost,
  visibleEntries,
} from './model'

const item = (id: string, kind: AttentionItem['kind'] = 'decision', at_ms = 1, priority: AttentionItem['priority'] = null): AttentionItem => (receiveComponent("method.roster.output.AttentionItem", { id, kind, title: id, detail: null, at_ms, task: null, agent: null, session: null, approval: null, decision: null, packet: null, ask: null, turn: null, priority, rank: 5 }))
const ptr = (kind: Pointer['kind'], ref: string): Pointer => ({ kind, ref, title: ref, note: null, text: null, root: null })

const F = ATTENTION_FIXTURE
/** Fixture index → item (the §9.2 table's `#` minus one). */
const at = (i: number): AttentionItem => F[i] as AttentionItem
const idx = (items: readonly AttentionItem[]) => items.map((i) => F.indexOf(i))
const filter = (over: Partial<ListFilter>): ListFilter => ({ ...EMPTY_FILTER, ...over })

test('fixture: 34 items, engine order (score desc, then waiting since asc), every score the sum of its reasons', () => {
  expect(F).toHaveLength(34)
  expect(new Set(F.map((i) => i.id)).size).toBe(34)
  for (const i of F) expect(i.score).toBe(i.reasons.reduce((s, r) => s + r.points, 0))
  for (let n = 1; n < F.length; n++) expect(at(n - 1).score).toBeGreaterThanOrEqual(at(n).score)
  // #21–#22 tie at 410: the blocked task has waited longer.
  expect([at(20).score, at(21).score, at(20).kind, at(21).kind]).toEqual([410, 410, 'blocked', 'review'])
})

test('matchesQuery: every term must occur (any case) in title, agent, tool, task, project or path; "" matches all', () => {
  const hits = (q: string) => idx(F.filter((i) => matchesQuery(i, q)))
  expect(hits('')).toHaveLength(34)
  expect(hits('ops fetch')).toEqual([3, 4, 5])
  expect(hits('KUBECTL   prod')).toEqual([0])
  expect(hits('reader browser')).toEqual([6])
  expect(hits('019a2071-0000-7000-8000-000000000005')).toEqual([20])
  expect(hits('competitor research')).toEqual([6, 7, 32])
  expect(hits('release-2.5')).toEqual([0, 1, 17, 26])
  expect(hits('analyst')).toEqual([16, 20, 27])
  expect(hits('ops nonsense')).toEqual([])
})

test('matchesFilter: kinds any-of, project none / named, combined with the query', () => {
  const hits = (f: Partial<ListFilter>) => idx(F.filter((i) => matchesFilter(i, filter(f))))
  expect(hits({})).toHaveLength(34)
  expect(hits({ kinds: new Set(['approval', 'ask']) })).toEqual([0, 1, 3, 4, 5, 6, 7])
  expect(hits({ project: { type: 'none' } })).toEqual([6, 7, 8, 9, 14, 15, 16, 18, 19, 20, 22, 23, 27, 29, 30, 32])
  expect(hits({ kinds: new Set(['approval']), project: { type: 'project', name: 'theuth' } })).toEqual([3, 4, 5])
  expect(hits({ kinds: new Set(['finished']), project: { type: 'project', name: 'release-2.5' }, query: 'changelog' })).toEqual([26])
})

test('sortItems: priority is the engine order; every other key is stable on ties', () => {
  const all = Array.from({ length: 34 }, (_, i) => i)
  expect(idx(sortItems(F, 'priority'))).toEqual(all)
  // Expiring: min(deadline, expires) asc — #1 and #11 tie at 2:00, #2 and #27 at 5:00 (engine order kept); then the rest.
  const expiring = idx(sortItems(F, 'expiring'))
  const timed = [33, 3, 0, 10, 4, 5, 6, 1, 26, 7, 8, 30, 9, 31, 25]
  expect(expiring.slice(0, 15)).toEqual(timed)
  expect(expiring.slice(15)).toEqual(all.filter((i) => !timed.includes(i)))
  // Oldest waiting: (waiting_since ?? at) asc — the finished #27 and the show #29 both 40 min.
  expect(idx(sortItems(F, 'oldest')).slice(0, 12)).toEqual([27, 15, 22, 20, 32, 11, 26, 28, 18, 21, 10, 1])
  // Newest: at_ms desc — decision #3 and approval #6 both a minute ago.
  expect(idx(sortItems(F, 'newest')).slice(0, 7)).toEqual([33, 6, 2, 5, 4, 9, 19])
  // By agent: agent, else the path's last segment; items with neither last, in engine order.
  const agent = idx(sortItems(F, 'agent'))
  expect(agent.slice(0, 5)).toEqual([16, 20, 27, 0, 21])
  expect(agent.slice(-5)).toEqual([14, 15, 18, 19, 29])
  // By project: by name, no project last, engine order inside.
  expect(idx(sortItems(F, 'project'))).toEqual([10, 11, 31, 0, 1, 17, 26, 2, 3, 4, 5, 12, 13, 21, 24, 25, 28, 33, 6, 7, 8, 9, 14, 15, 16, 18, 19, 20, 22, 23, 27, 29, 30, 32])
  expect(sortItems(F, 'oldest')).not.toBe(F)
})

test('stackEntries: the three fetch approvals of ops desk are one entry at #4; a lone approval stays an item', () => {
  const entries = stackEntries(F)
  expect(entries).toHaveLength(32)
  const stack = entries[3]
  expect(stack?.type).toBe('stack')
  expect(stack?.key).toBe(stackKey(at(3)))
  expect(stack?.type === 'stack' && idx(stack.items)).toEqual([3, 4, 5])
  expect(entryHead(stack!)).toBe(at(3))
  expect(entries[0]).toEqual({ type: 'item', key: at(0).id, item: at(0) })
  expect(entries[4]).toEqual({ type: 'item', key: at(6).id, item: at(6) })
  expect(stackKey(at(1))).toBeNull()
  // Sorted newest first, the stack sits where its first member lands; filtered to one member, it is an item again.
  const newest = visibleEntries(F, EMPTY_FILTER, 'newest', NOW)
  expect(newest.findIndex((e) => e.type === 'stack')).toBe(3)
  expect(visibleEntries(F, filter({ query: 'acme' }), 'priority', NOW)).toEqual([{ type: 'item', key: at(5).id, item: at(5) }])
})

test('selectedIds / allApprovals: entry keys expand to item ids in entry order; Allow/Deny only for all-approval selections', () => {
  const entries = stackEntries(F)
  const stack = entries[3]!.key
  const sel = new Set([stack, at(0).id])
  expect(selectedIds(entries, sel)).toEqual([at(0).id, at(3).id, at(4).id, at(5).id])
  expect(allApprovals(entries, sel)).toBe(true)
  expect(allApprovals(entries, new Set([stack, at(1).id]))).toBe(false)
  expect(allApprovals(entries, new Set())).toBe(false)
  expect(selectedIds(entries, new Set(['gone']))).toEqual([])
})

test('kindCounts: the 16 kinds of the fixture in chip order, unknown kinds after by name', () => {
  expect(kindCounts(F)).toEqual([
    { kind: 'approval', n: 5 },
    { kind: 'ask', n: 2 },
    { kind: 'decision', n: 2 },
    { kind: 'secret', n: 2 },
    { kind: 'handoff', n: 1 },
    { kind: 'resume', n: 2 },
    { kind: 'contact_first', n: 1 },
    { kind: 'node', n: 2 },
    { kind: 'home', n: 2 },
    { kind: 'review', n: 1 },
    { kind: 'hire', n: 1 },
    { kind: 'blocked', n: 3 },
    { kind: 'budget', n: 2 },
    { kind: 'show', n: 2 },
    { kind: 'finished', n: 4 },
    { kind: 'double', n: 2 },
  ])
  const odd = [item('z', 'zeta' as AttentionItem['kind']), item('a', 'alpha' as AttentionItem['kind']), item('d')]
  expect(kindCounts(odd).map((k) => k.kind)).toEqual(['decision', 'alpha', 'zeta'])
})

test('projectsOf: by name, "No project" last', () => {
  expect(projectsOf(F)).toEqual([
    { name: 'docs', n: 3 },
    { name: 'release-2.5', n: 4 },
    { name: 'theuth', n: 11 },
    { name: null, n: 16 },
  ])
  // An item from an engine before 207 has no `project` at all: it counts as none.
  expect(projectsOf([item('old')])).toEqual([{ name: null, n: 1 }])
})

test('countdown and deadlineText', () => {
  expect(countdown(102_000)).toBe('1:42')
  expect(countdown(725_000)).toBe('12:05')
  expect(countdown(3_723_000)).toBe('1:02:03')
  expect(countdown(999)).toBe('0:00')
  expect(countdown(0)).toBe('0:00')
  expect(countdown(-5_000)).toBe('0:00')
  expect(deadlineText(at(3), NOW)).toBe('denies itself in 1:00')
  expect(deadlineText(at(10), NOW)).toBe('times out in 2:00')
  expect(deadlineText(at(33), NOW)).toBe('double answers in 0:14')
  expect(deadlineText(at(33), NOW + 14_000)).toBe('due now')
  expect(deadlineText(at(2), NOW)).toBeNull()
  expect(deadlineText(item('old'), NOW)).toBeNull()
})

test('reasonLine: the first two of blocks · deadline · risk · live · task (urgent/high) · age, recomputed at now', () => {
  expect(reasonLine(at(0), NOW)).toBe('blocks 2 agents · denies itself in 2:00')
  expect(reasonLine(at(0), NOW + 61_000)).toBe('blocks 2 agents · denies itself in 0:59')
  expect(reasonLine(at(0), NOW + 120_000)).toBe('blocks 2 agents · due now')
  expect(reasonLine(at(1), NOW)).toBe('blocks 1 agent · times out in 5:00')
  expect(reasonLine(at(2), NOW)).toBe('blocks 3 agents · agent waiting on you')
  expect(reasonLine(at(6), NOW)).toBe('denies itself in 4:30 · mutating')
  expect(reasonLine(at(11), NOW)).toBe('waiting 45 min')
  expect(reasonLine(at(15), NOW)).toBe('waiting 3 h')
  expect(reasonLine(at(25), NOW)).toBe('urgent priority · ranked project · stopped 5 min ago')
  expect(reasonLine(at(31), NOW + 60_000)).toBe('stopped 11 min ago')
  expect(reasonLine(at(32), NOW)).toBe('50 min ago')
  expect(reasonLine(at(33), NOW)).toBe('double answers in 0:14')
  // Base never shows; an item younger than a minute with nothing else has no line.
  expect(reasonLine(item('fresh', 'review', NOW - 10_000), NOW)).toBe('')
})

test('pathText: project then the lineage, root first', () => {
  expect(pathText(at(0))).toBe('release-2.5 · Ship 2.5 release › builder')
  expect(pathText(at(6))).toBe('Competitor research › reader')
  expect(pathText(at(14))).toBe('')
  expect(pathText(item('old'))).toBe('')
})

test('isExpired: a stopped card past expires_ms leaves the visible list without an event', () => {
  const changelog = at(26)
  expect(isExpired(changelog, NOW)).toBe(false)
  expect(isExpired(changelog, NOW + 300_000)).toBe(true)
  expect(isExpired(at(0), NOW + 10 ** 9)).toBe(false)
  const ids = (now: number) => visibleEntries(F, EMPTY_FILTER, 'priority', now).flatMap((e) => (e.type === 'item' ? [e.key] : []))
  expect(ids(NOW)).toContain(changelog.id)
  expect(ids(NOW + 300_000)).not.toContain(changelog.id)
})

test('moveFocus: next / previous, clamped; the first key from nothing', () => {
  const keys = ['a', 'b', 'c']
  expect(moveFocus(keys, null, 1)).toBe('a')
  expect(moveFocus(keys, 'a', 1)).toBe('b')
  expect(moveFocus(keys, 'c', 1)).toBe('c')
  expect(moveFocus(keys, 'a', -1)).toBe('a')
  expect(moveFocus(keys, 'gone', -1)).toBe('a')
  expect(moveFocus([], 'a', 1)).toBeNull()
})

test('bulkTarget and outcomeSummary', () => {
  expect(bulkTarget(['a', 'b'])).toEqual({ ids: ['a', 'b'] })
  const o = (id: string, resolution: AttentionOutcome['resolution']): AttentionOutcome => ({ id, resolution, error: null })
  expect(outcomeSummary([o('1', 'gone'), o('2', 'hidden'), o('3', 'not_run'), o('4', 'hidden'), o('5', 'left_to_agent'), o('6', 'failed'), o('7', 'not_run')])).toBe('2 dismissed · 1 left to agents · 2 not run · 1 already gone · 1 failed')
  expect(outcomeSummary([o('1', 'snoozed'), o('2', 'left_for_later'), o('3', 'proceed_without')])).toBe('1 snoozed · 1 left for later · 1 told to go without')
  expect(outcomeSummary([])).toBe('')
})

const EPOCH = 1_700_000_000_000
const ev = (over: Partial<AttentionChanged>): AttentionChanged => ({ type: 'AttentionChanged', at_ms: NOW, count: 0, ids_added: [], ids_removed: [], items: [], order: [], seq: 1, epoch_ms: EPOCH, alerts_changed: false, ...over })

test('applyChange: upsert keeps the cached alert, removals drop, order is the engine’s; no refetch on a clean step', () => {
  const digest = at(31)
  const [a, b] = [at(0), at(1)]
  const cache = [digest, a, b]
  const changed = { ...digest, title: 'Weekly digest finished (edited)', alert: null }
  const fresh = { ...at(2), alert: null }
  const r = applyChange(cache, ev({ seq: 5, ids_removed: [a.id], ids_added: [fresh.id], items: [changed, fresh], order: [fresh.id, b.id, digest.id], count: 3 }), { epoch: EPOCH, seq: 4 })
  expect(r.refetch).toBe(false)
  expect(r.cursor).toEqual({ epoch: EPOCH, seq: 5 })
  expect(r.next?.map((i) => i.id)).toEqual([fresh.id, b.id, digest.id])
  expect(r.next?.[2]?.title).toBe('Weekly digest finished (edited)')
  expect(r.next?.[2]?.alert).toEqual(digest.alert)
  expect(r.next?.[0]?.alert).toBeNull()
  // The first event after a load has no cursor to compare: no gap.
  expect(applyChange(cache, ev({ seq: 9, order: cache.map((i) => i.id), count: 3 }), null).refetch).toBe(false)
})

test('applyChange: every refetch trigger — seq 0, no cache, a gap, a new run, a missing id, a count mismatch, alerts_changed', () => {
  const cache = [at(0), at(1)]
  const order = cache.map((i) => i.id)
  const cursor = { epoch: EPOCH, seq: 4 }
  // An engine before 207: no seq, nothing patched, the cursor stays.
  const legacy = applyChange(cache, ev({ seq: 0, order: [], count: 2 }), cursor)
  expect(legacy).toEqual({ next: cache, refetch: true, cursor })
  expect(applyChange(cache, { type: 'AttentionChanged', at_ms: NOW, count: 2, ids_added: [], ids_removed: [] } as unknown as AttentionChanged, null)).toEqual({ next: cache, refetch: true, cursor: null })
  expect(applyChange(undefined, ev({ seq: 5, order, count: 2 }), cursor)).toEqual({ next: undefined, refetch: true, cursor: { epoch: EPOCH, seq: 5 } })
  expect(applyChange(cache, ev({ seq: 6, order, count: 2 }), cursor).refetch).toBe(true)
  expect(applyChange(cache, ev({ seq: 5, order, count: 2, epoch_ms: EPOCH + 1 }), cursor).refetch).toBe(true)
  const missing = applyChange(cache, ev({ seq: 5, order: [...order, 'approval:unknown'], count: 3 }), cursor)
  expect(missing.refetch).toBe(true)
  expect(missing.next?.map((i) => i.id)).toEqual(order)
  expect(applyChange(cache, ev({ seq: 5, order, count: 3 }), cursor).refetch).toBe(true)
  expect(applyChange(cache, ev({ seq: 5, order, count: 2, alerts_changed: true }), cursor).refetch).toBe(true)
  expect(applyChange(cache, ev({ seq: 5, order, count: 2 }), cursor).refetch).toBe(false)
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
