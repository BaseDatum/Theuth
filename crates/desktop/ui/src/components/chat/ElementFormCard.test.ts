import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { type Root, createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { COLLECT, type ElementFormRequested, useElementForms } from '@/lib/element-form'
import type { Entry } from '@/store/sessions'
import { ElementFormCard } from './ElementFormCard'

const conn = vi.hoisted(() => ({ s: { api: null as unknown, state: 'open', discovery: { phase: 'ready' } } as { api: unknown; state: string; discovery: unknown } }))
vi.mock('@/store/connection', () => {
  const useConnection = (sel: (s: unknown) => unknown = (s) => s) => sel(conn.s)
  useConnection.getState = () => conn.s
  useConnection.subscribe = () => () => {}
  return { useConnection }
})
vi.mock('@/rpc/discovery', async (original) => ({ ...(await original<object>()), supports: () => true, ready: () => true }))

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

beforeEach(() => useElementForms.getState().clear())
afterEach(async () => {
  await unmount()
  vi.restoreAllMocks()
  document.body.innerHTML = ''
})

test('the app draws the origin in ASCII, the source and buttons that say where the values go', async () => {
  conn.s = { api: {}, state: 'open', discovery: { phase: 'ready' } }
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  expect(host.textContent).toContain('https://xn--pple-43d.com')
  expect(host.textContent).toContain('agent browser on studio')
  const submit = host.querySelector<HTMLButtonElement>('button[aria-label="Submit to xn--pple-43d.com"]')
  expect(submit?.disabled).toBe(false)
  expect(host.querySelector('button[aria-label="Cancel: nothing is sent"]')).not.toBeNull()
  expect(host.querySelector('[aria-label="Encrypted connection"]')).not.toBeNull()
  await unmount()
  // A form without a web origin cannot be submitted from the card.
  useElementForms.getState().clear()
  useElementForms.getState().add({ ...FORM, origin: 'null' })
  const opaque = await mount(tool())
  expect(opaque.querySelector<HTMLButtonElement>('button[aria-label="Submit to this page"]')?.disabled).toBe(true)
})

test('Submit sends the typed values only to answer_element_form: no console line, no store, no storage holds them', async () => {
  const answer = vi.fn(async () => true)
  conn.s = { api: { answer_element_form: answer }, state: 'open', discovery: { phase: 'ready' } }
  const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((k) => vi.spyOn(console, k))
  useElementForms.getState().add(FORM)
  const host = await mount(tool())
  const iframe = host.querySelector('iframe')!
  expect(iframe.getAttribute('sandbox')).toBe('allow-scripts allow-forms')
  expect(iframe.getAttribute('srcdoc')).toContain('Content-Security-Policy')
  // The frame answers `theuth/collect` as the bootstrap would, with what the user typed.
  const win = iframe.contentWindow!
  vi.spyOn(win, 'postMessage').mockImplementation(((msg: { type: string; nonce: string }) => {
    if (msg.type !== COLLECT) return
    // jsdom's MessageEvent refuses a frame's window as `source`: a plain event carrying the same fields.
    const ev = new Event('message')
    Object.defineProperties(ev, { source: { value: win }, data: { value: { type: COLLECT, nonce: msg.nonce, values: { 0: 'me@example.com', 1: SECRET, 2: 'x' } } } })
    window.dispatchEvent(ev)
  }) as typeof win.postMessage)
  await act(async () => host.querySelector<HTMLButtonElement>('button[aria-label="Submit to xn--pple-43d.com"]')!.click())
  await vi.waitFor(() => expect(answer).toHaveBeenCalledTimes(1))
  expect(answer).toHaveBeenCalledWith('f1', { submit: { values: { 0: 'me@example.com', 1: SECRET } } }, 's1')
  // Sent: the frame (and what was typed into it) is gone.
  await vi.waitFor(() => expect(host.querySelector('iframe')).toBeNull())
  for (const spy of spies) for (const call of spy.mock.calls) expect(JSON.stringify(call)).not.toContain(SECRET)
  expect(JSON.stringify(useElementForms.getState())).not.toContain(SECRET)
  expect(JSON.stringify({ ...localStorage })).not.toContain(SECRET)
  expect(JSON.stringify({ ...sessionStorage })).not.toContain(SECRET)
  expect(host.innerHTML).not.toContain(SECRET)
  // The engine resolves it: the card says how it ended, and never a value.
  await act(async () => useElementForms.getState().resolve('f1', 'submitted'))
  expect(host.textContent).toContain('Submitted to https://xn--pple-43d.com')
  expect(host.innerHTML).not.toContain(SECRET)
  await unmount()
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
    useElementForms.getState().clear()
    useElementForms.getState().add(FORM)
    useElementForms.getState().resolve('f1', outcome)
    const html = await render(tool())
    expect(html).toContain(text)
    expect(html).not.toContain('<iframe')
  }
  useElementForms.getState().clear()
  const reloaded = await render(tool({ pending: false, result: { output: 'submitted · https://a.example · filled 0 email, 1 password · now https://a.example/home — Home', isError: false } } as Partial<Tool>))
  expect(reloaded).toContain('Submitted')
  expect(reloaded).toContain('filled 0 email, 1 password')
  const refused = await render(tool({ pending: false, result: { output: 'element too large to show (300 KiB > 256 KiB); pick a smaller element', isError: true } } as Partial<Tool>))
  expect(refused).toContain('element too large to show')
})

test('a card waiting on a form this window never saw asks for its session\'s pending forms', async () => {
  const pending = vi.fn(async () => [FORM])
  conn.s = { api: { element_forms_pending: pending }, state: 'open', discovery: { phase: 'ready' } }
  const host = await mount(tool())
  await vi.waitFor(() => expect(pending).toHaveBeenCalledWith('s1'))
  await vi.waitFor(() => expect(host.querySelector('iframe')).not.toBeNull())
  await unmount()
})
