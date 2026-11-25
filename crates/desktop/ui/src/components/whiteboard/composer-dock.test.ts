// 218 composer: the whiteboard mounts the session view's own `Composer` (not a pill/sheet of its own) with the tools
// button leading its toolbar and the session picker just above its box; a send goes through the whiteboard's send path into the picked
// session, or a new one made with the composer's model/effort pick, bound, then sent.
import '../settings/test-dom'
import { QueryClientProvider, QueryClient } from '@tanstack/react-query'
import { act, createElement as h } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Y from 'yjs'
import { TooltipProvider } from '@/components/ui/tooltip'
import { NO_PICK, useDrafts } from '@/lib/drafts'
import type * as Rpc from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { WhiteboardShell } from './WhiteboardView'

const rpc = vi.hoisted(() => ({ api: {} as Record<string, unknown> }))
vi.mock('@/lib/rpc', async (original) => ({ ...(await original<typeof Rpc>()), useApi: () => rpc.api }))
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<object>()), useNavigate: () => () => undefined, useRouterState: () => '/' }))

window.HTMLElement.prototype.scrollIntoView = () => {}
Element.prototype.setPointerCapture ??= () => {}

const DOC = 'wb-doc-1'
/** Every engine call in order: `[method, ...params]` (the api's and the whiteboard RPCs'). */
let calls: unknown[][]
let bound: { session: string; title: string }[]
let root: Root
let el: HTMLDivElement

const answers: Record<string, (...a: unknown[]) => unknown> = {
  list_models: () => [{ id: 'm/one', name: 'One', provider: 'm', context: 0, reasoning: true, image: true, efforts: ['low', 'high'], budget_min: null, budget_max: null, local: false }],
  list_sessions: () => [],
  list_skills: () => [],
  vision_model: () => null,
  list_mcp: () => [],
  list_terminals: () => [],
  load_session: () => new Promise(() => {}),
  create_session: () => ({ session: 'new-1' }),
  send_input: (s: unknown) => s ?? 'made-by-send',
  steer: () => null,
}

beforeEach(() => {
  calls = []
  bound = []
  rpc.api = new Proxy(
    {},
    {
      get: (_, name: string) =>
        async (...a: unknown[]) => {
          calls.push([name, ...a])
          return answers[name]?.(...a) ?? null
        },
    },
  )
  const call = async (m: string, params: unknown[]) => {
    calls.push([m, ...params])
    return m === 'whiteboard_sessions' ? bound : m === 'whiteboard_turn' ? null : null
  }
  useConnection.setState({
    client: { call } as never,
    discovery: { ...useConnection.getState().discovery, phase: 'legacy' },
    info: { favorite_models: [], default_model: 'm/one' } as never,
  })
  useDrafts.setState({ pick: NO_PICK, by: {} })
})

afterEach(() => {
  act(() => root.unmount())
  el.remove()
  document.body.innerHTML = ''
})

async function mount() {
  el = document.createElement('div')
  document.body.append(el)
  const doc = new Y.Doc()
  const provider = { doc, meta: { title: 'Plan', kind: 'board' }, onStatus: () => () => {} }
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  root = createRoot(el)
  await act(async () =>
    root.render(h(QueryClientProvider, { client: qc }, h(TooltipProvider, null, h(WhiteboardShell, { doc: DOC, provider: provider as never, frame: 'relative size-full', embedded: true })))),
  )
  // The canvas is lazy: its handle is what the send plans with.
  await vi.waitFor(() => expect(el.querySelector('.react-flow__pane')).not.toBeNull())
}

const q = <T extends Element = HTMLButtonElement>(sel: string) => document.querySelector<T>(sel)
const click = (e: Element | null) =>
  act(async () => {
    expect(e).not.toBeNull()
    ;(e as HTMLElement).click()
  })
/** Types into the composer's box as a user would (React reads the native value setter). */
async function type(text: string) {
  const ta = q<HTMLTextAreaElement>('[data-whiteboard-composer] textarea')!
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(ta, text)
    ta.dispatchEvent(new Event('input', { bubbles: true }))
  })
}
const send = () => click(q('[data-whiteboard-composer] button[aria-label="Send"]'))
const method = (m: string) => calls.filter((c) => c[0] === m)
const index = (m: string) => calls.findIndex((c) => c[0] === m)

describe('the whiteboard composer is the session composer', () => {
  it('mounts the chat Composer (box, model, send) with the tools button on its toolbar and the session picker above the box, and no pill or sheet', async () => {
    await mount()
    const dock = q<HTMLDivElement>('[data-whiteboard-composer]')!
    expect(dock.querySelector('[data-composer] textarea')).not.toBeNull()
    expect(dock.querySelector('[data-composer-bar] button[aria-label="Add"]')).not.toBeNull()
    expect(dock.querySelector('button[aria-label="Send"]')).not.toBeNull()
    const bar = dock.querySelector('[data-composer-bar]')!
    expect(bar.firstElementChild?.getAttribute('aria-label')).toBe('Tools: Pen')
    const box = dock.querySelector('[data-composer]')!
    const picker = dock.querySelector('button[aria-label="New session. Choose a session"]')!
    expect(picker).not.toBeNull()
    expect(picker.compareDocumentPosition(box) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(q('[role="dialog"][aria-label="Message the agent about this whiteboard"]')).toBeNull()
    expect(q('button[aria-label="Show controls"]')).toBeNull()
  })

  it('the tools button opens the ink panel with Hand and the cluster actions; Hand shows on the button', async () => {
    await mount()
    await click(q('button[aria-label="Tools: Pen"]'))
    for (const label of ['Hand', 'Pen', 'Fine liner', 'Marker', 'Eraser', 'Add card', 'Link mode', 'Frame', 'Fit', 'Undo', 'More']) expect(q(`button[aria-label="${label}"]`), label).not.toBeNull()
    await click(q('button[aria-label="Marker"]'))
    expect(q('button[aria-label="Tools: Marker"]')!.dataset.armed).toBe('true')
    await click(q('button[aria-label="Hand"]'))
    expect(q('button[aria-label="Tools: Hand"]')).not.toBeNull()
  })

  it('a session picked in the dropdown takes the send, with the whiteboard marker and its context', async () => {
    bound = [
      { session: 's-alpha', title: 'Alpha' },
      { session: 's-beta', title: 'Beta' },
    ]
    await mount()
    await vi.waitFor(() => expect(q('button[aria-label^="Session: Alpha"]')).not.toBeNull())
    await click(q('button[aria-label^="Session: Alpha"]'))
    await click([...document.querySelectorAll('[aria-label="Sessions"] button')].find((b) => b.textContent === 'Beta') ?? null)
    await vi.waitFor(() => expect(q('button[aria-label^="Session: Beta"]')).not.toBeNull())

    await type('What do these mean?')
    await send()
    await vi.waitFor(() => expect(method('send_input')).toHaveLength(1))
    const [, session, body] = method('send_input')[0]!
    expect(session).toBe('s-beta')
    expect(body).toContain(DOC) // the whiteboard marker leads (it binds and survives compaction)
    expect(body).toContain('Plan')
    expect(body).toContain('What do these mean?')
    // The context the injection reads goes first, for the same session and whiteboard.
    const context = method('whiteboard_send_context')
    expect(context).toHaveLength(1)
    expect(context[0]!.slice(1, 3)).toEqual(['s-beta', DOC])
    expect(context[0]![3]).toMatchObject({ selected: [], images: [], omitted: 0 })
    expect(index('whiteboard_send_context')).toBeLessThan(index('send_input'))
    expect(method('whiteboard_bind')).toHaveLength(0) // already bound
    expect(method('create_session')).toHaveLength(0)
    // The box is cleared once the engine took it.
    await vi.waitFor(() => expect(q<HTMLTextAreaElement>('[data-whiteboard-composer] textarea')!.value).toBe(''))
  })

  it('New session: the first send creates the session with the composer’s model and effort, binds it, sends, and targets it', async () => {
    await mount()
    expect(q('button[aria-label="New session. Choose a session"]')).not.toBeNull()
    act(() => useDrafts.setState({ pick: { model: 'm/one', effort: 'high' } }))
    await type('Start here')
    await send()
    await vi.waitFor(() => expect(method('send_input')).toHaveLength(1))
    expect(method('create_session')[0]![1]).toMatchObject({ model: 'm/one', effort: 'high' })
    expect(method('whiteboard_bind')[0]!.slice(1)).toEqual(['new-1', DOC])
    expect(method('send_input')[0]![1]).toBe('new-1')
    expect(index('create_session')).toBeLessThan(index('whiteboard_bind'))
    expect(index('whiteboard_bind')).toBeLessThan(index('whiteboard_send_context'))
    expect(index('whiteboard_send_context')).toBeLessThan(index('send_input'))
    // The pick is spent on that session, which the whiteboard now talks to.
    expect(useDrafts.getState().pick).toEqual(NO_PICK)
    await vi.waitFor(() => expect(q('button[aria-label^="Session: "]')).not.toBeNull())
  })
})
