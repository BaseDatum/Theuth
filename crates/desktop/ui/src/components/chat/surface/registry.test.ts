import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { JsonRenderFence } from './JsonRenderFence'

// The `Code` copy button toasts; none of that exists in a node test.
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))

/** A closed one-element fence through the real path: parse → registry → markup. */
const render = (type: string, props: unknown) =>
  renderToStaticMarkup(createElement(JsonRenderFence, { code: JSON.stringify({ root: 'x', elements: { x: { type, props } } }), isIncomplete: false, language: 'json-render' }))

test('Steps numbers items from 1', () => {
  const html = render('Steps', { items: [{ title: 'Clone' }, { title: 'Build', detail: 'cargo build' }] })
  expect(html).toMatch(/>1<\/span>.*Clone.*>2<\/span>.*Build.*cargo build/s)
})

test('Callout picks the icon by variant and defaults to note; no filled tone background', () => {
  expect(render('Callout', { text: 't', variant: 'tip' })).toContain('aria-label="tip"')
  const note = render('Callout', { text: 't', title: 'Heads up' })
  expect(note).toContain('aria-label="note"')
  expect(note).toContain('Heads up')
  expect(note).not.toMatch(/bg-(info|success|warning|danger)-bg/)
})

test('Code shows the language caption and the text in a pre, with a copy button', () => {
  const html = render('Code', { text: 'let x = 1', lang: 'rust', title: 'main.rs' })
  expect(html).toContain('main.rs')
  expect(html).toMatch(/<span class="font-mono">rust<\/span>/)
  expect(html).toMatch(/<pre[^>]*>let x = 1<\/pre>/)
  expect(html).toContain('aria-label="Copy code"')
})

test('Diff heads the columns and marks only the differing line on each side', () => {
  const html = render('Diff', { before: 'a\nb\nc', after: 'a\nB\nc', from: 'v1' })
  expect(html).toContain('>v1<')
  expect(html).toContain('>After<')
  expect(html.match(/bg-danger-bg"[^>]*>b</g)).toHaveLength(1)
  expect(html.match(/bg-success-bg"[^>]*>B</g)).toHaveLength(1)
  expect(html).not.toMatch(/bg-(danger|success)-bg"[^>]*>a</)
})

test('Tree nests by depth: a parent is a collapsible holding its children, a leaf is a plain row', () => {
  const html = render('Tree', { items: [{ label: 'src' }, { label: 'lib.rs', depth: 1 }, { label: 'main.rs', depth: 1, detail: '2 KB' }, { label: 'README' }] })
  expect(html).toMatch(/data-slot="collapsible"[^>]*>.*>src<.*data-slot="collapsible-content".*lib\.rs.*main\.rs.*2 KB/s)
  expect(html).toMatch(/<\/div><div class="flex items-center gap-1.5 pl-5"><span>README<\/span><\/div>/)
  expect(html.match(/data-slot="collapsible"/g)).toHaveLength(1)
})

test('Glossary is a dl of term/text pairs', () => {
  expect(render('Glossary', { items: [{ term: 'ACK', text: 'acknowledged' }] })).toMatch(/<dl[^>]*>.*<dt[^>]*>ACK<\/dt><dd[^>]*>acknowledged<\/dd>/)
})

test('Quote is a blockquote with the source as an em-dash caption', () => {
  const html = render('Quote', { text: 'Less is more.', source: 'Mies' })
  expect(html).toMatch(/<blockquote.*Less is more\..*— Mies/s)
  expect(render('Quote', { text: 'q' })).not.toContain('—')
})

test('a remote Image waits for a click; an inline data: image shows at once', () => {
  const remote = render('Image', { src: 'https://x.test/a.png', alt: 'A chart', caption: 'Fig. 1' })
  expect(remote).not.toContain('<img')
  expect(remote).toMatch(/<button[^>]*title="https:\/\/x\.test\/a\.png"[^>]*>Load image ·.*A chart<\/button>/)
  expect(remote).toContain('<figcaption class="text-xs text-fg-tertiary">Fig. 1</figcaption>')
  const inline = render('Image', { src: 'data:image/png;base64,AA==', alt: 'dot' })
  expect(inline).toMatch(/<img[^>]*src="data:image\/png;base64,AA=="[^>]*alt="dot"/)
})

test('LinkList opens each url in a new noopener tab with its note', () => {
  const html = render('LinkList', { items: [{ title: 'Docs', url: 'https://x.test/docs', note: 'official' }] })
  expect(html).toMatch(/<a href="https:\/\/x\.test\/docs" target="_blank" rel="noopener noreferrer"[^>]*>Docs<\/a>/)
  expect(html).toContain('official')
})

test('Tags renders one badge per item with the Badge tone classes', () => {
  const html = render('Tags', { items: [{ text: 'a' }, { text: 'b', variant: 'success' }, { text: 'c', variant: 'destructive' }] })
  expect(html.match(/data-slot="badge"/g)).toHaveLength(3)
  expect(html).toMatch(/data-variant="outline"[^>]*bg-success-bg[^>]*>b</)
  expect(html).toMatch(/data-variant="destructive"[^>]*>c</)
})

test('Gauge clamps to the min..max range and defaults 0..100', () => {
  expect(render('Gauge', { value: 150, label: 'CPU' })).toMatch(/stroke-dasharray="100 100".*>150<.*CPU/s)
  expect(render('Gauge', { value: 5, min: 0, max: 20 })).toContain('stroke-dasharray="25 100"')
  expect(render('Gauge', { value: -3 })).toContain('stroke-dasharray="0 100"')
})

test('Sparkline is one polyline point per value, flat at mid-height when all values are equal', () => {
  const html = render('Sparkline', { values: [1, 3, 2, 4], label: 'week' })
  expect(html).toMatch(/points="0\.0,22\.0 33\.3,8\.7 66\.7,15\.3 100\.0,2\.0"/)
  expect(html).toContain('week')
  expect(render('Sparkline', { values: [2, 2, 2] })).toContain('points="0.0,12.0 50.0,12.0 100.0,12.0"')
})

test('Metric with spark draws the sparkline beside the value', () => {
  const html = render('Metric', { label: 'Tests', value: '412', spark: [1, 2] })
  expect(html).toMatch(/>412<.*<polyline points="0\.0,22\.0 100\.0,2\.0"/s)
  expect(render('Metric', { label: 'Tests', value: '412' })).not.toContain('<polyline')
})

test('Heatmap mixes the accent by value over the largest cell and titles each cell row · col: value', () => {
  const html = render('Heatmap', { rows: ['mon', 'tue'], cols: ['am', 'pm'], cells: [[1, 4], [2, 0]] })
  expect(html).toMatch(/title="mon · pm: 4"[^>]*background-color:color-mix\(in oklab, var\(--color-fg-accent\) 100%, transparent\)/)
  expect(html).toMatch(/title="tue · am: 2"[^>]*50%/)
  expect(html).toMatch(/title="tue · pm: 0"[^>]*0%/)
  expect(render('Heatmap', { rows: ['r'], cols: ['c'], cells: [[5]], max: 10 })).toContain('50%')
})

test('Person shows the initials of up to two words', () => {
  expect(render('Person', { name: 'Ada Byron Lovelace', role: 'Analyst' })).toMatch(/>AB<\/div>.*Ada Byron Lovelace.*Analyst/s)
  expect(render('Person', { name: 'plato' })).toContain('>P</div>')
})

test('FileList uses the folder icon for dirs (by kind or trailing slash) and the file icon otherwise', () => {
  const html = render('FileList', { items: [{ path: 'src/', size: '12 KB' }, { path: 'x', kind: 'dir' }, { path: 'Cargo.toml', kind: 'file' }] })
  expect(html.match(/aria-label="folder"/g)).toHaveLength(2)
  expect(html.match(/aria-label="file"/g)).toHaveLength(1)
  expect(html).toContain('12 KB')
})

test('Kanban counts the cards per column and tags a card', () => {
  const html = render('Kanban', { columns: [{ title: 'Todo', cards: [{ title: 'a' }, { title: 'b', tag: 'bug' }] }, { title: 'Done', cards: [] }] })
  expect(html).toMatch(/>Todo<\/span><span[^>]*>2<\/span>/)
  expect(html).toMatch(/>Done<\/span><span[^>]*>0<\/span>/)
  expect(html).toMatch(/data-slot="badge"[^>]*>bug</)
})

test('Calendar marks the active day and titles the strip', () => {
  const html = render('Calendar', { title: 'Sprint 4', days: [{ day: '12', state: 'done' }, { day: '13', label: 'demo', state: 'active' }, { day: '14' }] })
  expect(html).toContain('Sprint 4')
  expect(html).toMatch(/ring-fg-accent\/30" aria-label="active"/)
  expect(html.match(/aria-label="pending"/g)).toHaveLength(1)
  expect(html).toContain('demo')
})
