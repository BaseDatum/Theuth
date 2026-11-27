import { expect, test } from 'vitest'
import type { DiagramTokens } from './diagram-theme'
import { type El, exportAssets, mountedDiagrams } from './doc-export'

const DARK: DiagramTokens = {
  background: '#12100e',
  foreground: '#F3EADF',
  primary: '#f3eadf',
  muted: '#2a241c',
  mutedForeground: '#a89880',
  border: '#3a3228',
  card: '#1c1915',
  accent: '#d8763a',
  success: '#7fb59a',
  danger: '#e0705f',
  font: 'ui-sans-serif',
  series: ['#d8763a', '#7fb59a', '#7f9fd0', '#c8ae5a', '#b088b0', '#a89880'],
}
// Light foreground equals dark primary's hue family but not its value; background/foreground cross over on purpose.
const LIGHT: DiagramTokens = { ...DARK, background: '#f3eadf', foreground: '#1a1612', primary: '#1a1612', card: '#ffffff', border: '#d4c4a8' }

/** A DOM node stand-in: tag, attributes (`class` too), children; answers the selectors `mountedDiagrams` uses. */
type Node = El & { tag: string; attrs: Record<string, string>; kids: Node[]; parent: Node | null }

function h(tag: string, attrs: Record<string, string> = {}, kids: Node[] = []): Node {
  const n: Node = {
    tag,
    attrs,
    kids,
    parent: null,
    get outerHTML() {
      const a = Object.entries(attrs).map(([k, v]) => ` ${k}="${v}"`)
      return `<${tag}${a.join('')}/>`
    },
    getAttribute: (name) => attrs[name] ?? null,
    closest: (sel) => {
      for (let p: Node | null = n; p; p = p.parent) if (matches(p, sel)) return p
      return null
    },
    querySelector: (sel) => all(n, sel)[0] ?? null,
    querySelectorAll: (sel) => all(n, sel),
  }
  for (const k of kids) k.parent = n
  return n
}

/** One compound selector: `tag`, `.class`, `[attr]`, `[attr="v"]`, any combination. */
function matches(n: Node, sel: string): boolean {
  const tag = /^[a-z]+/.exec(sel)?.[0]
  if (tag && n.tag !== tag) return false
  for (const [, c] of sel.matchAll(/\.([\w-]+)/g)) if (!(n.attrs.class ?? '').split(' ').includes(c!)) return false
  for (const [, k, v] of sel.matchAll(/\[([\w-]+)(?:="([^"]*)")?\]/g)) if (n.attrs[k!] === undefined || (v !== undefined && n.attrs[k!] !== v)) return false
  return true
}

function all(root: Node, selector: string): Node[] {
  if (selector === ':scope > *') return root.kids
  const sels = selector.split(',').map((s) => s.trim())
  const out: Node[] = []
  const walk = (n: Node) => {
    for (const k of n.kids) {
      if (sels.some((s) => matches(k, s))) out.push(k)
      walk(k)
    }
  }
  walk(root)
  return out
}

const svg = (viewBox: string) => h('svg', { viewBox })
const icon = () => h('button', {}, [svg('0 0 24 24')])
/** A board diagram block. */
const board = (id: string, ...kids: Node[]) => h('div', { class: 'board-block board-wide board-diagram', 'data-block': id }, kids)
/** A note code block rendered as a diagram: `DiagramView` around the fence's surface. */
const note = (kind: 'json-render' | 'mermaid', id: string, surface: Node) => h('div', { 'data-diagram': kind, 'data-block': id, 'data-doc': 'n1' }, [icon(), surface])
const surface = (...kids: Node[]) => h('div', { 'data-streamdown': 'json-render' }, kids)
const frame = (kind: string, viewBox: string) => h('div', { 'data-streamdown': `${kind}-block` }, [icon(), h('div', {}, [svg(viewBox)])])

test('mountedDiagrams keys a board diagram svg by block id, skipping button icons and blocks without a diagram', () => {
  const root = h('body', {}, [board('g1', icon(), svg('0 0 10 10')), board('s2'), board('s3', h('svg'), svg('0 0 4 4'))])
  expect(mountedDiagrams(root)).toEqual([
    { id: 'g1', markup: '<svg viewBox="0 0 10 10"/>' },
    { id: 's3', markup: '<svg viewBox="0 0 4 4"/>' },
  ])
})

test('a note json-render block that is one diagram gives its svg; composites, mermaid and plain blocks give nothing', () => {
  const root = h('body', {}, [
    note('json-render', 'g1', surface(frame('graph', '0 0 30 20'))),
    // A Card around the Graph: the daemon snapshots the whole card.
    note('json-render', 'c2', surface(h('div', { class: 'card' }, [h('h3'), frame('graph', '0 0 5 5')]))),
    // A Graph beside a Text at the top level.
    note('json-render', 'c3', surface(frame('sequence', '0 0 6 6'), h('p'))),
    // A chart drawn outside DiagramFrame.
    note('json-render', 'b4', surface(h('div', { class: 'chart' }, [svg('0 0 7 7')]))),
    // Mermaid goes through the daemon's PNG snapshot (Typst's svg text uses the host's fonts).
    note('mermaid', 'm5', h('div', { 'data-streamdown': 'mermaid-block' }, [svg('0 0 8 8')])),
  ])
  expect(mountedDiagrams(root)).toEqual([{ id: 'g1', markup: '<svg viewBox="0 0 30 20"/>' }])
})

test('with a docId, only that document column counts; without its column, the whole root', () => {
  const column = (doc: string, id: string) => h('div', { 'data-doc': doc }, [board(id, svg('0 0 1 1'))])
  const root = h('body', {}, [column('d1', 'a'), column('d2', 'b')])
  expect(mountedDiagrams(root, 'd2').map((d) => d.id)).toEqual(['b'])
  expect(mountedDiagrams(root, 'd1').map((d) => d.id)).toEqual(['a'])
  expect(mountedDiagrams(root, 'gone').map((d) => d.id)).toEqual(['a', 'b'])
  expect(mountedDiagrams(root).map((d) => d.id)).toEqual(['a', 'b'])
})

test('exportAssets recolours in one pass to the light palette and makes each svg standalone', () => {
  const markup = '<svg viewBox="0 0 20 10"><rect fill="#12100e" stroke="#f3eadf"/><text fill="#F3EADF">a</text><path stroke="#d8763a"/></svg>'
  const asset = exportAssets([{ id: 'g1', markup }], DARK, LIGHT)[0]!
  expect(asset.id).toBe('g1')
  // background → light background, foreground → light foreground, and the light background is not re-swapped.
  expect(asset.svg).toContain('<rect fill="#f3eadf" stroke="#1a1612"/>')
  expect(asset.svg).toContain('<text fill="#1a1612">a</text>')
  // Unchanged tokens stay.
  expect(asset.svg).toContain('stroke="#d8763a"')
  expect(asset.svg).toMatch(/^<svg xmlns="http:\/\/www.w3.org\/2000\/svg" viewBox="0 0 20 10" width="20" height="10"><rect x="-1" y="-1" width="22" height="12" fill="#fff"\/>/)
})

test('exportAssets drops an svg without a viewBox and is identity when the palettes match', () => {
  expect(exportAssets([{ id: 'x', markup: '<svg/>' }], DARK, LIGHT)).toEqual([])
  const a = exportAssets([{ id: 'y', markup: '<svg viewBox="0 0 1 1"><g fill="#12100e"/></svg>' }], DARK, DARK)[0]!
  expect(a.svg).toContain('fill="#12100e"')
})
