// M8: two type registers and no third. `board.css` may only ever name the prose face (`--font-serif`) and the chrome
// face (`--font-mono`); `.board-prose` is the prose register, `.board-chrome` the chrome register, and the
// `boards.serif` opt-out swaps the prose face for `--font-sans` without touching chrome. The rendered chrome pieces
// (section summary, TOC) carry `.board-chrome`. Computed styles are checked in the browser smoke (as-built).
import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { SectionView } from './Sections'
import { Toc } from './Toc'

const CSS = readFileSync(new URL('./board.css', import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')

/** `selector → declarations` for every rule, flattening `@media`/`@container` blocks. */
function rules(css: string): [string, string][] {
  const out: [string, string][] = []
  const re = /([^{}]+)\{([^{}]*)\}/g
  for (const m of css.matchAll(re)) {
    const sel = m[1]!.trim()
    if (sel.startsWith('@')) continue
    out.push([sel, m[2]!])
  }
  return out
}

test('board.css names exactly two faces: the prose face and the mono chrome face', () => {
  const faces = new Set<string>()
  for (const [, decls] of rules(CSS)) for (const m of decls.matchAll(/font-family:\s*([^;]+);/g)) faces.add(m[1]!.trim())
  expect([...faces].sort()).toEqual(['var(--font-mono)', 'var(--font-serif)'])
})

test('.board-prose is the prose register, .board-chrome the mono register; boards.serif off swaps prose to sans only', () => {
  const byRule = Object.fromEntries(rules(CSS))
  expect(byRule['.board-prose']).toMatch(/font-family:\s*var\(--font-serif\)/)
  expect(byRule['.board-chrome']).toMatch(/font-family:\s*var\(--font-mono\)/)
  expect(byRule['.board-chrome']).toMatch(/font-size:\s*11px/)
  expect(byRule[".board-shell[data-serif='false']"]).toMatch(/--font-serif:\s*var\(--font-sans\)/)
  // No rule under `.board-prose` re-declares a family except inline `code` (mono).
  for (const [sel, decls] of rules(CSS)) {
    if (!sel.startsWith('.board-prose ') || !/font-family/.test(decls)) continue
    expect(sel).toBe('.board-prose code')
    expect(decls).toMatch(/var\(--font-mono\)/)
  }
})

test('rendered chrome carries .board-chrome: the collapsed summary and the TOC', () => {
  const section = {
    heading: { id: 'h', type: 'heading', props: { level: 2 }, content: [], children: [] } as never,
    blocks: [{ id: 'p', type: 'paragraph' } as never, { id: 'q', type: 'paragraph' } as never],
    anchors: 1,
  }
  const html = renderToStaticMarkup(
    createElement(SectionView, { section, collapsed: true, onToggle: () => {}, heading: createElement('h2', { className: 'board-prose' }, 'Map'), children: createElement('p', null, 'body') }),
  )
  expect(html).toContain('data-collapsed="true"')
  expect(html).toMatch(/<span class="board-section-meta board-chrome">2 blocks · 1 anchor<\/span>/)
  expect(html).not.toContain('<p>body</p>') // the body is unmounted while collapsed
  expect(html).toContain('aria-expanded="false"')

  const toc = renderToStaticMarkup(
    createElement(Toc, {
      entries: [
        { id: 'a', level: 1, text: 'Title', section: null },
        { id: 'b', level: 2, text: 'Map', section: 'b' },
      ],
      active: 'b',
      onSelect: () => {},
    }),
  )
  expect(toc).toContain('class="board-toc board-chrome"')
  expect(toc).toMatch(/<li data-level="2"><button[^>]*data-active="true"[^>]*aria-current="location"[^>]*>Map<\/button>/)
  expect(toc).toContain('aria-label="Contents"')
})
