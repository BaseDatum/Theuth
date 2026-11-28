import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { beforeEach, describe, expect, test, vi } from 'vitest'

const { openViewTileMock, selectMock, openPopoutMock, canPopoutMock, notifyMock } = vi.hoisted(() => ({
  openViewTileMock: vi.fn<(props: { path: string; title?: string }) => string | null>(),
  selectMock: vi.fn<(leaf: string) => void>(),
  openPopoutMock: vi.fn(),
  canPopoutMock: vi.fn(() => false),
  notifyMock: vi.fn(),
}))

vi.mock('@/components/tiles/open', () => ({
  openViewTile: openViewTileMock,
}))

vi.mock('@/lib/panes', () => ({
  tileLeaf: (id: string) => `tile:${id}`,
  usePanes: {
    getState: () => ({ select: selectMock }),
  },
}))

vi.mock('@/lib/popout', () => ({
  canPopout: canPopoutMock,
  openPopout: openPopoutMock,
}))

vi.mock('@/lib/notify', () => ({
  notify: notifyMock,
}))

vi.mock('@/components/tasks/queries', () => ({
  useMesh: () => ({ data: undefined }),
}))

vi.mock('@/lib/rpc', () => ({
  useApi: () => ({}),
}))

vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => vi.fn(),
}))

import { useUserTerminals } from '@/store/terminals'
import { CreateTerminalMenu, createUserTerminal, openTerminalPane, terminalNodes } from './CreateTerminal'

describe('terminalNodes', () => {
  test('always includes this machine enabled even when mesh_status has not loaded', () => {
    const nodes = terminalNodes(undefined)
    expect(nodes).toHaveLength(1)
    expect(nodes[0]).toMatchObject({
      self: true,
      reachable: true,
      name: 'This machine',
      status: 'this machine',
    })
  })

  test('lists this machine enabled, reachable peers enabled, and unreachable peers visible and disabled', () => {
    const now = 1_700_000_000_000
    const nodes = terminalNodes(
      {
        self_id: 'self-engine',
        self_name: 'mac-studio',
        peers: [
          {
            id: 'self-engine',
            name: 'mac-studio',
            reachable: true,
            path: 'lan',
            last_seen_ms: now,
          } as never,
          {
            id: 'peer-linux',
            name: 'build-box',
            reachable: true,
            path: 'relay',
            last_seen_ms: now,
          } as never,
          {
            id: 'peer-offline',
            name: 'home-nas',
            reachable: false,
            path: null,
            last_seen_ms: now - 120_000,
          } as never,
        ],
      },
      now,
    )

    expect(nodes.map((n) => ({ id: n.id, name: n.name, self: n.self, reachable: n.reachable }))).toEqual([
      { id: 'self-engine', name: 'mac-studio', self: true, reachable: true },
      { id: 'peer-linux', name: 'build-box', self: false, reachable: true },
      { id: 'peer-offline', name: 'home-nas', self: false, reachable: false },
    ])
    expect(nodes[1]?.status).toBe('via relay')
    expect(nodes[2]?.status).toContain('offline')
  })
})

describe('CreateTerminalMenu rendering', () => {
  test('renders reachable nodes enabled, unreachable peers visible and disabled, and inline monospace alert on error', () => {
    const html = renderToStaticMarkup(
      createElement(CreateTerminalMenu, {
        nodes: [
          { id: 'self-1', name: 'mac-studio', self: true, reachable: true, status: 'this machine' },
          { id: 'peer-1', name: 'build-box', self: false, reachable: true, status: 'LAN' },
          { id: 'peer-2', name: 'home-nas', self: false, reachable: false, status: 'offline 2m ago' },
        ],
        terminals: [{ engine: 'self-1', shell: 'term-abcd', name: 'mac-studio terminal', node: 'mac-studio' }],
        pending: null,
        error: 'connection refused: peer-1',
        onCreate: () => {},
        onOpen: () => {},
      }),
    )

    // All three nodes are visible; only the unreachable peer button has disabled.
    expect(html).toContain('data-terminal-node="self-1"')
    expect(html).toContain('data-terminal-node="peer-1"')
    expect(html).toContain('data-terminal-node="peer-2" disabled=""')
    expect(html).not.toContain('data-terminal-node="self-1" disabled=""')
    expect(html).not.toContain('data-terminal-node="peer-1" disabled=""')

    // Inline error alert in monospace (no toast / notify).
    expect(html).toContain('role="alert"')
    expect(html).toContain('font-mono')
    expect(html).toContain('connection refused: peer-1')
    expect(notifyMock).not.toHaveBeenCalled()

    // Created terminals list is rendered so the owner can reopen them.
    expect(html).toContain('data-user-terminal="self-1:term-abcd"')
    expect(html).toContain('mac-studio terminal')
  })
})

describe('createUserTerminal and openTerminalPane', () => {
  beforeEach(() => {
    useUserTerminals.getState().clear()
    openViewTileMock.mockReset()
    selectMock.mockReset()
    openPopoutMock.mockReset()
    canPopoutMock.mockReturnValue(false)
    notifyMock.mockReset()
  })

  test('calls api.engine_shell(engine, freshShell), stores terminal, opens workspace pane even when canPopout() is false, and never calls engine_terminal or navigates to /sessions or /new', async () => {
    openViewTileMock.mockReturnValue('view::/terminal/peer-1/term-1111?name=build-box%20terminal')
    const engine_shell = vi.fn(async (engine?: string | null, shell?: string | null) => ({
      engine: engine ?? 'self-1',
      shell: shell ?? 'term-1111',
    }))
    const engine_terminal = vi.fn()
    const navigate = vi.fn()

    const node = { id: 'peer-1', name: 'build-box', self: false, reachable: true, status: 'LAN' }
    const created = await createUserTerminal({ engine_shell, engine_terminal } as never, node, navigate)

    expect(engine_shell).toHaveBeenCalledTimes(1)
    expect(engine_shell).toHaveBeenCalledWith('peer-1', created.shell)
    expect(created.shell).not.toBe('main')
    expect(created).toEqual({
      engine: 'peer-1',
      shell: created.shell,
      name: 'build-box terminal',
      node: 'build-box',
    })

    // Stored in session store for sidebar & composer attach picker.
    expect(useUserTerminals.getState().terminals).toEqual([created])

    // Opens a workspace pane on /terminal/$engine/$shell (never an OS popout, even when canPopout() is false).
    expect(openViewTileMock).toHaveBeenCalledWith({
      path: `/terminal/peer-1/${created.shell}?name=build-box%20terminal`,
      title: 'build-box terminal',
    })
    expect(selectMock).toHaveBeenCalledWith('tile:view::/terminal/peer-1/term-1111?name=build-box%20terminal')
    expect(openPopoutMock).not.toHaveBeenCalled()
    expect(engine_terminal).not.toHaveBeenCalled()
    expect(navigate).not.toHaveBeenCalled()
  })

  test('two terminals created on the same node get distinct non-"main" shell ids', async () => {
    openViewTileMock.mockImplementation(({ path }) => `view::${path}`)
    const engine_shell = vi.fn(async (engine?: string | null, shell?: string | null) => ({
      engine: engine ?? 'self-1',
      shell: shell ?? 'fallback',
    }))
    const node = { id: 'self-1', name: 'mac-studio', self: true, reachable: true, status: 'this machine' }

    const first = await createUserTerminal({ engine_shell }, node)
    const second = await createUserTerminal({ engine_shell }, node)

    expect(first.shell).not.toBe('main')
    expect(second.shell).not.toBe('main')
    expect(first.shell).not.toBe(second.shell)
    expect(useUserTerminals.getState().terminals).toHaveLength(2)
  })

  test('falls back to in-app navigation to /terminal/$engine/$shell when no workspace is mounted', () => {
    openViewTileMock.mockReturnValue(null)
    const navigate = vi.fn()
    const leaf = openTerminalPane(
      { engine: 'self-1', shell: 'term-xyz', name: 'mac-studio terminal', node: 'mac-studio' },
      navigate,
    )
    expect(leaf).toBeNull()
    expect(navigate).toHaveBeenCalledWith({
      to: '/terminal/$engine/$shell',
      params: { engine: 'self-1', shell: 'term-xyz' },
      search: { name: 'mac-studio terminal' },
    })
    expect(openPopoutMock).not.toHaveBeenCalled()
  })

  test('does not store terminal, open pane, or toast when engine_shell rejects', async () => {
    const engine_shell = vi.fn(async () => {
      throw new Error('shell spawn failed')
    })
    const navigate = vi.fn()
    const node = { id: 'peer-1', name: 'build-box', self: false, reachable: true, status: 'LAN' }

    await expect(createUserTerminal({ engine_shell }, node, navigate)).rejects.toThrow('shell spawn failed')
    expect(useUserTerminals.getState().terminals).toEqual([])
    expect(openViewTileMock).not.toHaveBeenCalled()
    expect(navigate).not.toHaveBeenCalled()
    expect(notifyMock).not.toHaveBeenCalled()
  })
})
