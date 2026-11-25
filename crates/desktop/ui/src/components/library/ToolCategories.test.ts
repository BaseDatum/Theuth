import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { afterEach, expect, test, vi } from 'vitest'
import { useDrafts } from '@/lib/drafts'
import type * as Rpc from '@/lib/rpc'
import type { ToolCategoryInfo } from '@/rpc/bindings/method.agent.output'
import { ToolCategories } from './ToolCategories'

vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))

const catalog = (): ToolCategoryInfo[] => [
  {
    id: 'engineering',
    label: 'Engineering',
    description: 'Files, shell and code',
    custom: false,
    default_enabled: true,
    hidden: false,
    builtin_count: 2,
    builtin_tools: ['read_file', 'write_file'],
    mcp_servers: [],
  },
  {
    id: 'web',
    label: 'Web',
    description: 'Search and browse',
    custom: false,
    default_enabled: false,
    hidden: false,
    builtin_count: 1,
    builtin_tools: ['search_web'],
    mcp_servers: [],
  },
  {
    id: 'research',
    label: 'Research',
    description: 'Custom investigations',
    custom: true,
    default_enabled: false,
    hidden: false,
    builtin_count: 0,
    builtin_tools: [],
    mcp_servers: ['linear'],
  },
]

let answer = catalog()
const calls: [string, unknown[]][] = []

vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    list_tool_categories: async (session: string | null) => {
      calls.push(['list_tool_categories', [session]])
      return answer.map((c) => ({ ...c, builtin_tools: [...c.builtin_tools], mcp_servers: [...c.mcp_servers] }))
    },
    upsert_tool_category: async (id: string | null, label: string, description: string | null) => {
      calls.push(['upsert_tool_category', [id, label, description]])
      if (id !== null) {
        const current = answer.find((c) => c.id === id)!
        const saved = { ...current, label, description: description ?? '' }
        answer = answer.map((c) => c.id === id ? saved : c)
        return saved
      }
      const slug = label.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')
      const saved: ToolCategoryInfo = {
        id: slug,
        label,
        description: description ?? '',
        custom: true,
        default_enabled: true,
        hidden: false,
        builtin_count: 0,
        builtin_tools: [],
        mcp_servers: [],
      }
      answer = [...answer, saved]
      return saved
    },
    delete_tool_category: async (id: string) => {
      calls.push(['delete_tool_category', [id]])
      answer = answer.filter((c) => c.id !== id)
      return null
    },
    set_default_tool_categories: async (disabled: string[]) => {
      calls.push(['set_default_tool_categories', [disabled]])
      answer = answer.map((c) => ({ ...c, default_enabled: !disabled.includes(c.id) }))
      return null
    },
    set_mcp_hidden: async (session: string, names: string[], hidden: boolean) => {
      calls.push(['set_mcp_hidden', [session, names, hidden]])
      answer = answer.map((c) => names.includes(`tcat:${c.id}`) ? { ...c, hidden } : c)
      return null
    },
    set_mcp_category: async (server: string, category: string | null) => {
      calls.push(['set_mcp_category', [server, category]])
      answer = answer.map((c) => ({
        ...c,
        mcp_servers: c.id === category ? [...c.mcp_servers.filter((s) => s !== server), server].sort() : c.mcp_servers.filter((s) => s !== server),
      }))
      return null
    },
  }),
}))

afterEach(() => {
  answer = catalog()
  calls.length = 0
  useDrafts.setState({ settings: {}, opening: null })
})

async function mount(session: string | null = null) {
  const el = document.body.appendChild(document.createElement('div'))
  const root = createRoot(el)
  const qc = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })
  await act(async () => root.render(createElement(QueryClientProvider, { client: qc, children: createElement(ToolCategories, { session }) })))
  await vi.waitFor(() => expect(el.textContent).toContain('Engineering'))

  const button = (label: string) => [...el.querySelectorAll('button')].find((b) => b.textContent === label)!
  const toggle = (label: string) => el.querySelector<HTMLButtonElement>(`[role="switch"][aria-label="${label}"]`)!
  const chat = (label: string) => el.querySelector<HTMLInputElement>(`input[type="checkbox"][aria-label="${label}"]`)!
  const input = (label: string) => el.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!
  const setInput = async (label: string, value: string) => {
    const field = input(label)
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(field, value)
      field.dispatchEvent(new Event('input', { bubbles: true }))
    })
  }
  const expand = async (label: string) => {
    const trigger = [...el.querySelectorAll('button')].find((b) => b.textContent?.startsWith(label))!
    await act(async () => trigger.click())
  }
  const unmount = () => {
    act(() => root.unmount())
    el.remove()
  }
  return { el, button, toggle, chat, input, setInput, expand, unmount }
}

const callsTo = (method: string) => calls.filter(([name]) => name === method)

test('creates a custom category from the visible label and description form', async () => {
  const { el, button, input, setInput, unmount } = await mount()
  await setInput('New category label', 'Analysis')
  await setInput('New category description', 'Compare evidence and results')
  await act(async () => button('Create').click())
  await vi.waitFor(() => expect(callsTo('upsert_tool_category')).toEqual([
    ['upsert_tool_category', [null, 'Analysis', 'Compare evidence and results']],
  ]))
  await vi.waitFor(() => expect(el.textContent).toContain('Analysis'))
  expect(input('New category label').value).toBe('')
  expect(input('New category description').value).toBe('')
  unmount()
})

test('a default switch sends the complete disabled id list, including categories already off', async () => {
  const { toggle, unmount } = await mount()
  const engineering = toggle('Default Engineering in new chats')
  expect(engineering.getAttribute('aria-checked')).toBe('true')
  await act(async () => engineering.click())
  await vi.waitFor(() => expect(callsTo('set_default_tool_categories')).toEqual([
    ['set_default_tool_categories', [['web', 'research', 'engineering']]],
  ]))
  await vi.waitFor(() => expect(toggle('Default Engineering in new chats').getAttribute('aria-checked')).toBe('false'))
  unmount()
})

test('this chat hides a live category through its tcat hide key', async () => {
  const { chat, unmount } = await mount('s1')
  const engineering = chat('Engineering in this chat')
  expect(engineering.checked).toBe(true)
  await act(async () => engineering.click())
  await vi.waitFor(() => expect(callsTo('set_mcp_hidden')).toEqual([
    ['set_mcp_hidden', ['s1', ['tcat:engineering'], true]],
  ]))
  await vi.waitFor(() => expect(chat('Engineering in this chat').checked).toBe(false))
  unmount()
})

test('an explicit false draft hide enables a default-disabled category without a session RPC', async () => {
  useDrafts.setState({ settings: { hidden: { 'tcat:web': false } }, opening: null })
  const { chat, unmount } = await mount('')
  const web = chat('Web in this chat')
  expect(web.checked).toBe(true)
  await act(async () => web.click())
  await vi.waitFor(() => expect(useDrafts.getState().settings.hidden?.['tcat:web']).toBe(true))
  await vi.waitFor(() => expect(chat('Web in this chat').checked).toBe(false))
  await vi.waitFor(() => expect(chat('Web in this chat').disabled).toBe(false))
  await act(async () => chat('Web in this chat').click())
  await vi.waitFor(() => expect(useDrafts.getState().settings.hidden?.['tcat:web']).toBe(false))
  await vi.waitFor(() => expect(chat('Web in this chat').checked).toBe(true))
  expect(callsTo('set_mcp_hidden')).toEqual([])
  unmount()
})

test('edits and deletes a custom category, refreshing the consumer-visible list', async () => {
  const { el, button, setInput, unmount } = await mount()
  await act(async () => button('Edit').click())
  await setInput('Edit Research label', 'Studies')
  await setInput('Edit Research description', 'Evidence and sources')
  await act(async () => button('Save').click())
  await vi.waitFor(() => expect(callsTo('upsert_tool_category')).toEqual([
    ['upsert_tool_category', ['research', 'Studies', 'Evidence and sources']],
  ]))
  await vi.waitFor(() => expect(el.textContent).toContain('Studies'))

  await act(async () => button('Delete').click())
  const confirmation = () => [...document.querySelectorAll('button')].find((b) => b.textContent === 'Delete category')
  await vi.waitFor(() => expect(confirmation()).toBeDefined())
  await act(async () => confirmation()!.click())
  await vi.waitFor(() => expect(callsTo('delete_tool_category')).toEqual([['delete_tool_category', ['research']]]))
  await vi.waitFor(() => expect(el.textContent).not.toContain('Studies'))
  unmount()
})

test('expands category assignments and unassigns an MCP server', async () => {
  const { el, button, expand, unmount } = await mount()
  await expand('Research')
  await vi.waitFor(() => expect(el.textContent).toContain('linear'))
  await act(async () => button('Unassign').click())
  await vi.waitFor(() => expect(callsTo('set_mcp_category')).toEqual([['set_mcp_category', ['linear', null]]]))
  await vi.waitFor(() => expect(el.textContent).toContain('No MCP servers assigned.'))
  unmount()
})
