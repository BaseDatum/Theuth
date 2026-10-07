import { expect, test } from 'vitest'
import {
  BOOTSTRAP_JS,
  CONSOLE,
  OPEN_LINK,
  SIZE_CHANGED,
  hostContext,
  injectBootstrap,
  readConsole,
  readOpenLink,
  readSizeChanged,
  result,
  sanitizeVariables,
  themeFragment,
} from './page-bootstrap'
import type { PageTheme } from './page-theme'

const THEME: PageTheme = { appearance: 'dark', variables: { '--background': '#12100e', '--foreground': '#e8dcc8' } }
const MARK = '<style id="theuth-theme">'

test('injects_at_start_of_head', () => {
  const out = injectBootstrap('<!doctype html><html><head><title>t</title><style>:root{--background:red}</style></head><body>x</body></html>', THEME)
  const head = out.indexOf('<head>') + 6
  expect(out.slice(head)).toMatch(/^<meta charset="utf-8"><meta name="viewport"[^>]*><style id="theuth-theme">/)
  // The page's own `:root` comes later in the cascade and wins.
  expect(out.indexOf(MARK)).toBeLessThan(out.indexOf('<style>:root'))
  expect(out).toContain('--background:#12100e')
  expect(out).toContain('<script>(function(window,document,console,THEME){')
  expect(out.endsWith('<body>x</body></html>')).toBe(true)
})

test('wraps_when_no_head', () => {
  expect(injectBootstrap('<html lang="en"><body>x</body></html>', THEME)).toMatch(/^<html lang="en"><head><meta charset="utf-8">.*<\/script><\/head><body>x/s)
  expect(injectBootstrap('<!DOCTYPE html><p>x', THEME)).toMatch(/^<!DOCTYPE html><head><meta charset="utf-8">.*<\/head><p>x$/s)
  expect(injectBootstrap('<p>x</p>', THEME)).toMatch(/^<!doctype html><head><meta charset="utf-8">.*<\/head><p>x<\/p>$/s)
})

test('ignores_head_in_comment_script_and_template', () => {
  const src = '<!-- <head> --><script>var s = "<head>";</script><template><head></head></template><textarea><head></textarea><html><body>x</body></html>'
  const out = injectBootstrap(src, THEME)
  const at = out.indexOf(MARK)
  // Spliced right after the real `<html>` (wrapped), with every decoy left exactly as written before it.
  expect(out.slice(0, at)).toBe(`${src.slice(0, src.indexOf('<html>') + 6)}<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">`)
  expect(out.split(MARK)).toHaveLength(2)
})

test('keeps_existing_charset_and_viewport', () => {
  const out = injectBootstrap('<html><head><meta charset="iso-8859-1"><meta name="viewport" content="width=600"></head></html>', THEME)
  expect(out).not.toContain('charset="utf-8"')
  expect(out).not.toContain('initial-scale=1')
  expect(out).toContain(MARK)
  // A charset declared only inside a comment does not count.
  expect(injectBootstrap('<html><head><!-- <meta charset="x"> --></head></html>', THEME)).toContain('<meta charset="utf-8">')
})

test('whitelists_variable_names_and_strips_delimiters', () => {
  const vars = sanitizeVariables({
    '--background': '#fff;}</style><script>alert(1)</script>{',
    '--Bad_Name': 'x',
    'color': 'red',
    '--ok': 12,
    '--empty': ';',
    '--obj': { a: 1 },
  })
  expect(vars).toEqual({ '--background': '#fff/stylescriptalert(1)/script', '--ok': '12' })
  // The fragment and the embedded THEME carry the sanitised set, and the JSON cannot close the script element.
  const frag = themeFragment({ appearance: 'light', variables: { '--x': '</script>', '--y': 'a' } })
  expect(JSON.parse(decodeURIComponent(frag.slice('#theuth-theme='.length)))).toEqual({ theme: 'light', styles: { variables: { '--x': '/script', '--y': 'a' } } })
  const out = injectBootstrap('<html><head></head></html>', { appearance: 'dark', variables: { '--a': 'x' } })
  expect(out.match(/<\/script>/g)).toHaveLength(1)
  expect(hostContext(THEME)).toEqual({ theme: 'dark', styles: { variables: THEME.variables } })
})

test('size_changed_requires_finite_positive', () => {
  const msg = (height: unknown) => ({ jsonrpc: '2.0', method: SIZE_CHANGED, params: { height } })
  expect(readSizeChanged(msg(412.5))).toBe(412.5)
  for (const bad of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, '400', null, undefined]) expect(readSizeChanged(msg(bad))).toBeNull()
  expect(readSizeChanged({ jsonrpc: '1.0', method: SIZE_CHANGED, params: { height: 10 } })).toBeNull()
  expect(readSizeChanged({ jsonrpc: '2.0', method: OPEN_LINK, params: { height: 10 } })).toBeNull()
  expect(readSizeChanged('nope')).toBeNull()
})

test('open_link_rejects_javascript_file_and_custom_schemes', () => {
  const msg = (url: unknown, id: unknown = 'theuth-link-1') => ({ jsonrpc: '2.0', id, method: OPEN_LINK, params: { url } })
  const noId = { jsonrpc: '2.0', method: OPEN_LINK, params: { url: 'https://example.com' } }
  expect(readOpenLink(msg('https://example.com/a?b#c'))).toEqual({ id: 'theuth-link-1', url: 'https://example.com/a?b#c' })
  expect(readOpenLink(msg('http://example.com', 7))).toEqual({ id: 7, url: 'http://example.com' })
  for (const bad of ['javascript:alert(1)', 'file:///etc/passwd', 'theuth-page://localhost/abc', 'asset://localhost/x', 'ftp://x', 'https:relative', '', 42])
    expect(readOpenLink(msg(bad))).toBeNull()
  // A notification (no id) is not a request the host can answer.
  expect(readOpenLink(noId)).toBeNull()
  expect(result('theuth-link-3')).toEqual({ jsonrpc: '2.0', id: 'theuth-link-3', result: {} })
})

/** Run the real in-page script against fakes; returns everything it posted to the parent. */
function boot(theme = hostContext(THEME), hash = '') {
  const posted: unknown[] = []
  const listeners: Record<string, ((ev: unknown) => void)[]> = {}
  const timers: (null | (() => void))[] = []
  const style = { textContent: '' }
  const root = { scrollHeight: 300, clientHeight: 300, getBoundingClientRect: () => ({ height: 300 }) }
  const doc = {
    readyState: 'complete',
    documentElement: root,
    body: {},
    baseURI: 'https://page.test/',
    getElementById: (id: string) => (id === 'theuth-theme' ? style : null),
    addEventListener: (n: string, cb: (ev: unknown) => void) => (listeners[`doc:${n}`] ??= []).push(cb),
  }
  const win = {
    parent: { postMessage: (m: unknown) => posted.push(m) },
    location: { hash, pathname: '/t', search: '' },
    history: { replaceState: () => {} },
    addEventListener: (n: string, cb: (ev: unknown) => void) => (listeners[`win:${n}`] ??= []).push(cb),
    setTimeout: (cb: () => void) => timers.push(cb),
    clearTimeout: (id: number) => {
      timers[id - 1] = null
    },
    URL,
  }
  type Fn = (...a: unknown[]) => void
  const con: { log: Fn; info: Fn; warn: Fn; error: Fn } = { log: () => {}, info: () => {}, warn: () => {}, error: () => {} }
  new Function('window', 'document', 'console', 'THEME', BOOTSTRAP_JS)(win, doc, con, theme)
  const fire = (key: string, ev: unknown) => listeners[key]?.forEach((cb) => cb(ev))
  const flush = () => timers.splice(0).forEach((cb) => cb?.())
  return { posted, style, root, con, fire, flush }
}

test('console_caps_at_twenty_by_five_hundred', () => {
  const { posted, con, flush } = boot()
  const before = posted.length
  con.log('a', { b: 1 }, 3)
  con.warn('w')
  con.error(new Error('boom'))
  const lines = posted.slice(before).map(readConsole)
  expect(lines[0]).toEqual({ level: 'log', text: 'a {"b":1} 3' })
  expect(lines[1]).toEqual({ level: 'warning', text: 'w' })
  expect(lines[2]?.level).toBe('error')
  expect(lines[2]?.text).toMatch(/^Error: boom\n\s+at /)
  // 500-char cut per line, 20 lines per load, then exactly one `… N more` warning.
  con.log('x'.repeat(900))
  expect(readConsole(posted[posted.length - 1])?.text).toHaveLength(500)
  for (let i = 0; i < 30; i++) con.info(`m${i}`)
  flush()
  const all = posted.slice(before).map(readConsole)
  expect(all).toHaveLength(21)
  expect(all[20]).toEqual({ level: 'warning', text: '… 14 more console messages omitted' })
  // Host-side parser: unknown levels and non-strings are dropped; long text is cut again.
  expect(readConsole({ jsonrpc: '2.0', method: CONSOLE, params: { level: 'debug', text: 'x' } })).toBeNull()
  expect(readConsole({ jsonrpc: '2.0', method: CONSOLE, params: { level: 'error', text: 7 } })).toBeNull()
  expect(readConsole({ jsonrpc: '2.0', method: CONSOLE, params: { level: 'error', text: 'y'.repeat(600) } })?.text).toHaveLength(500)
})

test('the script themes from THEME, the fragment wins, host-context-changed rewrites, size posts once per change', () => {
  const frag = themeFragment({ appearance: 'light', variables: { '--background': '#e8dcc8' } })
  const { posted, style, root, fire } = boot(hostContext(THEME), frag)
  expect(style.textContent).toMatch(/^:root\{color-scheme:light;--background:#e8dcc8;\}html\{background:var\(--background\)/)
  expect(readSizeChanged(posted[0])).toBe(300)
  fire('win:message', { data: { jsonrpc: '2.0', method: 'ui/notifications/host-context-changed', params: { theme: 'dark', styles: { variables: { '--foreground': 'red;}', bad: 'x' } } } } })
  expect(style.textContent).toMatch(/^:root\{color-scheme:dark;--foreground:red;\}/)
  // Same height again: nothing; taller content than the viewport: scrollHeight.
  fire('win:load', {})
  expect(posted.filter((m) => readSizeChanged(m) !== null)).toHaveLength(1)
  root.scrollHeight = 900.2
  root.clientHeight = 300
  fire('win:load', {})
  expect(readSizeChanged(posted[posted.length - 1])).toBe(901)
})

test('the script asks the host for http(s) links only and lets same-document hashes through', () => {
  const { posted, fire } = boot()
  const click = (href: string, extra: Record<string, unknown> = {}) => {
    const ev = {
      isTrusted: true,
      defaultPrevented: false,
      button: 0,
      prevented: false,
      preventDefault() {
        ev.prevented = true
      },
      target: { closest: () => ({ getAttribute: () => href }) },
      ...extra,
    }
    fire('doc:click', ev)
    return ev
  }
  expect(click('https://example.com/x').prevented).toBe(true)
  expect(readOpenLink(posted[posted.length - 1])).toEqual({ id: 'theuth-link-1', url: 'https://example.com/x' })
  expect(click('/relative').prevented).toBe(true)
  expect(readOpenLink(posted[posted.length - 1])).toEqual({ id: 'theuth-link-2', url: 'https://page.test/relative' })
  const n = posted.length
  expect(click('#section').prevented).toBe(false)
  expect(click('javascript:alert(1)').prevented).toBe(true)
  expect(click('https://example.com', { isTrusted: false }).prevented).toBe(false)
  expect(posted).toHaveLength(n)
})
