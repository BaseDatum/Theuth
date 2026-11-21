// Item (c), mounted: the original position presses the newest version (its ref, the engine's digest), and the later
// reply's stub scrolls to the original position.
import '../settings/test-dom'
import { act, createElement as h } from 'react'
import { createRoot } from 'react-dom/client'
import { Streamdown } from 'streamdown'
import { expect, test, vi } from 'vitest'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { FenceContext } from '@/components/chat/surface/JsonRenderFence'
import { useConnection } from '@/store/connection'
import { active } from '@/store/engine-scope'
import type { Entry } from '@/store/sessions'
import { cellDomId } from './Cell'
import { CELL_LANGUAGES, CellFence, CellOrdinals, cellOrdinals } from './CellFence'
import { cells, refKey, scopeKey } from './store'
import { REPLY, SESSION, chatRef, fakeApi, state } from './test-fixtures'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children, useNavigate: () => () => {} }))
vi.mock('@/rpc/capabilities', () => ({ useQuerySupport: () => true }))

const PREVIEW_V2 = '#| name: preview-ir-prod1\n#| label: Preview v2\n#| cwd: /tmp\npulumi preview -s ir-prod1 --diff'
const LATER = `Updated preview.\n\n\`\`\`bash\n${PREVIEW_V2}\n\`\`\`\n`
const reply = (id: string, seq: number, text: string) => ({ id, kind: 'assistant', text, open: false, context: { target: { kind: 'message', seq } } }) as unknown as Entry
const plugins = { renderers: [{ language: CELL_LANGUAGES, component: CellFence }] }
const message = (id: string, entry: string, text: string) =>
  h(
    FenceContext,
    { value: { running: false, hasLaterAssistant: false, session: SESSION, entry: id } },
    h(CellOrdinals, { value: cellOrdinals(text, entry, false) }, h('div', { 'data-message': id }, h(Streamdown, { mode: 'static', plugins }, text))),
  )

test('item (c): the original position runs the newest version; the later reply’s stub scrolls there', async () => {
  const replaced = chatRef(2)
  const newest = chatRef(1, '99')
  const pressed = Promise.withResolvers<unknown[]>()
  const { api } = fakeApi({ cell_run: (...a) => pressed.resolve(a) })
  useConnection.setState({ api } as never)
  active().sessions.setState((st) => ({ sessions: { ...st.sessions, [SESSION]: { ...st.sessions[SESSION], entries: [reply('a1', 42, REPLY), reply('a2', 99, LATER)] } as never } }))
  cells.setState({
    scopes: {
      [scopeKey({ type: 'session', id: SESSION })]: {
        states: { [refKey(replaced)]: state(replaced, { superseded_by: newest }), [refKey(newest)]: state(newest, { digest: 'd99' }) } as never,
        kernels: [],
        error: null,
        notices: {},
      },
    },
  })
  const scrolled: Element[] = []
  Element.prototype.scrollIntoView = function (this: Element) {
    scrolled.push(this)
  }
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  await act(async () => root.render(h(QueryClientProvider, { client: new QueryClient() }, h('div', null, message('a1', '42', REPLY), message('a2', '99', LATER)))))

  // The later reply: one stub line, no cell of its own.
  const later = host.querySelector('[data-message="a2"]') as HTMLElement
  expect(later.querySelector('[data-cell]')).toBeNull()
  const stub = later.querySelector('[data-cell-restated]') as HTMLButtonElement
  expect(stub.textContent).toBe('↑ updated cell preview-ir-prod1 above')

  // The original position carries the newest ref's DOM id and presses it with the engine's digest.
  const original = host.querySelector(`#${cellDomId(newest)}`) as HTMLElement
  expect(host.querySelector('[data-message="a1"]')?.contains(original)).toBe(true)
  const button = [...original.querySelectorAll('button')].find((b) => b.textContent === 'Preview v2') as HTMLButtonElement
  await act(async () => button.click())
  expect((await pressed.promise).slice(0, 2)).toEqual([newest, 'd99'])

  await act(async () => stub.click())
  expect(scrolled).toEqual([original])

  await act(async () => root.unmount())
  host.remove()
  cells.setState({ scopes: {} })
})
