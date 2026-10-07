import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { CONSOLE, OPEN_LINK, SIZE_CHANGED } from '@/lib/page-bootstrap'
import { type Bridge, HtmlPage, capFromArgs, errorsAfter, frameHeight, handleBridge } from './HtmlPage'

// The theme hook reads `document`; none in a node test.
vi.mock('@/lib/page-theme', async (actual) => ({ ...(await actual<typeof import('@/lib/page-theme')>()), useHtmlPageTheme: () => ({ appearance: 'dark', variables: {} }) }))

test('reserves_height_or_240_before_first_post', () => {
  expect(frameHeight(null, undefined)).toBe(240)
  expect(frameHeight(null, 'none')).toBe(240)
  expect(frameHeight(null, 600)).toBe(600)
  expect(frameHeight(null, 3000)).toBe(2000)
  expect(frameHeight(null, 10)).toBe(80)
  // The card before the bytes arrive: the pulse box at the reserve; a page the shell could not read keeps the box and says so.
  const loading = renderToStaticMarkup(createElement(HtmlPage, { callId: 'c1', html: null, title: 'Chart', cap: 320 }))
  expect(loading).toContain('style="height:320px"')
  expect(loading).toContain('animate-pulse')
  expect(loading).not.toContain('<iframe')
  expect(capFromArgs({ content: 'x', height: 500 })).toBe(500)
  expect(capFromArgs({ height: 9000 })).toBe(2000)
  expect(capFromArgs({ height: '500' })).toBeUndefined()
  expect(capFromArgs(undefined)).toBeUndefined()
})

test('clamps_to_cap_and_2000', () => {
  expect(frameHeight(412.2, undefined)).toBe(413)
  expect(frameHeight(5000, undefined)).toBe(2000)
  expect(frameHeight(5000, 'none')).toBe(5000)
  expect(frameHeight(900, 300)).toBe(300)
  expect(frameHeight(100, 300)).toBe(100)
  expect(frameHeight(5, 300)).toBe(80)
  expect(frameHeight(5, 'none')).toBe(80)
  expect(frameHeight(1500, 2500)).toBe(1500)
  expect(frameHeight(2500, 2500)).toBe(2000)
})

const me = { tag: 'frame' }
const bridge = (over: Partial<Bridge> = {}) => {
  const calls = { heights: [] as number[], lines: [] as unknown[], opened: [] as string[], replies: [] as unknown[] }
  const b: Bridge = {
    source: me,
    focused: true,
    activation: true,
    onHeight: (h) => calls.heights.push(h),
    onConsole: (l) => calls.lines.push(l),
    open: (u) => calls.opened.push(u),
    reply: (m) => calls.replies.push(m),
    ...over,
  }
  return { b, calls }
}
const size = (height: number) => ({ jsonrpc: '2.0', method: SIZE_CHANGED, params: { height } })
const link = (url: string, id = 'theuth-link-1') => ({ jsonrpc: '2.0', id, method: OPEN_LINK, params: { url } })
const line = (level: string, text: string) => ({ jsonrpc: '2.0', method: CONSOLE, params: { level, text } })

test('ignores_messages_from_other_windows', () => {
  const { b, calls } = bridge()
  handleBridge({ source: { tag: 'other' }, data: size(400) }, b)
  handleBridge({ source: null, data: size(400) }, b)
  handleBridge({ source: { tag: 'other' }, data: link('https://example.com') }, b)
  expect(calls).toEqual({ heights: [], lines: [], opened: [], replies: [] })
  handleBridge({ source: me, data: size(400) }, b)
  expect(calls.heights).toEqual([400])
  // Before the iframe mounted there is no source to match: nothing passes.
  const none = bridge({ source: undefined })
  handleBridge({ source: undefined, data: size(400) }, none.b)
  expect(none.calls.heights).toEqual([])
})

test('link_opens_only_with_focus_and_activation', () => {
  const ok = bridge()
  handleBridge({ source: me, data: link('https://example.com/x') }, ok.b)
  expect(ok.calls.opened).toEqual(['https://example.com/x'])
  expect(ok.calls.replies).toEqual([{ jsonrpc: '2.0', id: 'theuth-link-1', result: {} }])
  // A page that took focus by script but has no user activation, or one that is not focused: answered, not opened.
  for (const over of [{ focused: false }, { activation: false }, { focused: false, activation: false }]) {
    const gated = bridge(over)
    handleBridge({ source: me, data: link('https://example.com/x', 'theuth-link-2') }, gated.b)
    expect(gated.calls.opened).toEqual([])
    expect(gated.calls.replies).toEqual([{ jsonrpc: '2.0', id: 'theuth-link-2', result: {} }])
  }
  // Non-http(s) never opens and gets no answer either (the parser rejects it).
  const bad = bridge()
  for (const url of ['javascript:alert(1)', 'file:///etc/hosts', 'theuth-page://localhost/t', 'asset://localhost/x']) handleBridge({ source: me, data: link(url) }, bad.b)
  expect(bad.calls.opened).toEqual([])
  expect(bad.calls.replies).toEqual([])
})

test('error_chip_counts_errors_only', () => {
  let errors: string[] = []
  const { b } = bridge({ onConsole: (l) => (errors = errorsAfter(errors, l)) })
  for (const [level, text] of [
    ['log', 'a'],
    ['info', 'b'],
    ['warning', 'c'],
    ['error', 'boom'],
    ['error', 'bang'],
    ['debug', 'ignored'],
  ])
    handleBridge({ source: me, data: line(level!, text!) }, b)
  expect(errors).toEqual(['boom', 'bang'])
  for (let i = 0; i < 30; i++) handleBridge({ source: me, data: line('error', `e${i}`) }, b)
  expect(errors).toHaveLength(20)
})

test('shows_unable_to_load_on_failure', () => {
  const html = renderToStaticMarkup(createElement(HtmlPage, { callId: 'c1', html: '', title: 'Sales chart', cap: 600, toolbar: createElement('button', { type: 'button' }, 'Open in panel') }))
  expect(html).toContain('Unable to load Sales chart</div>')
  expect(html).toContain('style="height:600px"')
  expect(html).not.toContain('<iframe')
  // The toolbar still offers its actions.
  expect(html).toMatch(/group-hover:opacity-100[^>]*>.*>Open in panel<\/button>/)
})
