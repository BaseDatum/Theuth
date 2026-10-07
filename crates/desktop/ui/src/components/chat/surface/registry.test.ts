import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { JsonRenderFence } from './JsonRenderFence'
import { cellClass, MarkdownTd, NO_MID_WORD_BREAK } from './registry'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { SequenceSvg } from './Sequence'

// The `Code` copy button toasts; none of that exists in a node test.
vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { message: vi.fn(), error: vi.fn() }) }))
// `DiagramFrame` (around `Sequence`) reads the connection, the CSS tokens and the tooltip provider; none exist off the DOM.
vi.mock('@/lib/rpc', () => ({ useApi: () => ({}) }))
vi.mock('@/components/ui/tooltip', () => ({ Tooltip: ({ children }: { children: unknown }) => children, TooltipTrigger: ({ children }: { children: unknown }) => children, TooltipContent: () => null }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))
vi.mock('@/lib/diagram-theme', () => ({
  diagramTokens: () => ({ background: '#fff', foreground: '#111', primary: '#111', muted: '#ddd', mutedForeground: '#666', border: '#ccc', card: '#fafafa', accent: '#b4531e', success: '#2f5d4a', danger: '#9b2d1f', font: 'sans-serif', series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'] }),
}))

const RUST = new URL('../../../../../../surface/fixtures/json-render/', import.meta.url)
const fence = (code: string) => renderToStaticMarkup(createElement(JsonRenderFence, { code, isIncomplete: false, language: 'json-render' }))

/** A closed one-element fence through the real path: parse → registry → markup. */
const render = (type: string, props: unknown) => fence(JSON.stringify({ root: 'x', elements: { x: { type, props } } }))

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

test('Table takes natural column widths behind a sideways-scrolling wrapper; headers never wrap, cells never break mid-word', () => {
  const html = render('Table', {
    columns: ['Option', 'Effort', 'Risk', 'Fixes', 'Notes'],
    rows: [['Restore the invariant in the reducer', 'Small, one crate', 'Low', 'Yes', 'Keeps the public API as it is today and needs no migration.']],
  })
  expect(html).toMatch(/<div class="[^"]*\bmax-w-full\b[^"]*\boverflow-x-auto\b[^"]*"><table class="[^"]*\bw-max\b[^"]*\bmin-w-full\b/)
  const cells = [...html.matchAll(/<(th|td) class="([^"]*)"/g)]
  expect(cells).toHaveLength(10)
  for (const [, , cls] of cells) for (const c of NO_MID_WORD_BREAK.split(' ')) expect(cls).toContain(c)
  for (const [, tag, cls] of cells) if (tag === 'th') expect(cls).toContain('whitespace-nowrap')
  expect(html).toMatch(/<td class="[^"]*whitespace-nowrap[^"]*"><div class="min-w-\[6ch\] max-w-\[48ch\]">Small, one crate</)
  expect(html).toMatch(/<td class="(?:(?!whitespace-nowrap)[^"])*"><div[^>]*>Keeps the public API/)
})

test('cellClass: a short or single-token value never wraps, prose wraps at words, an over-wide token may break anywhere', () => {
  expect(cellClass('Small, one crate')).toBe('whitespace-nowrap')
  expect(cellClass('Low')).toBe('whitespace-nowrap')
  expect(cellClass('a-single-token-that-is-long-but-under-48')).toBe('whitespace-nowrap')
  expect(cellClass('Restore the invariant in the reducer')).toBe('')
  expect(cellClass(`see ${'x'.repeat(60)} here`)).toBe('wrap-anywhere')
})

test('MarkdownTd keeps streamdown classes and marker and reads the text through inline elements', () => {
  const html = renderToStaticMarkup(createElement(MarkdownTd, { node: {}, style: { textAlign: 'right' } }, createElement('strong', null, 'Low')))
  expect(html).toBe('<td data-streamdown="table-cell" style="text-align:right" class="px-4 py-2 text-sm break-normal wrap-normal hyphens-none whitespace-nowrap"><div class="min-w-[6ch] max-w-[48ch]"><strong>Low</strong></div></td>')
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

test('Excerpt (valid-excerpt fixture): mono header path:start-end · symbol · lang, gutter from start, marked rows, caption', () => {
  const html = fence(readFileSync(new URL('valid-excerpt.json', RUST), 'utf8'))
  expect(html).toContain('crates/surface/src/validate.rs:26-29</span> · esc · rust')
  expect(html.match(/data-line="(\d+)"/g)).toEqual(['data-line="26"', 'data-line="27"', 'data-line="28"', 'data-line="29"'])
  expect(html.match(/data-line="\d+" class="[^"]*bg-fg-accent\/10/g)).toEqual(['data-line="28" class="grid min-h-5 grid-cols-[42px_14px_1fr] pr-3 bg-fg-accent/10'])
  expect(html).toContain('every error path goes through one escape')
  expect(html).toContain('aria-label="Copy excerpt"')
  expect(html).not.toContain('Show all')
})

test('Excerpt marks every row inside each [from, to] pair and caps at 18 rows with Show all', () => {
  const two = render('Excerpt', { path: 'a.rs', text: 'x\ny\nz', start: 10, mark: [[11, 12]] })
  expect(two.match(/data-line="(\d+)" class="[^"]*bg-fg-accent\/10/g)?.map((m) => m.slice(11, 13))).toEqual(['11', '12'])
  const long = render('Excerpt', { path: 'a.rs', text: Array.from({ length: 30 }, (_, i) => `l${i}`).join('\n') })
  expect(long).toContain('max-height:360px')
  expect(long).toContain('Show all 30 lines')
  expect(long.match(/data-line="/g)).toHaveLength(30)
})

test('Sequence (valid-sequence fixture): one focusable button per step with its ref as aria-description, actors as lanes', () => {
  const html = fence(readFileSync(new URL('valid-sequence.json', RUST), 'utf8'))
  const steps = html.match(/<g data-step="\d+" role="button" tabindex="0"[^>]*>/g) ?? []
  expect(steps).toHaveLength(6)
  expect(steps[0]).toContain('aria-label="1. Model → parse: fence body"')
  expect(steps[0]).toContain('aria-description="crates/surface/src/lib.rs#L90"')
  expect(steps[5]).not.toContain('aria-description')
  expect(html.match(/data-actor="/g)).toHaveLength(4)
  expect(html.match(/stroke-dasharray="6 4"/g)).toHaveLength(2)
  expect(html.match(/marker-end="url\(#seq-[0-9a-f]+-open-fg\)"/g)).toHaveLength(1)
  expect(html.match(/stroke-width="18"/g)).toHaveLength(6)
  expect(html).toContain('One json-render fence, model to picture')
})

test('Sequence highlight: the active step goes accent 2.4 with an accent dot, every other step goes muted; onSelect makes steps pointers', () => {
  const data = { actors: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }], steps: [{ from: 'a', to: 'b', label: 'x' }, { from: 'b', to: 'a', label: 'y', style: 'async' as const }] }
  const t: DiagramTokens = { background: '#fff', foreground: '#111', primary: '#111', muted: '#ddd', mutedForeground: '#666', border: '#ccc', card: '#fafafa', accent: '#b4531e', success: '#2f5d4a', danger: '#9b2d1f', font: 'sans-serif', series: ['#b4531e', '#2f5d4a', '#3b5b8c', '#8a6d1f', '#7a3e7a', '#6b5e4f'] }
  const html = renderToStaticMarkup(createElement(SequenceSvg, { data, tokens: t, markerId: 'm', highlight: [1], onSelect: () => {} }))
  const step = (i: number) => /<g data-step="I"[^>]*>(.*?)<\/g>/.exec(html.replace(`data-step="${i}"`, 'data-step="I"'))?.[1] ?? ''
  expect(step(1)).toContain('stroke="#b4531e" stroke-width="2.4"')
  expect(step(1)).toContain('marker-end="url(#m-open-accent)"')
  expect(step(1)).toContain('<circle cx="276" cy="188" r="5.5" fill="#b4531e"')
  expect(step(0)).toContain('stroke="#666" stroke-width="1.25"')
  expect(step(0)).toContain('marker-end="url(#m-closed-muted)"')
  expect(html.match(/cursor:pointer/g)).toHaveLength(2)
  const rest = renderToStaticMarkup(createElement(SequenceSvg, { data, tokens: t, markerId: 'm' }))
  expect(rest.match(/stroke="#111" stroke-width="1.25"/g)).toHaveLength(2)
  expect(rest).not.toContain('cursor:pointer')
})
