import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import { SuffixList, SuffixScopes } from './SuffixPicker'

const SAVED = [
  { id: 'a', name: 'Terse', text: 'Be terse.' },
  { id: 'b', name: 'Cite', text: 'Cite sources.' },
]

/** A phone (`(max-width: 767px)` and a coarse pointer) or a desktop window. */
function layout(phone: boolean): void {
  const mql = { matches: phone, addEventListener: () => undefined, removeEventListener: () => undefined }
  vi.stubGlobal('window', { matchMedia: () => mql })
  vi.stubGlobal('matchMedia', () => mql)
}

afterEach(() => vi.unstubAllGlobals())

const list = () => renderToStaticMarkup(createElement(SuffixList, { suffixes: SAVED, onPick: () => {}, onBack: () => {}, onManage: () => {} }))

test('the + menu lists every saved suffix and both scopes in the phone layout, with finger-sized rows', () => {
  layout(true)
  const html = list()
  expect([...html.matchAll(/data-suffix="([^"]+)"/g)].map((m) => m[1])).toEqual(['a', 'b'])
  expect(html).toContain('Manage prompts')
  expect(html).toContain('Prompts')
  expect(html).not.toContain('Add prompt suffix')
  expect(html).toMatch(/min-h-11[^"]*"[^>]*data-suffix="a"|data-suffix="a"[^>]*min-h-11/)
  const scopes = renderToStaticMarkup(createElement(SuffixScopes, { suffix: SAVED[0]!, onPick: () => {} }))
  expect(scopes).toContain('This message')
  expect(scopes).toContain('Every message until I turn it off')
  expect(scopes).toContain('min-h-11')
})

test('the desktop layout lists the same suffixes at the menu’s normal row height', () => {
  layout(false)
  const html = list()
  expect([...html.matchAll(/data-suffix="([^"]+)"/g)].map((m) => m[1])).toEqual(['a', 'b'])
  expect(html).not.toContain('min-h-11')
})
