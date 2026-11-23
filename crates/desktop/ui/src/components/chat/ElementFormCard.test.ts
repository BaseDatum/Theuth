import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { type Root, createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { COLLECT, type ElementFormRequested, SIZE, SUBMIT_REQUEST } from '@/lib/element-form'
import { useElementForms } from '@/store/element-forms'
import type { Entry } from '@/store/sessions'
import { ElementFormCard } from './ElementFormCard'
import { buildRows } from './rows'

const conn = vi.hoisted(() => ({ s: { api: null as unknown, state: 'open', discovery: { phase: 'ready' } } as { api: unknown; state: string; discovery: unknown } }))
// The real module's other exports stay: the per-engine scope creates its connection store with them.
vi.mock('@/store/connection', async (original) => {
  const useConnection = (sel: (s: unknown) => unknown = (s) => s) => sel(conn.s)
  useConnection.getState = () => conn.s
  useConnection.subscribe = () => () => {}
  return { ...(await original<object>()), useConnection }
})
/** An open connection to an engine whose API is `api` (plus `element_form_shown`, which every card calls). */
const open = (api: object) => ({ api: { element_form_shown: async () => true, ...api }, state: 'open', discovery: { phase: 'ready' } })
/** What the engine supports: an older engine has no `answer_element_form`. */
const caps = vi.hoisted(() => ({ answer: true }))
vi.mock('@/rpc/discovery', async (original) => ({
  ...(await original<object>()),
  supports: (_d: unknown, r: { methods?: string[] }) => caps.answer || !r.methods?.includes('answer_element_form'),
  ready: () => true,
}))
/** The shell: off (a `srcdoc` frame) unless a test turns it on; `pagePut` as the test sets it. */
const shell = vi.hoisted(() => ({ tauri: false, put: (async () => 'theuth-page://x') as (call: string, doc: string) => Promise<string> }))
vi.mock('@/lib/tauri', () => ({ isTauri: () => shell.tauri }))
vi.mock('@/lib/pages', () => ({ pagePut: (call: string, doc: string) => shell.put(call, doc), pageDrop: async () => {} }))
// The card names the agent browser's engine from the mesh (`obscura:studio` → "studio").
vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: { self_id: 'me', self_name: 'mac', peers: [{ id: 'studio', name: 'studio', reachable: true }] } }) }))
const toasts = vi.hoisted(() => [] as string[])
vi.mock('sonner', () => ({ toast: Object.assign((m: string) => void toasts.push(m), { error: (m: string) => void toasts.push(m) }) }))

type Tool = Extract<Entry, { kind: 'tool' }>
const SECRET = 'hunter2-correct-horse'
const FIELDS = [
  { index: 0, kind: 'email', name: 'email', label: 'Email', masked: false },
  { index: 1, kind: 'password', name: 'pw', label: 'Password', masked: true },
  { index: 2, kind: 'submit', name: '', label: 'Sign in', masked: false },
]
const FORM: ElementFormRequested = {
  type: 'ElementFormRequested',
  id: 'f1',
  session: 's1',
  call_id: 'c1',
  origin: 'https://xn--pple-43d.com',
  action_origin: 'https://xn--pple-43d.com',
  url: 'https://xn--pple-43d.com/login',
  source: 'obscura:studio',
  title: 'Sign in',
  html: '<form><input type="email" data-theuth-field="0"><input type="password" data-theuth-field="1"><button data-theuth-field="2">Sign in</button></form>',
  width: 320,
  height: 180,
  fields: FIELDS,
}
const tool = (over: Partial<Tool> = {}): Tool => ({ id: 'e1', kind: 'tool', callId: 'c1', name: 'browser_show_element', args: {}, pending: true, child: false, ...over }) as Tool

let root: Root | null = null
/** Mount the card (zustand hooks read the live store only in a client render) and return its host. */
const mount = async (t: Tool): Promise<HTMLElement> => {
  const host = document.body.appendChild(document.createElement('div'))
  root = createRoot(host)
  await act(async () => root!.render(h(ElementFormCard, { tool: t, session: 's1' })))
  return host
}
const unmount = async () => {
  const r = root
  root = null
  if (r) await act(async () => r.unmount())
}
const button = (host: HTMLElement, label: string) => host.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)
const SUBMIT = 'Submit to xn--pple-43d.com'
const CANCEL = 'Cancel: nothing is sent'

/** A message from the card's frame to the app window (jsdom's MessageEvent refuses a frame's window as `source`). */
const fromFrame = (win: Window, data: unknown) => {
  const ev = new Event('message')
  Object.defineProperties(ev, { source: { value: win }, data: { value: data } })
  window.dispatchEvent(ev)
}
/** Make the frame answer `theuth/collect` as the bootstrap would, with `values`. */
const answering = (host: HTMLElement, values: Record<string, string>) => {
  const win = host.querySelector('iframe')!.contentWindow!
  vi.spyOn(win, 'postMessage').mockImplementation(((msg: { type: string; nonce: string }) => {
    if (msg.type === COLLECT) fromFrame(win, { type: COLLECT, nonce: msg.nonce, values })
  }) as typeof win.postMessage)
  return win
}

beforeEach(() => {
  useElementForms.setState({ pending: {}, outcomes: {}, calls: {} })
  caps.answer = true
  shell.tauri = false
  toasts.length = 0
})
afterEach(async () => {
  await unmount()
  vi.restoreAllMocks()
  document.body.innerHTML = ''
})

test('the app draws the origin in ASCII, the source, the agent\'s title as its words, where the values go, and tells the engine the card is shown', async () => {
  const shown = vi.fn(async () => true)
  conn.s = { api: { element_form_shown: shown }, state: 'open', discovery: { phase: 'ready' } }
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  expect(host.textContent).toContain('https://xn--pple-43d.com')
  expect(host.textContent).toContain('agent browser on studio')
  expect(host.textContent).toContain('the agent calls it “Sign in”')
  expect(host.textContent).toContain('Sends to')
  expect(host.textContent).not.toContain('not to https://')
  expect(button(host, SUBMIT)?.disabled).toBe(false)
  expect(button(host, CANCEL)).not.toBeNull()
  expect(host.querySelector('[aria-label="Encrypted connection"]')).not.toBeNull()
  // Shown: once per card per connection, with the request's session.
  expect(shown).toHaveBeenCalledWith('f1', 's1')
  await unmount()
  await mount(tool())
  expect(shown).toHaveBeenCalledTimes(1)
  await unmount()
  // A form without a web origin cannot be submitted from the card.
  useElementForms.setState({ pending: {}, outcomes: {}, calls: {} })
  useElementForms.getState().add({ ...FORM, origin: 'null', action_origin: 'null' })
  const opaque = await mount(tool())
  expect(button(opaque, 'Submit to this page')?.disabled).toBe(true)
})

test('a form that sends to another origin than the page says so, and Submit names where it goes', async () => {
  conn.s = open({})
  useElementForms.getState().add({ ...FORM, action_origin: 'https://sso.evil.example' })
  const host = await mount(tool())
  expect(host.textContent).toContain('This form sends what you type to https://sso.evil.example, not to https://xn--pple-43d.com')
  expect(button(host, 'Submit to sso.evil.example')).not.toBeNull()
})

test('Submit sends only the typed values to answer_element_form: no console line, no store, no storage holds them', async () => {
  const answer = vi.fn(async () => true)
  conn.s = open({ answer_element_form: answer })
  const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((k) => vi.spyOn(console, k))
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  const iframe = host.querySelector('iframe')!
  expect(iframe.getAttribute('sandbox')).toBe('allow-scripts allow-forms')
  expect(iframe.getAttribute('srcdoc')).toContain('Content-Security-Policy')
  // The content width is the element's own.
  expect(iframe.style.maxWidth).toBe('336px')
  answering(host, { 0: 'me@example.com', 1: SECRET, 2: 'x' })
  await act(async () => button(host, SUBMIT)!.click())
  await vi.waitFor(() => expect(answer).toHaveBeenCalledTimes(1))
  expect(answer).toHaveBeenCalledWith('f1', { submit: { values: { 0: 'me@example.com', 1: SECRET } } }, 's1')
  // Sent: the frame (and what was typed into it) is gone; the wait is announced.
  await vi.waitFor(() => expect(host.querySelector('iframe')).toBeNull())
  expect(host.querySelector('[role="status"]')?.textContent).toContain('Waiting for xn--pple-43d.com')
  for (const spy of spies) for (const call of spy.mock.calls) expect(JSON.stringify(call)).not.toContain(SECRET)
  expect(JSON.stringify(useElementForms.getState())).not.toContain(SECRET)
  expect(JSON.stringify({ ...localStorage })).not.toContain(SECRET)
  expect(JSON.stringify({ ...sessionStorage })).not.toContain(SECRET)
  expect(host.innerHTML).not.toContain(SECRET)
  // The engine resolves it: the card says how it ended, never a value, and the outcome takes the focus.
  await act(async () => useElementForms.getState().resolve('f1', 'submitted'))
  expect(host.textContent).toContain('Submitted to https://xn--pple-43d.com')
  expect(document.activeElement?.getAttribute('role')).toBe('status')
  expect(host.innerHTML).not.toContain(SECRET)
})

test('Enter in the frame submits; a Submit with nothing typed is not sent', async () => {
  const answer = vi.fn(async () => true)
  conn.s = open({ answer_element_form: answer })
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  const win = answering(host, {})
  await act(async () => fromFrame(win, { type: SUBMIT_REQUEST }))
  await vi.waitFor(() => expect(toasts).toEqual(['Nothing to submit: type into the form, or Cancel']))
  expect(answer).not.toHaveBeenCalled()
  expect(host.querySelector('iframe')).not.toBeNull()
  // A request from any other window is not the frame's.
  answering(host, { 0: 'me@example.com' })
  await act(async () => fromFrame(window, { type: SUBMIT_REQUEST }))
  expect(answer).not.toHaveBeenCalled()
  await act(async () => fromFrame(host.querySelector('iframe')!.contentWindow!, { type: SUBMIT_REQUEST }))
  await vi.waitFor(() => expect(answer).toHaveBeenCalledWith('f1', { submit: { values: { 0: 'me@example.com' } } }, 's1'))
})

test('the frame takes the height it reports, within bounds', async () => {
  conn.s = open({})
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  const iframe = host.querySelector('iframe')!
  expect(iframe.style.height).toBe('196px')
  await act(async () => fromFrame(iframe.contentWindow!, { type: SIZE, height: 412.3 }))
  expect(iframe.style.height).toBe('413px')
  await act(async () => fromFrame(iframe.contentWindow!, { type: SIZE, height: 5000 }))
  expect(iframe.style.height).toBe('900px')
  await act(async () => fromFrame(iframe.contentWindow!, { type: SIZE, height: 10 }))
  expect(iframe.style.height).toBe('80px')
})

test('Cancel answers cancel; it stays live after a Submit; an answer another window got first resolves the card at once', async () => {
  const answer = vi.fn(async (_id: string, a: object) => !('cancel' in a))
  conn.s = open({ answer_element_form: answer })
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  answering(host, { 0: 'me@example.com' })
  await act(async () => button(host, SUBMIT)!.click())
  await vi.waitFor(() => expect(host.querySelector('iframe')).toBeNull())
  // After Submit, Cancel is still pressable: the first answer won, so the engine says no and the card keeps waiting.
  expect(button(host, CANCEL)?.disabled).toBe(false)
  await act(async () => button(host, CANCEL)!.click())
  await vi.waitFor(() => expect(answer).toHaveBeenLastCalledWith('f1', { cancel: {} }, 's1'))
  expect(toasts).toEqual(['Already submitted: waiting for the page'])
  expect(host.textContent).toContain('Waiting for xn--pple-43d.com')
  await unmount()

  // Another window answered first: this one's Submit is refused, and the card ends without a spinner.
  useElementForms.setState({ pending: {}, outcomes: {}, calls: {} })
  answer.mockImplementation(async () => false)
  useElementForms.getState().add(FORM)
  const other = await mount(tool())
  answering(other, { 0: 'me@example.com' })
  await act(async () => button(other, SUBMIT)!.click())
  await vi.waitFor(() => expect(other.textContent).toContain('Answered in another window'))
  expect(other.querySelector('iframe')).toBeNull()
  // The engine's outcome replaces it when it comes.
  await act(async () => useElementForms.getState().resolve('f1', 'submitted'))
  expect(other.textContent).toContain('Submitted to https://xn--pple-43d.com')
  await unmount()

  // Cancel before anything: `{ cancel: {} }` with the session, then "Cancelling…" until the engine resolves it.
  useElementForms.setState({ pending: {}, outcomes: {}, calls: {} })
  answer.mockImplementation(async () => true)
  useElementForms.getState().add(FORM)
  const third = await mount(tool())
  await act(async () => button(third, CANCEL)!.click())
  await vi.waitFor(() => expect(answer).toHaveBeenLastCalledWith('f1', { cancel: {} }, 's1'))
  expect(third.textContent).toContain('Cancelling…')
})

test('an engine without answer_element_form shows the card but offers no answer', async () => {
  caps.answer = false
  const shown = vi.fn(async () => true)
  conn.s = { api: { element_form_shown: shown }, state: 'open', discovery: { phase: 'ready' } }
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  expect(host.querySelector('iframe')).not.toBeNull()
  expect(button(host, SUBMIT)?.disabled).toBe(true)
  expect(button(host, CANCEL)?.disabled).toBe(true)
})

test('in the shell, a page that cannot be stored says so instead of a frame', async () => {
  shell.tauri = true
  shell.put = async () => {
    throw new Error('page store full')
  }
  conn.s = open({})
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  await vi.waitFor(() => expect(host.textContent).toContain('Unable to show the form'))
  expect(host.querySelector('iframe')).toBeNull()
})

test('each outcome renders its line; after a reload the receipt stands in', async () => {
  const render = async (t: Tool) => {
    const html = (await mount(t)).innerHTML
    await unmount()
    return html
  }
  const cases = [
    ['cancelled', 'Cancelled: nothing was sent'],
    ['stale', 'The page changed before it could be filled: nothing was typed'],
    ['timeout', 'Not answered in time: nothing was sent'],
  ] as const
  for (const [outcome, text] of cases) {
    useElementForms.setState({ pending: {}, outcomes: {}, calls: {} })
    useElementForms.getState().add(FORM)
    useElementForms.getState().resolve('f1', outcome)
    const html = await render(tool())
    expect(html).toContain(text)
    expect(html).not.toContain('<iframe')
  }
  useElementForms.setState({ pending: {}, outcomes: {}, calls: {} })
  const reloaded = await render(tool({ pending: false, result: { output: 'submitted · https://a.example · filled 0 email, 1 password · now https://a.example/home — Home', isError: false } } as Partial<Tool>))
  expect(reloaded).toContain('Submitted')
  expect(reloaded).toContain('filled 0 email, 1 password')
  const refused = await render(tool({ pending: false, result: { output: 'element too large to show (300 KiB > 256 KiB); pick a smaller element', isError: true } } as Partial<Tool>))
  expect(refused).toContain('element too large to show')
})

test('a card waiting on a form this window never saw asks for its session\'s pending forms', async () => {
  const pending = vi.fn(async () => [FORM])
  conn.s = open({ element_forms_pending: pending })
  const host = await mount(tool())
  await vi.waitFor(() => expect(pending).toHaveBeenCalledWith('s1'))
  await vi.waitFor(() => expect(host.querySelector('iframe')).not.toBeNull())
})

test('a subagent\'s form is a loose row of its parent\'s transcript, labelled with the child; the asking session\'s own is not', () => {
  const child = { ...FORM, id: 'f2', session: 'child-1' }
  const from = { id: 'child-1', name: 'researcher' } as never
  const rows = buildRows([], [], 's1', { 'child-1': from }, true, undefined, [FORM, child, { ...FORM, id: 'f3', session: 'stranger' }])
  expect(rows).toEqual([{ key: 'f2', form: child, from }])
})
