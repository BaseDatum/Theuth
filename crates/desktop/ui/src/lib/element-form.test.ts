import '@/components/settings/test-dom'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { useElementForms } from '@/store/element-forms'
import {
  ALLOWED_ATTRS,
  ALLOWED_TAGS,
  COLLECT,
  DROP_TAGS,
  FORM_CSP,
  SUBMIT_REQUEST,
  attachSession,
  collectValues,
  formDocument,
  originLabel,
  outcomeOfReceipt,
  pickValues,
  readFrameMessage,
  sanitizeSnapshot,
  sourceLabel,
  startElementForms,
  type ElementFormRequested,
} from './element-form'

// What the shell's connection would be: the store is per engine in the app; one fake here.
const conn = vi.hoisted(() => ({
  s: { api: null as unknown, state: 'open', discovery: { phase: 'ready' } } as { api: unknown; state: string; discovery: unknown },
  subs: new Set<() => void>(),
}))
// The real module's other exports stay: the per-engine scope creates its connection store with them.
vi.mock('@/store/connection', async (original) => {
  const useConnection = (sel: (s: unknown) => unknown = (s) => s) => sel(conn.s)
  useConnection.getState = () => conn.s
  useConnection.subscribe = (cb: () => void) => {
    conn.subs.add(cb)
    return () => conn.subs.delete(cb)
  }
  return { ...(await original<object>()), useConnection }
})
vi.mock('@/rpc/discovery', async (original) => ({ ...(await original<object>()), supports: () => true, ready: () => true }))

const { JSDOM } = createRequire(import.meta.url)('../../../../../packages/apps/node_modules/jsdom')

const FIELDS = [
  { index: 0, kind: 'email', name: 'email', label: 'Email', masked: false },
  { index: 1, kind: 'password', name: 'pw', label: 'Password', masked: true },
  { index: 2, kind: 'checkbox', name: 'keep', label: 'Keep me signed in', masked: false },
  { index: 3, kind: 'submit', name: '', label: 'Sign in', masked: false },
  { index: 4, kind: 'select', name: 'country', label: 'Country', masked: false },
]
const SNAPSHOT =
  '<form data-theuth-field-root><label for="e">Email</label><input id="e" type="email" data-theuth-field="0">' +
  '<input type="text" data-theuth-field="1"><input type="checkbox" data-theuth-field="2">' +
  '<select data-theuth-field="4"><option>Andorra</option><option>Belgium</option></select>' +
  '<textarea data-theuth-field="9"></textarea><button data-theuth-field="3">Sign in</button></form>'

const form = (over: Partial<ElementFormRequested> = {}): ElementFormRequested => ({
  type: 'ElementFormRequested',
  id: 'f1',
  session: 's1',
  call_id: 'c1',
  origin: 'https://accounts.example.com',
  action_origin: 'https://accounts.example.com',
  url: 'https://accounts.example.com/login',
  source: 'web:0123456789abcdef',
  title: 'Sign in',
  html: SNAPSHOT,
  width: 320,
  height: 200,
  fields: FIELDS,
  ...over,
})

const reset = () => useElementForms.setState({ pending: {}, outcomes: {}, calls: {} })
beforeEach(reset)

// jsdom enforces no CSP: this proves the sanitiser and the policy's place in the document only. That the policy holds
// (no network, no submit) is proven by `pages.rs`'s header test and the simulator run.
test('the sanitiser leaves nothing that loads or runs, and the CSP meta is the first thing in the document', () => {
  const hostile =
    '<base href="https://evil.example/"><meta http-equiv="refresh" content="0;url=https://evil.example/">' +
    '<meta http-equiv="Content-Security-Policy" content="default-src *">' +
    '<link rel="stylesheet" href="https://evil.example/s.css"><script>fetch("https://evil.example")</script>' +
    '<img src="https://evil.example/pixel.png" alt="logo"><img src="data:image/png;base64,AAAA" alt="Example">' +
    '<div style="background:url(https://evil.example/bg.png);color:red" onclick="alert(1)">x</div>' +
    '<iframe src="https://evil.example"></iframe><a href="https://evil.example">help</a>' +
    '<xmp><img src="https://evil.example/x.png"></xmp><noframes>evil.example</noframes>' +
    '<input type="hidden" name="csrf" data-theuth-field="7">'
  const doc = formDocument(hostile, FIELDS)
  // The very first tag after the doctype's <html><head> is the policy.
  expect(doc.startsWith(`<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${FORM_CSP}">`)).toBe(true)
  const parsed = new DOMParser().parseFromString(doc, 'text/html')
  const first = parsed.head.firstElementChild
  expect(first?.getAttribute('http-equiv')).toBe('Content-Security-Policy')
  expect(first?.getAttribute('content')).toBe(FORM_CSP)
  expect(parsed.querySelectorAll('base')).toHaveLength(0)
  expect(parsed.querySelectorAll('meta[http-equiv]')).toHaveLength(1)
  expect(parsed.querySelectorAll('link, iframe, [href], [onclick]')).toHaveLength(0)
  // One script: the app's bootstrap, in the head.
  expect(parsed.querySelectorAll('script')).toHaveLength(1)
  expect(parsed.querySelector('body script')).toBeNull()
  // The network image loses its source; the inline one keeps it; both keep their alternative text.
  const imgs = [...parsed.querySelectorAll('img')]
  expect(imgs.map((i) => [i.getAttribute('src'), i.getAttribute('alt')])).toEqual([
    [null, 'logo'],
    ['data:image/png;base64,AAAA', 'Example'],
  ])
  expect(parsed.querySelector('div')?.getAttribute('style')).toBe('background:none;color:red')
  expect(parsed.querySelector('input[type=hidden]')).toBeNull()
  expect(doc).not.toContain('evil.example')
})

test('the client allowlist is the engine\'s (show_element.allow.json)', () => {
  const engine = JSON.parse(readFileSync(new URL('../../../../browser/src/show_element.allow.json', import.meta.url), 'utf8')) as {
    tags: string[]
    dropped: string[]
    attrs: string[]
    img_attrs: string[]
  }
  expect([...ALLOWED_TAGS].sort()).toEqual([...engine.tags].sort())
  expect([...DROP_TAGS].sort()).toEqual([...engine.dropped].sort())
  expect([...ALLOWED_ATTRS].sort()).toEqual([...engine.attrs].sort())
  // `img` keeps `alt` and (an inline image's) `src`, as the engine does.
  expect(engine.img_attrs.sort()).toEqual(['alt', 'src'])
})

test('a field the event marks masked renders as a password whatever the clone says', () => {
  const out = new DOMParser().parseFromString(sanitizeSnapshot(SNAPSHOT, new Set(['1'])), 'text/html')
  expect(out.querySelector('[data-theuth-field="1"]')?.getAttribute('type')).toBe('password')
  expect(out.querySelector('[data-theuth-field="0"]')?.getAttribute('type')).toBe('email')
})

test('the origin is shown in ASCII with its registrable end apart: a homograph is punycode, the port stays, a non-web origin is refused', () => {
  // Cyrillic "а" in place of the Latin one.
  expect(originLabel('https://\u0430pple.com')).toEqual({ text: 'https://xn--pple-43d.com', host: 'xn--pple-43d.com', secure: true, head: 'https://', tail: 'xn--pple-43d.com' })
  expect(originLabel('http://192.168.1.20:8080')).toEqual({ text: 'http://192.168.1.20:8080', host: '192.168.1.20:8080', secure: false, head: 'http://', tail: '192.168.1.20:8080' })
  expect(originLabel('https://accounts.example.com:443')?.text).toBe('https://accounts.example.com')
  // A phishing suffix: the part a truncation must never hide is the last two labels.
  const phish = originLabel('https://accounts.google.com.evil.example')!
  expect([phish.head, phish.tail]).toEqual(['https://accounts.google.com.', 'evil.example'])
  expect(originLabel('https://a.b.example:8443')?.tail).toBe('b.example:8443')
  expect(originLabel('null')).toBeNull()
  expect(originLabel('javascript:alert(1)')).toBeNull()
  expect(sourceLabel('web:0123456789abcdef')).toBe('your web view')
  expect(sourceLabel('obscura:studio')).toBe('agent browser on studio')
})

/** The form document in its own window, running the bootstrap as the frame would; what it posts to its parent is in `posted`. */
function frame(fields = FIELDS) {
  const posted: unknown[] = []
  const dom = new JSDOM(formDocument(SNAPSHOT, fields), {
    runScripts: 'dangerously',
    beforeParse(w: { postMessage: (m: unknown) => void }) {
      // A top-level jsdom window is its own parent: what the bootstrap sends to its parent lands here (size reports aside).
      w.postMessage = (m: unknown) => void (readFrameMessage(m)?.type === 'size' || posted.push(m))
    },
  })
  const w = dom.window
  const el = (n: number) => w.document.querySelector(`[data-theuth-field="${n}"]`)
  /** What the user does: set the value, and the control fires `input`. */
  const type = (n: number, value: string) => {
    el(n).value = value
    el(n).dispatchEvent(new w.Event('input', { bubbles: true }))
  }
  return { w, el, type, posted }
}

test('collect answers only the controls the user changed, every checkbox\'s state, and leaves buttons out', () => {
  const { w, el, type, posted } = frame()
  // The page prefilled the email: untouched, it is not sent, so the page keeps it.
  el(0).value = 'prefilled@example.com'
  type(1, 'hunter2')
  el(2).checked = true
  type(9, 'not a field')
  // Only the parent may ask.
  w.dispatchEvent(new w.MessageEvent('message', { data: { type: COLLECT, nonce: 'n1' }, source: null }))
  expect(posted).toEqual([])
  w.dispatchEvent(new w.MessageEvent('message', { data: { type: COLLECT, nonce: 'n1' }, source: w }))
  expect(posted).toEqual([{ type: COLLECT, nonce: 'n1', values: { 1: 'hunter2', 2: 'true', 9: 'not a field' } }])
  const raw = (posted[0] as { values: unknown }).values
  expect(pickValues(FIELDS, raw)).toEqual({ 1: 'hunter2', 2: 'true' })
  // A select the user changed sends its option (by text: the clone keeps no `value`); clearing a field sends "".
  el(4).selectedIndex = 1
  el(4).dispatchEvent(new w.Event('change', { bubbles: true }))
  type(1, '')
  el(2).checked = false
  w.dispatchEvent(new w.MessageEvent('message', { data: { type: COLLECT, nonce: 'n2' }, source: w }))
  expect(pickValues(FIELDS, (posted[1] as { values: unknown }).values)).toEqual({ 1: '', 2: 'false', 4: 'Belgium' })
  // A non-string value is dropped, never sent as something else.
  expect(pickValues(FIELDS, { 0: 5, 1: 'x' })).toEqual({ 1: 'x' })
})

test('Enter in a field, or the clone\'s own submit button, becomes a submit request; Enter in a textarea does not', () => {
  const { w, el, posted } = frame()
  const enter = (target: Element) => {
    const ev = new w.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
    target.dispatchEvent(ev)
    return ev.defaultPrevented
  }
  expect(enter(el(9))).toBe(false)
  expect(posted).toEqual([])
  expect(enter(el(1))).toBe(true)
  expect(posted).toEqual([{ type: SUBMIT_REQUEST }])
  el(3).click()
  expect(posted).toEqual([{ type: SUBMIT_REQUEST }, { type: SUBMIT_REQUEST }])
})

test('the host takes the values only from its frame and with its own nonce', async () => {
  let cb: ((ev: { source: unknown; data: unknown }) => void) | null = null
  const win = { postMessage: vi.fn() }
  const values = collectValues(win, FIELDS, (f) => ((cb = f), () => (cb = null)), 'n1', 1_000)
  expect(win.postMessage).toHaveBeenCalledWith({ type: COLLECT, nonce: 'n1' }, '*')
  cb!({ source: {}, data: { type: COLLECT, nonce: 'n1', values: { 0: 'forged' } } })
  cb!({ source: win, data: { type: COLLECT, nonce: 'other', values: { 0: 'stale' } } })
  cb!({ source: win, data: { type: COLLECT, nonce: 'n1', values: { 0: 'me@example.com', 1: 'pw' } } })
  await expect(values).resolves.toEqual({ 0: 'me@example.com', 1: 'pw' })
  expect(cb).toBeNull()
})

test('a resolved card keeps its outcome and origin; the receipt answers after a reload', () => {
  const s = useElementForms.getState()
  s.add(form())
  expect(useElementForms.getState().calls.c1).toBe('f1')
  s.resolve('f1', 'elsewhere')
  expect(useElementForms.getState().pending).toEqual({})
  // The engine's outcome replaces the stand-in and keeps what was known of the card.
  s.resolve('f1', 'stale')
  expect(useElementForms.getState().outcomes.f1).toEqual({ outcome: 'stale', origin: 'https://accounts.example.com', session: 's1' })
  // A late `Requested` after its `Resolved` is ignored.
  s.add(form())
  expect(useElementForms.getState().pending).toEqual({})
  expect(outcomeOfReceipt('submitted · https://a.example · filled 0 email, 1 password · now https://a.example/home — Home')).toBe('submitted')
  expect(outcomeOfReceipt('  submitted · https://a.example')).toBe('submitted')
  expect(outcomeOfReceipt('cancelled')).toBe('cancelled')
  expect(outcomeOfReceipt('no app showed the form; ask the user to open theuth')).toBe('cancelled')
  expect(outcomeOfReceipt('the page changed since the form was shown; nothing was typed')).toBe('stale')
  expect(outcomeOfReceipt('no answer')).toBe('timeout')
  expect(outcomeOfReceipt('element too large to show (300 KiB > 256 KiB); pick a smaller element')).toBeNull()
  expect(outcomeOfReceipt('see below\nsubmitted')).toBeNull()
})

test('a session\'s reload forgets its own calls and outcomes the engine no longer lists, and leaves other sessions alone', () => {
  const s = useElementForms.getState()
  s.add(form())
  s.add(form({ id: 'f2', call_id: 'c2' }))
  s.add(form({ id: 'g1', call_id: 'd1', session: 's2' }))
  s.resolve('f1', 'submitted')
  s.resolve('g1', 'cancelled')
  s.replace([form({ id: 'f2', call_id: 'c2' })], { session: 's1' })
  const after = useElementForms.getState()
  expect(Object.keys(after.pending)).toEqual(['f2'])
  expect(after.calls).toEqual({ c2: 'f2', d1: 'g1' })
  expect(Object.keys(after.outcomes)).toEqual(['g1'])
})

let stop: (() => void) | null = null
afterEach(() => {
  stop?.()
  stop = null
  conn.subs.clear()
})

test('a client that connects late asks the engine for the pending cards, and again on reconnect', async () => {
  const pending = vi.fn(async () => [form()])
  conn.s = { api: { element_forms_pending: pending }, state: 'open', discovery: { phase: 'ready' } }
  stop = startElementForms()
  await vi.waitFor(() => expect(useElementForms.getState().pending.f1?.origin).toBe('https://accounts.example.com'))
  expect(pending).toHaveBeenCalledWith()
  // Resolved elsewhere while this client was away: the reconnect's answer no longer lists it.
  pending.mockResolvedValueOnce([])
  conn.s = { ...conn.s, state: 'reconnecting' }
  for (const cb of conn.subs) cb()
  conn.s = { ...conn.s, state: 'open' }
  for (const cb of conn.subs) cb()
  await vi.waitFor(() => expect(useElementForms.getState().pending).toEqual({}))
  expect(pending).toHaveBeenCalledTimes(2)
})

test('a failed ask for a session\'s cards is not remembered: the next missing card asks again', async () => {
  const pending = vi.fn(async (_s?: string): Promise<ElementFormRequested[]> => {
    throw new Error('offline')
  })
  conn.s = { api: { element_forms_pending: pending }, state: 'open', discovery: { phase: 'ready' } }
  attachSession('s9')
  await vi.waitFor(() => expect(pending).toHaveBeenCalledTimes(1))
  pending.mockResolvedValueOnce([form({ session: 's9' })])
  // Once the failure is in, the same card's ask goes out again.
  await vi.waitFor(() => {
    attachSession('s9')
    expect(pending).toHaveBeenCalledTimes(2)
  })
  await vi.waitFor(() => expect(useElementForms.getState().pending.f1).toBeDefined())
  expect(pending).toHaveBeenCalledTimes(2)
  // Answered: asked again only after a reconnect.
  attachSession('s9')
  expect(pending).toHaveBeenCalledTimes(2)
})
