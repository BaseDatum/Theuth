// 216 §9.4: a plugin's OpenUI surface renders with the restricted library — no masked field, no raw HTML through
// `MarkDownRenderer` — while the same program keeps both under the apps' own library.
import { library } from '@theuth/apps'
import { AppRenderer } from '@theuth/apps/renderer'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { MASKED_REFUSED, pluginLibrary } from './library'

const SRC = [
  'root = Card([f, m])',
  'f = Form("login", b, [c, d])',
  'b = Buttons([Button("Save")])',
  'c = FormControl("API key", Input("key", "sk-…", "password"))',
  'd = FormControl("Name", Input("who", "you", "text"))',
  'm = MarkDownRenderer("Paste it: <input type=\\"password\\" name=\\"x\\"> **now**")',
].join('\n')

const render = (lib: typeof library) =>
  renderToStaticMarkup(createElement(AppRenderer, { response: SRC, library: lib, isStreaming: false, generation: 0, provider: () => Promise.reject(new Error('none')) }))

test('the apps library draws the masked field the restricted one refuses', () => {
  expect(render(library)).toMatch(/<input[^>]*type="password"/)
  const html = render(pluginLibrary)
  expect(html).not.toMatch(/type="password"/)
  expect(html).toContain(MASKED_REFUSED)
  expect(html).toMatch(/<input[^>]*name="who"/)
})

test('MarkDownRenderer skips raw HTML and still renders markdown', () => {
  const html = render(pluginLibrary)
  expect(html).not.toContain('name="x"')
  expect(html).toContain('Paste it:')
  expect(html).toMatch(/<strong[^>]*>now<\/strong>|data-streamdown="strong"[^>]*>now</)
})
