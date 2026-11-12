import { readyConnection } from '@/components/settings/test-engine'
import type * as ConnectionModule from '@/store/connection'
import '@/components/settings/test-dom'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, test, vi } from 'vitest'
import type * as Rpc from '@/lib/rpc'
import { queryKeys } from '@/lib/rpc'
import { LOCAL, useEngines } from '@/link/engines'
import type { McpServer } from "@/rpc/bindings/method.mcp_2dserver.output"
import type { McpToolInfo } from "@/rpc/bindings/method.mcp_2dserver.output"
import type { MeshStatus } from "@/rpc/bindings/method.nodes.output"
import { Tools } from './Tools'

vi.mock('sonner', () => ({ toast: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn() }) }))

// 187: this is xeus; mikrotik was added on 333.
const mesh = {
  self_id: 'id-xeus',
  self_name: 'xeus',
  peers: [{ id: 'id-333', name: '333', reachable: true, last_seen_ms: 0 }],
} as unknown as MeshStatus
const server = (over: Partial<McpServer> = {}): McpServer => ({
  name: 'mikrotik',
  connected: false,
  gate: null,
  tools: [],
  repo: null,
  auth: 'none',
  home: 'id-333',
  home_name: '333',
  home_here: false,
  everywhere: false,
  state: 'elsewhere',
  reason: null,
  ...over,
})
const servers: McpServer[] = [
  server(),
  server({ name: 'grafana', home: 'id-xeus', home_name: 'xeus', home_here: true, state: 'running', connected: true, auth: 'logged_in' }),
  server({ name: 'portable', everywhere: true, state: 'unavailable_here', reason: 'portable-mcp is not installed on xeus' }),
  server({ name: 'linear', home_here: true, state: 'error', auth: 'required' }),
]
// The chat `s1`'s view: one running server with two tools, none hidden yet.
const tool = (name: string): McpToolInfo => ({ name, description: '', gate: null, risk: 'read_only', action: 'allow', registered: name, hidden: false })
const chat: McpServer[] = [
  server({ name: 'fs', home: 'id-xeus', home_name: 'xeus', home_here: true, state: 'running', connected: true, tools: [tool('read_file'), tool('write_file')] }),
]

const calls: [string, unknown][] = []
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  useApi: () => ({
    list_mcp: async (session: string | null) => (session ? chat : servers),
    set_mcp_hidden: async (session: string, names: string[], hidden: boolean) => {
      calls.push(['set_mcp_hidden', [session, names, hidden]])
      return null
    },
    mesh_status: async () => mesh,
    mcp_status: async () => ({ connecting: [], connected: [], failed: [], homes_problem: '/d/mcp/homes.yaml is invalid: line 1' }),
    set_mcp_home: async (name: string, action: string) => {
      calls.push(['set_mcp_home', [name, action]])
      return null
    },
    mcp_login: async (name: string) => {
      calls.push(['mcp_login', name])
      return `https://auth.example/${name}`
    },
    mcp_logout: async (name: string) => {
      calls.push(['mcp_logout', name])
      return null
    },
  }),
}))
vi.mock('@/store/connection', async (original) => {
  const m = await original<typeof ConnectionModule>()
  return { ...m, useConnection: readyConnection(m.createConnection()) }
})

const client = () => new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } })

async function mount(session: string | null = null) {
  const el = document.createElement('div')
  document.body.append(el)
  const root = createRoot(el)
  await act(async () => root.render(createElement(QueryClientProvider, { client: client(), children: createElement(Tools, { session }) })))
  await vi.waitFor(() => {
    expect(el.textContent).toContain(session ? 'fs2 tools' : 'portable-mcp is not installed on xeus')
    expect(el.textContent).toContain('homes.yaml is invalid: line 1')
    if (!session) expect(el.textContent).toContain('Move home here')
  })
  const button = (label: string) => [...el.querySelectorAll('button')].filter((b) => b.textContent === label)
  const unmount = () => {
    act(() => root.unmount())
    el.remove()
  }
  return { el, button, unmount }
}

afterEach(() => {
  calls.length = 0
  useEngines.setState({ active: LOCAL })
  vi.restoreAllMocks()
})

test('Settings without a chat: "Off here" is greyed with the way to the chat’s own sheet', async () => {
  const { el, button, unmount } = await mount()
  expect(button('Off here')[0]?.disabled).toBe(true)
  expect(el.querySelector('[title="Open from a chat: ⋯ › Tools… or /tools"]')).not.toBeNull()
  unmount()
})

test('in a chat, "Off here" hides every tool of the server and a tool’s switch hides that tool, both for that chat', async () => {
  const { el, button, unmount } = await mount('s1')
  const hides = () => calls.filter(([m]) => m === 'set_mcp_hidden')
  expect(button('Off here')[0]?.disabled).toBe(false)
  await act(async () => button('Off here')[0]!.click())
  await vi.waitFor(() => expect(hides()).toEqual([['set_mcp_hidden', ['s1', ['read_file', 'write_file'], true]]]))
  // The tools open under the row; the first one's switch turns it off here.
  await act(async () => [...el.querySelectorAll('button')].find((b) => b.textContent?.includes('fs'))!.click())
  const switches = () => [...el.querySelectorAll<HTMLButtonElement>('[role="switch"]')]
  await vi.waitFor(() => expect(switches()).toHaveLength(2))
  expect(switches()[0]!.disabled).toBe(false)
  await act(async () => switches()[0]!.click())
  await vi.waitFor(() => expect(hides()[1]).toEqual(['set_mcp_hidden', ['s1', ['read_file'], true]]))
  unmount()
})

test('188 D2: no home chip and no home actions until the mesh answers (no "on this machine" flicker)', () => {
  const qc = client()
  qc.setQueryData(queryKeys.mcp(null), servers)
  const html = renderToStaticMarkup(createElement(QueryClientProvider, { client: qc, children: createElement(Tools, { session: null }) }))
  expect(html).toContain('mikrotik')
  expect(html).not.toMatch(/>on (xeus|333|this machine)</)
  expect(html).not.toContain('Move home here')
})

test('188 D2: each server row names its home and state here; a home action sets the home of its own row', async () => {
  const { el, button, unmount } = await mount()
  for (const t of ['on 333', 'not running here (home is 333)', 'on xeus', 'not installed here']) expect(el.innerHTML).toContain(t)
  expect(button('Move home here')).toHaveLength(2)
  expect(button('Run everywhere')[0]?.className).toContain('pointer-coarse:min-h-11')
  const homes = () => calls.filter(([m]) => m === 'set_mcp_home')
  await act(async () => button('Move home here')[0]!.click())
  await vi.waitFor(() => expect(homes()).toEqual([['set_mcp_home', ['mikrotik', 'move_here']]]))
  await vi.waitFor(() => expect(button('Only on 333')[0]?.disabled).toBe(false))
  await act(async () => button('Only on 333')[0]!.click())
  await vi.waitFor(() =>
    expect(homes()).toEqual([
      ['set_mcp_home', ['mikrotik', 'move_here']],
      ['set_mcp_home', ['portable', 'only_home']],
    ]),
  )
  unmount()
})

test('188 D2: Log in opens the sign-in URL here; over link it is shown to open on the node; Log out logs out', async () => {
  const open = vi.spyOn(window, 'open').mockReturnValue(null)
  const { el, button, unmount } = await mount()
  expect(el.textContent).toContain('logged in')
  await act(async () => button('Log in')[0]!.click())
  await vi.waitFor(() => expect(open).toHaveBeenCalledWith('https://auth.example/linear', '_blank'))
  expect(el.textContent).not.toContain('Sign in from the node')
  await act(async () => useEngines.setState({ active: 'remote-node' }))
  await vi.waitFor(() => expect(button('Log in')[0]?.disabled).toBe(false))
  await act(async () => button('Log in')[0]!.click())
  await vi.waitFor(() => expect(el.textContent).toContain('Sign in from the node (node): https://auth.example/linear'))
  expect(open).toHaveBeenCalledTimes(1)
  await act(async () => button('Log out')[0]!.click())
  await vi.waitFor(() => expect(calls).toContainEqual(['mcp_logout', 'grafana']))
  unmount()
})
