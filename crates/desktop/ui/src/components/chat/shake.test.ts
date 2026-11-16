import { expect, test } from 'vitest'
import type { ContextItem, ContextKind, ContextTarget } from '@/lib/context-197'
import { OTHER_TOOLS, SHAKE_WORDS, catalog, expand, key, preselect, review, toggle, tokens, tri } from './shake'

const m = (seq: number): ContextTarget => ({ kind: 'message', seq })
const head = (epoch: number): ContextTarget => ({ kind: 'head', epoch })
const b = (seq: number, part: number, image: number | null = null): ContextTarget => ({ kind: 'binary', owner: { kind: 'message', seq }, part, image })
const hb = (epoch: number, part: number): ContextTarget => ({ kind: 'binary', owner: { kind: 'head', epoch }, part, image: null })
const f = (seq: number, part: number, index: number): ContextTarget => ({ kind: 'fence', owner: { kind: 'message', seq }, part, index })
const s = (seq: number, part: number): ContextTarget => ({ kind: 'source', owner: { kind: 'message', seq }, part })
const item = (target: ContextTarget, kind: ContextKind, group: string, tokens = 100, remove_with: ContextTarget[] = []): ContextItem => ({
  target,
  kind,
  group,
  label: key(target),
  tokens,
  remove_with,
})

// Assistant 2 calls A/B, tool 3 answers both; a stray multi-row response 4 belongs to 3.
const closure = [
  item(m(1), 'user', 'user'),
  item(m(2), 'tool', 'python', 300, [m(3)]),
  item(m(3), 'tool', 'python', 200, [m(2), m(4)]),
  item(m(4), 'tool', 'python', 50, [m(3)]),
  item(b(1, 1), 'image', 'attachment'),
  item(b(3, 0, 0), 'image', 'tool'),
  item(b(3, 0, 1), 'image', 'tool'),
]

test('closure follows remove_with to a fixed point and lists companions once', () => {
  const { targets, companions } = expand(closure, new Set(['m2']))
  expect(targets).toEqual([m(2), m(3), m(4)])
  expect(companions.map((c) => c.target)).toEqual([m(3), m(4)])
})

test('a whole message subsumes its own binaries, fences and sources; unrelated parts stay', () => {
  const items = [...closure, item(m(5), 'assistant', 'assistant'), item(f(5, 0, 0), 'surface', 'diagram'), item(s(5, 1), 'surface', 'page'), item(f(6, 0, 0), 'surface', 'diagram')]
  const { targets } = expand(items, new Set(['m3/0.0', 'm2', 'm1/1', 'm5#f0.0', 'm5#s1', 'm5', 'm6#f0.0']))
  // Insertion order: selected keys first, then closure; `m3/0.0` and m5's fence/source drop with their rows.
  expect(targets).toEqual([m(2), b(1, 1), m(5), f(6, 0, 0), m(3), m(4)])
})

test('kinds land in their section, categories in contract order, groups in their named order', () => {
  const items = [
    item(m(9), 'assistant', 'assistant'),
    item(m(8), 'user', 'user'),
    item(head(1), 'summary', 'summary'),
    item(b(7, 0), 'document', 'document'),
    item(m(6), 'peer', 'peer'),
    item(m(5), 'note', 'time'),
    item(m(4), 'note', 'knowledge'),
    item(f(3, 0, 0), 'surface', 'page'),
    item(f(3, 0, 1), 'surface', 'diagram'),
    item(b(2, 0), 'image', 'attachment'),
    item(hb(1, 0), 'image', 'summary'),
    item(b(2, 1), 'image', 'screenshot'),
  ]
  const cats = catalog(items)
  expect(cats.map((c) => [c.section, c.kind])).toEqual([
    ['output', 'image'],
    ['output', 'surface'],
    ['output', 'note'],
    ['output', 'peer'],
    ['output', 'document'],
    ['conversation', 'user'],
    ['conversation', 'assistant'],
    ['conversation', 'summary'],
  ])
  expect(cats[0]!.groups.map((g) => g.name)).toEqual(['Screenshots', 'Attachments', 'Summary images'])
  expect(cats[1]!.groups.map((g) => g.name)).toEqual(['Diagrams in replies', 'Pages'])
  expect(cats[2]!.groups.map((g) => g.name)).toEqual(['Knowledge hints', 'Time stamps'])
  expect(cats[5]!.groups.map((g) => g.name)).toEqual(['Your messages'])
})

test('tool output keeps its six largest tools by tokens and folds the rest into Other tools, in snapshot order', () => {
  const names = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h']
  // Tool `x` is the largest; `a`/`b` the two smallest, interleaved so the fold must keep snapshot order.
  const items = [
    item(m(1), 'tool', 'a', 10),
    item(m(2), 'tool', 'b', 20),
    ...names.slice(2).map((n, i) => item(m(10 + i), 'tool', n, 100 + i)),
    item(m(3), 'tool', 'a', 5),
    item(m(4), 'tool', 'x', 9000),
  ]
  const tools = catalog(items)[0]!
  expect(tools.groups.map((g) => g.name)).toEqual(['x', 'h', 'g', 'f', 'e', 'd', OTHER_TOOLS])
  const other = tools.groups.at(-1)!
  expect(other.items.map((i) => i.target)).toEqual([m(1), m(2), m(10), m(3)])
  expect(other.tokens).toBe(10 + 20 + 100 + 5)
  expect(tools.tokens).toBe(items.reduce((n, i) => n + i.tokens, 0))
})

test('tri-state at category, group and item level', () => {
  const cat = ['a', 'b', 'c', 'd']
  const group = ['a', 'b']
  let sel = new Set<string>()
  expect([tri(cat, sel), tri(group, sel)]).toEqual(['none', 'none'])
  sel = toggle(['a'], sel) // one item
  expect([tri(cat, sel), tri(group, sel), tri(['a'], sel)]).toEqual(['some', 'some', 'all'])
  sel = toggle(group, sel) // a partly selected group selects all of it
  expect([tri(cat, sel), tri(group, sel)]).toEqual(['some', 'all'])
  sel = toggle(cat, sel) // a partly selected category selects everything
  expect(tri(cat, sel)).toBe('all')
  sel = toggle(group, sel) // a full group clears; the category goes partial
  expect([tri(cat, sel), tri(group, sel)]).toEqual(['some', 'none'])
  sel = toggle(['c'], sel)
  expect([...sel]).toEqual(['d'])
  expect(tri([], sel)).toBe('none')
})

test('the token sum counts the closed set once: companions in, parts of a selected row out', () => {
  expect(tokens(closure, new Set(['m1/1']))).toBe(100)
  // m2 brings m3 and m4; m3's images are inside m3's projected row.
  expect(tokens(closure, new Set(['m2', 'm3/0.0', 'm3/0.1']))).toBe(300 + 200 + 50)
  expect(tokens(closure, new Set(['m3/0.0', 'm3/0.1']))).toBe(200)
  // The head subsumes its own images.
  const summary = [item(head(2), 'summary', 'summary', 900), item(hb(2, 0), 'image', 'summary', 400)]
  expect(tokens(summary, new Set(['h2', 'h2/0']))).toBe(900)
  expect(tokens(summary, new Set(['h2/0']))).toBe(400)
})

test('/shake preselection for each word', () => {
  const items = [
    item(b(1, 0), 'image', 'attachment'),
    item(hb(1, 0), 'image', 'summary'),
    item(b(2, 0), 'document', 'document'),
    item(f(3, 0, 0), 'surface', 'diagram'),
    item(s(3, 1), 'surface', 'app'),
    item(m(4), 'tool', 'shell'),
    item(m(5), 'note', 'todo'),
    item(m(6), 'peer', 'peer'),
    item(m(7), 'user', 'user'),
    item(m(8), 'assistant', 'assistant'),
    item(head(1), 'summary', 'summary'),
  ]
  const pick = (w: (typeof SHAKE_WORDS)[number]) => [...preselect(w, items)]
  expect(SHAKE_WORDS).toEqual(['images', 'documents', 'surfaces', 'tools', 'notes', 'agents', 'messages', 'summary'])
  expect(pick('images')).toEqual(['m1/0', 'h1/0'])
  expect(pick('documents')).toEqual(['m2/0'])
  expect(pick('surfaces')).toEqual(['m3#f0.0', 'm3#s1'])
  expect(pick('tools')).toEqual(['m4'])
  expect(pick('notes')).toEqual(['m5'])
  expect(pick('agents')).toEqual(['m6'])
  expect(pick('messages')).toEqual([])
  expect(pick('summary')).toEqual(['h1'])
})

test('review: a line per category, per group when partly selected, companions apart, and the three warnings', () => {
  const items = [
    item(b(1, 0), 'image', 'screenshot', 1000),
    item(b(1, 1), 'image', 'screenshot', 1000),
    item(b(2, 0), 'image', 'attachment', 500),
    item(hb(1, 0), 'image', 'summary', 400),
    item(f(3, 0, 0), 'surface', 'diagram', 300),
    item(f(3, 0, 1), 'surface', 'diagram', 200),
    item(s(4, 1), 'surface', 'page', 6000),
    item(m(5), 'tool', 'python', 700, [m(6)]),
    item(m(6), 'tool', 'python', 80, [m(5)]),
    item(m(7), 'user', 'user', 40),
    item(m(8), 'user', 'user', 60),
  ]
  const cats = catalog(items)
  const all = (kind: ContextKind) => items.filter((i) => i.kind === kind).map((i) => key(i.target))
  const r = review(cats, items, new Set([...all('image'), 'm3#f0.0', 'm4#s1', 'm5', 'm8']))
  expect(r.lines).toEqual([
    { label: 'All 4 · Images', tokens: 2900 },
    { label: 'Surfaces & diagrams › Diagrams in replies (1 of 2)', tokens: 300 },
    { label: 'Surfaces & diagrams › Pages (all 1)', tokens: 6000 },
    // A partly selected single-group category stays one line.
    { label: '1 of 2 · Tool output', tokens: 700 },
    { label: '1 of 2 · Your messages', tokens: 60 },
  ])
  expect(r.companions).toEqual({ count: 1, tokens: 80 })
  expect([r.summary, r.diagrams, r.cards]).toEqual([true, true, true])
  // A whole reply subsumes its fences: nothing is cut out of a reply that leaves whole.
  const whole = review(catalog([...items, item(m(3), 'assistant', 'assistant', 900)]), [...items, item(m(3), 'assistant', 'assistant', 900)], new Set(['m3', 'm3#f0.0']))
  expect([whole.summary, whole.diagrams, whole.cards]).toEqual([false, false, false])
  expect(whole.lines.at(-2)).toEqual({ label: 'Surfaces & diagrams › Diagrams in replies (1 of 2)', tokens: 0 })
})
