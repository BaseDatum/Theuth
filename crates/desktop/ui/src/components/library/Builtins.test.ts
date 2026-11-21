import { readyConnection } from '@/components/settings/test-engine'
import { useConnection } from '@/store/connection'
import type * as ConnectionModule from '@/store/connection'
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, beforeAll, expect, test, vi } from 'vitest'
import type { BuiltinCategory, BuiltinTool } from '@/lib/builtins'
import { useDrafts } from '@/lib/drafts'
import type * as Rpc from '@/lib/rpc'
import { Builtins } from './Builtins'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))

const tool = (name: string, description: string, flags: Partial<BuiltinTool> = {}): BuiltinTool => ({
  name,
  description,
  risk: 'read_only',
  direct: true,
  disabled: false,
  hidden: false,
  ...flags,
})
const catalog = (flags: Partial<BuiltinTool> = {}): BuiltinCategory[] => [
  {
    id: 'files',
    label: 'Files',
    description: 'Read, search and edit files on this machine',
    tools: [tool('read_file', 'Read a text file', flags), tool('write_file', 'Write a file', flags), tool('grep', 'Search file contents', flags)],
  },
  {
    id: 'web',
    label: 'Web',
    description: 'Search and read the web',
    tools: [tool('search_web', 'Search the web'), tool('read_url', 'Fetch a page', { direct: false, risk: 'mutating' })],
  },
]
let answer = catalog()
/** How often the list was fetched, and whether `set_builtins_disabled` fails. */
let fetched = 0
let failAlways = false

const calls: [string, unknown[]][] = []
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    list_builtins: async () => {
      fetched++
      return answer
    },
    set_builtins_disabled: async (...args: unknown[]) => {
      calls.push(['set_builtins_disabled', args])
      if (failAlways) throw new Error('config.yaml is read-only')
      return null
    },
    set_mcp_hidden: async (...args: unknown[]) => {
      calls.push(['set_mcp_hidden', args])
      return null
    },
  }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

// The engine answers `list_builtins` (206), whatever the exported contract of this checkout lists.
beforeAll(() => {
  type Discovery = { document: { methods: { name: string }[] } }
  const store = useConnection as unknown as { getState(): { discovery: Discovery }; setState(p: object): void }
  const d = store.getState().discovery
  const missing = ['list_builtins', 'set_builtins_disabled'].filter((name) => !d.document.methods.some((m) => m.name === name))
  const methods = [...d.document.methods, ...missing.map((name) => ({ name }))]
  store.setState({ discovery: { ...d, document: { ...d.document, methods } } })
})

afterEach(() => {
  calls.length = 0
  answer = catalog()
  fetched = 0
  failAlways = false
  useDrafts.setState({ settings: {}, opening: null })
})

async function mount(session: string | null) {
  const el = document.body.appendChild(document.createElement('div'))
  const root = createRoot(el)
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc, children: createElement(Builtins, { session }) })))
  await vi.waitFor(() => expect(el.textContent).toContain('Files'))
  /** The `label` item of the On / Off here / Off always group of a category label or a tool name. */
  const choice = (row: string, label: string) =>
    [...el.querySelectorAll<HTMLButtonElement>(`[aria-label="${row}: on or off"] button`)].find((b) => b.textContent === label)!
  const expand = (label: string) => act(async () => [...el.querySelectorAll('button')].find((b) => b.textContent?.startsWith(label))!.click())
  const search = (text: string) =>
    act(async () => {
      const input = el.querySelector<HTMLInputElement>('[aria-label="Search built-in tools"]')!
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, text)
      input.dispatchEvent(new Event('input', { bubbles: true }))
    })
  const unmount = () => {
    act(() => root.unmount())
    el.remove()
  }
  return { el, choice, expand, search, unmount }
}

test('lists each category with its count and its tools with risk, direct badge and description', async () => {
  answer = catalog()
  answer[0]!.tools[1] = { ...answer[0]!.tools[1]!, hidden: true }
  const { el, expand, unmount } = await mount('s1')
  expect(el.textContent).toContain('Read, search and edit files on this machine')
  expect(el.textContent).toContain('2 of 3 on')
  expect(el.textContent).toContain('2 of 2 on')
  await expand('Web')
  await vi.waitFor(() => expect(el.textContent).toContain('Fetch a page'))
  expect(el.textContent).toContain('mutating')
  expect(el.textContent).toContain('read-only')
  expect(el.textContent).toContain('direct')
  // A category with tools in different states presses no item and says so.
  expect(el.textContent).toContain('Mixed')
  unmount()
})

test('Settings without a chat: "Off here" is greyed with the way to the chat’s sheet; "Off always" is harness-wide', async () => {
  const { el, choice, unmount } = await mount(null)
  expect(choice('Files', 'Off here').disabled).toBe(true)
  expect(el.querySelector('[title="Open from a chat: ⋯ › Tools… or /tools"]')).not.toBeNull()
  await act(async () => choice('Files', 'Off always').click())
  await vi.waitFor(() => expect(calls).toEqual([['set_builtins_disabled', [['cat:files'], true]]]))
  unmount()
})

test('in a chat, "Off here" hides the category for that chat by its key', async () => {
  const { choice, unmount } = await mount('s1')
  expect(choice('Files', 'Off here').disabled).toBe(false)
  await act(async () => choice('Files', 'Off here').click())
  await vi.waitFor(() => expect(calls).toEqual([['set_mcp_hidden', ['s1', ['cat:files'], true]]]))
  unmount()
})

test('one tool on inside an always-off category: the others get their own entries before the key goes', async () => {
  answer = catalog({ disabled: true })
  const { choice, expand, unmount } = await mount(null)
  expect(choice('Files', 'Off always').getAttribute('data-state')).toBe('on')
  await expand('Files')
  await vi.waitFor(() => expect(choice('grep', 'On')).toBeDefined())
  await act(async () => choice('grep', 'On').click())
  await vi.waitFor(() =>
    expect(calls).toEqual([
      ['set_builtins_disabled', [['read_file', 'write_file'], true]],
      ['set_builtins_disabled', [['cat:files', 'grep'], false]],
    ]),
  )
  unmount()
})

test('search matches tool names, descriptions and category labels, and opens what matches', async () => {
  const { el, search, unmount } = await mount('s1')
  await search('fetch')
  await vi.waitFor(() => expect(el.textContent).toContain('read_url'))
  expect(el.textContent).not.toContain('search_web')
  expect(el.textContent).not.toContain('Files')
  // A matching label shows the whole category.
  await search('files')
  await vi.waitFor(() => expect(el.textContent).toContain('write_file'))
  expect(el.textContent).not.toContain('Web')
  await search('nothing like it')
  await vi.waitFor(() => expect(el.textContent).toContain('No built-in tool matches "nothing like it".'))
  unmount()
})

test('a draft chat keeps "Off here" in the draft and calls no session RPC', async () => {
  const { choice, unmount } = await mount('')
  await act(async () => choice('Files', 'Off here').click())
  await vi.waitFor(() => expect(useDrafts.getState().settings.hidden).toEqual({ 'cat:files': true }))
  await vi.waitFor(() => expect(choice('Files', 'Off here').getAttribute('data-state')).toBe('on'))
  expect(calls).toEqual([])
  unmount()
})

test('U-2: a multi-call switch whose second call fails still refetches the list, so the row shows the engine', async () => {
  answer = catalog({ disabled: true })
  failAlways = true
  const { choice, unmount } = await mount('s1')
  expect(fetched).toBe(1)
  // Off here from Off always: this chat's key goes on first, then the harness-wide entries come off (and fail).
  await act(async () => choice('Files', 'Off here').click())
  await vi.waitFor(() =>
    expect(calls).toEqual([
      ['set_mcp_hidden', ['s1', ['cat:files'], true]],
      ['set_builtins_disabled', [['cat:files', 'read_file', 'write_file', 'grep'], false]],
    ]),
  )
  await vi.waitFor(() => expect(fetched).toBe(2))
  unmount()
})

test('search also matches a category description and id; a row opened by the search can be closed', async () => {
  const { el, search, expand, unmount } = await mount('s1')
  await search('the web')
  await vi.waitFor(() => expect(el.textContent).toContain('read_url'))
  expect(el.textContent).not.toContain('read_file')
  await search('files')
  await vi.waitFor(() => expect(el.textContent).toContain('write_file'))
  // The user's click wins over the search until the text changes.
  await expand('Files')
  await vi.waitFor(() => expect(el.textContent).not.toContain('write_file'))
  await search('file')
  await vi.waitFor(() => expect(el.textContent).toContain('write_file'))
  unmount()
})

test('Settings without a chat says, in visible text, where "Off here" lives', async () => {
  const { el, unmount } = await mount(null)
  expect(el.textContent).toContain('"Off here" needs a chat. Open from a chat: ⋯ › Tools… or /tools')
  unmount()
})
