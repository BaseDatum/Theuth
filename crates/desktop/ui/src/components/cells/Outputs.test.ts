import { readFileSync } from 'node:fs'
import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import type * as PageTheme from '@/lib/page-theme'
import type * as Page from '@/components/chat/HtmlPage'
import type { CellOutput } from "@/rpc/bindings/method.cells.output"
import { AnsiText, CellOutputs, frameDocument } from './Outputs'

// The theme hook reads `document`; none in a node test. The api is never reached: no run-file image is rendered here.
vi.mock('@/lib/page-theme', async (actual) => ({ ...(await actual<typeof PageTheme>()), useHtmlPageTheme: () => ({ appearance: 'dark', variables: {} }) }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => ({}) }))
const htmlPage = vi.hoisted(() => ({ calls: [] as { callId: string; html: string | null }[] }))
vi.mock('@/components/chat/HtmlPage', async (original) => {
  const m = await original<typeof Page>()
  return {
    ...m,
    HtmlPage: (props: Parameters<typeof m.HtmlPage>[0]) => {
      htmlPage.calls.push({ callId: props.callId, html: props.html })
      return m.HtmlPage(props)
    },
  }
})

const render = (outputs: CellOutput[]) => renderToStaticMarkup(createElement(CellOutputs, { run: 'r1', outputs }))

test('streams carry the stream token classes', () => {
  const html = render([
    { output_type: 'stream', name: 'stdout', text: 'hello out\n' },
    { output_type: 'stream', name: 'stderr', text: 'oh no\n' },
  ])
  expect(html).toMatch(/data-stream="stdout"[^>]*class="[^"]*text-foreground[^"]*"[\s\S]*hello out/)
  expect(html).toMatch(/data-stream="stderr"[^>]*class="[^"]*text-destructive[^"]*"[\s\S]*oh no/)
  expect(html).not.toMatch(/text-(red|gray)-\d/)
})

test('ansi red is the --ansi-red class and \\r keeps the last state', () => {
  const html = renderToStaticMarkup(createElement(AnsiText, { text: '\u001b[31mfail\u001b[0m\nprogress 10%\rprogress 100%\n' }))
  expect(html).toMatch(/<span class="ansi-red-fg">fail<\/span>/)
  expect(html).toContain('progress 100%')
  expect(html).not.toContain('progress 10%')
  expect(html).toMatch(/^<pre data-stream="stdout" class="[^"]*font-mono[^"]*text-foreground/)
  const css = readFileSync(new URL('../../styles.css', import.meta.url), 'utf8')
  expect(css).toMatch(/\.ansi-red-fg \{ color: var\(--ansi-red\); \}/)
  expect(css).toMatch(/--ansi-red: var\(--status-danger-fg\)/)
  // OSC (title, hyperlink) and stray C0 never reach the text.
  const stripped = renderToStaticMarkup(createElement(AnsiText, { text: '\u001b]0;title\u0007ok\u001b]8;;http://x\u0007link\u001b]8;;\u0007\u0001' }))
  expect(stripped).toContain('oklink')
  expect(stripped).not.toContain('title')
})

test('text/html goes through HtmlPage only, never into the main DOM', () => {
  htmlPage.calls.length = 0
  const markup = '<b onclick="alert(1)">hi</b><script>x()</script>'
  const html = render([{ output_type: 'display_data', data: { 'text/html': markup, 'text/plain': 'hi' }, metadata: {} }])
  expect(htmlPage.calls).toEqual([{ callId: 'cell:r1:0', html: markup }])
  expect(html).not.toContain('<b onclick')
  expect(html).not.toContain('<script>')
  // Static markup is the frame's reserve box (the iframe mounts after the page is published); the frame itself is 120's.
  expect(html).toContain('animate-pulse')
  const page = readFileSync(new URL('../chat/HtmlPage.tsx', import.meta.url), 'utf8')
  expect(page).toMatch(/sandbox="allow-scripts allow-forms"/)
  expect(page).not.toMatch(/sandbox="[^"]*allow-same-origin/)
  // SVG and markdown take the same path, as documents.
  expect(frameDocument('image/svg+xml', '<svg/>')).toBe('<!doctype html><html><body style="margin:0"><svg/></body></html>')
  expect(frameDocument('text/markdown', '# T\n\n*x*')).toContain('<h1>T</h1>')
  expect(frameDocument('text/markdown', '# T\n\n*x*')).toContain('<em>x</em>')
})

test('an unsupported bundle shows its text/plain', () => {
  const html = render([{ output_type: 'execute_result', data: { 'application/vnd.plotly.v1+json': { data: [] }, 'text/plain': '<Figure 1>' }, metadata: {}, execution_count: 1 }])
  expect(html).toContain('data-mime-type="text/plain"')
  expect(html).toContain('&lt;Figure 1&gt;')
  expect(html).not.toContain('plotly')
})

test('an error shows ename and evalue', () => {
  const html = render([{ output_type: 'error', ename: 'ValueError', evalue: 'bad thing', traceback: ['Traceback (most recent call last):', '  File "<cell>", line 1', 'ValueError: bad thing'] }])
  expect(html).toContain('ValueError')
  expect(html).toContain('bad thing')
})
