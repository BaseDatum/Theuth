import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { expect, test } from 'vitest'
import { htmlToMarkdown, pasteText, unwrapUrl } from './paste-markdown'

// Synthetic clipboard HTML shaped like each app's real `text/html` flavour; `<name>.md` is the golden (emoji written
// `{U+XXXX}`: no literal emoji under src), `<name>.txt` the `text/plain` flavour where the gate falls back to it.
const DIR = new URL('./__fixtures__/paste/', import.meta.url)
const file = (name: string) => readFileSync(new URL(name, DIR), 'utf8')
const golden = (name: string) =>
  file(name)
    .replace(/\{U\+([0-9A-F]+)\}/g, (_, hex: string) => String.fromCodePoint(parseInt(hex, 16)))
    .replace(/\n$/, '')

const FIXTURES = readdirSync(DIR)
  .filter((f) => f.endsWith('.html'))
  .map((f) => f.slice(0, -5))

test('every fixture app is covered', () => {
  expect(FIXTURES.sort()).toEqual(
    ['bad-hrefs', 'chrome', 'confluence', 'gdocs', 'mailto', 'notion', 'outlook', 'safari', 'slack', 'teams', 'url-text', 'vscode'],
  )
})

test.each(FIXTURES)('%s pastes as its golden markdown', (name) => {
  const plain = existsSync(new URL(`${name}.txt`, DIR)) ? golden(`${name}.txt`) : 'PLAIN FALLBACK'
  expect(pasteText({ html: file(`${name}.html`), plain })).toBe(golden(`${name}.md`))
})

test('gate: HTML without a usable link pastes exactly the plain text', () => {
  const plain = 'a_b *c*\n\n\n1. d  '
  expect(pasteText({ html: '', plain })).toBe(plain)
  expect(pasteText({ html: '<p><b>bold</b> only</p>', plain })).toBe(plain)
  expect(pasteText({ html: '<img src="https://x.test/a.png" alt="a">', plain })).toBe(plain)
  expect(pasteText({ html: '<a href="/rel">rel</a> <a href="javascript:x()">js</a> <a href="#top">top</a>', plain })).toBe(plain)
  expect(pasteText({ html: '<a href="https://example.com"> </a><a href="https://example.com"><img src="https://x.test/p.png"></a>', plain })).toBe(plain)
  expect(pasteText({ html: '<a name="anchor">no href</a>', plain })).toBe(plain)
  expect(pasteText({ html: '', plain: '' })).toBe('')
})

test('gate: a Safe Links href is a link to its real address; one hiding a non-http address stays the wrapper', () => {
  const safe = (u: string) => `https://nam12.safelinks.protection.outlook.com/?url=${encodeURIComponent(u)}&data=x&reserved=0`
  expect(pasteText({ html: `<a href="${safe('https://real.test/a')}">doc</a>`, plain: 'doc' })).toBe('[doc](https://real.test/a)')
  expect(pasteText({ html: `<a href="${safe('ftp://real.test/a')}">doc</a>`, plain: 'doc' })).toBe(`[doc](${safe('ftp://real.test/a')})`)
})

test('unwrapUrl: each wrapper decodes once to an absolute http(s) address, anything else is kept', () => {
  const real = 'https://contoso.com/a b?x=1&y=2#frag'
  const enc = encodeURIComponent(real)
  expect(unwrapUrl(`https://eur02.safelinks.protection.outlook.com/?url=${enc}&data=05%7C02&sdata=a%3D&reserved=0`)).toBe(real)
  expect(unwrapUrl(`https://nam12.safelinks.protection.outlook.com/?url=${enc}`)).toBe(real)
  expect(unwrapUrl(`https://www.google.com/url?q=${enc}&sa=D&source=editors&ust=1&usg=x`)).toBe(real)
  expect(unwrapUrl(`https://www.google.com/url?sa=t&url=${enc}`)).toBe(real)
  expect(unwrapUrl(`https://google.co.uk/url?q=${enc}`)).toBe(real)
  expect(unwrapUrl(`https://slack-redir.net/link?url=${enc}`)).toBe(real)
  // Nested: Safe Links around a Google redirect.
  expect(unwrapUrl(`https://nam12.safelinks.protection.outlook.com/?url=${encodeURIComponent(`https://www.google.com/url?q=${enc}`)}`)).toBe(real)
  // Double-encoded: decoded once it is still not an absolute address, so the wrapper stays.
  const double = `https://nam12.safelinks.protection.outlook.com/?url=${encodeURIComponent(enc)}`
  expect(unwrapUrl(double)).toBe(double)
  for (const kept of [
    'https://nam12.safelinks.protection.outlook.com/?url=javascript%3Aalert(1)',
    'https://nam12.safelinks.protection.outlook.com/?url=%2Frelative',
    'https://nam12.safelinks.protection.outlook.com/?data=only',
    'https://www.google.com/search?q=https%3A%2F%2Fcontoso.com',
    'https://slack-redir.net/other?url=https%3A%2F%2Fcontoso.com',
    'https://safelinks.protection.outlook.com.evil.test/?url=https%3A%2F%2Fcontoso.com',
    'https://contoso.com/?url=https%3A%2F%2Fevil.test',
    'not a url',
    '',
    'mailto:sky@contoso.com',
  ])
    expect(unwrapUrl(kept)).toBe(kept)
})

test('wrapper URLs written out in plain text are unwrapped on the plain path too', () => {
  const safe = 'https://nam12.safelinks.protection.outlook.com/?url=https%3A%2F%2Fcontoso.com%2Fx&data=05%7C02&reserved=0'
  expect(pasteText({ html: '', plain: `See <${safe}>.\nAnd ${safe}.` })).toBe('See <https://contoso.com/x>.\nAnd https://contoso.com/x.')
  expect(pasteText({ html: '', plain: 'keep https://example.com/a_(b) as is' })).toBe('keep https://example.com/a_(b) as is')
})

test('no markdown escaping; nbsp is a plain space, code included', () => {
  expect(htmlToMarkdown('<p>file_name_v2 *note* 1. step # not a heading [x] a\\b <a href="https://e.test">l</a></p>')).toBe(
    'file_name_v2 *note* 1. step # not a heading [x] a\\b [l](https://e.test)',
  )
  expect(htmlToMarkdown('<p>a&nbsp;&nbsp;b <span>&nbsp;c</span></p><pre><code>x&nbsp;y</code></pre>')).toBe('a  b  c\n\n```\nx y\n```')
})

test('newlines: <br> is a plain newline, 3+ collapse to 2, ends trimmed, no trailing spaces', () => {
  expect(htmlToMarkdown('<br><p>a<br>b</p><br><br><br><br><p>c   </p><br>')).toBe('a\nb\n\nc')
  expect(htmlToMarkdown('<div>one</div><div>two</div>')).toBe('one\n\ntwo')
})

test('single-column (layout) tables read as paragraphs, wide ones as GFM with padded rows', () => {
  expect(htmlToMarkdown('<table><tr><td><p>Hello</p></td></tr><tr><td>Sky</td></tr></table>')).toBe('Hello\n\nSky')
  expect(htmlToMarkdown('<table><tr><td>a</td><td>b</td><td>c</td></tr><tr><td>1</td></tr></table>')).toBe(
    '| a | b | c |\n| --- | --- | --- |\n| 1 | | |',
  )
})

test('idempotent: pasting the output as plain text gives the same text', () => {
  for (const name of FIXTURES) {
    const once = pasteText({ html: file(`${name}.html`), plain: '' })
    expect(pasteText({ html: '', plain: once })).toBe(once)
  }
})
