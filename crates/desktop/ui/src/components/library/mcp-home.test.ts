import { expect, test } from 'vitest'
import { engines } from '@/components/tasks/home'
import type { McpServer } from '@/rpc/bindings/McpServer'
import type { MeshStatus } from '@/rpc/bindings/MeshStatus'
import { homeActions, homeChip, stateText } from './mcp-home'

// 187: this is xeus; mikrotik was added on 333.
const mesh = {
  self_id: 'id-xeus',
  self_name: 'xeus',
  peers: [{ id: 'id-333', name: '333', reachable: true, last_seen_ms: 0n }],
} as unknown as MeshStatus
const MACHINES = engines(mesh)
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

test('187: each row names its home and what it does on this machine', () => {
  expect([homeChip(server(), MACHINES), stateText(server(), MACHINES)]).toEqual(['on 333', 'not running here (home is 333)'])
  // The mesh's current name wins; the stamped one stands in for an engine the mesh does not know.
  expect(homeChip(server({ home: 'id-gone', home_name: 'old-box' }), MACHINES)).toBe('on old-box')
  expect(homeChip(server({ home: null, home_name: null }), MACHINES)).toBe('home not claimed yet')
  expect(homeChip(server({ home_here: true }), MACHINES)).toBe('on xeus')
  const states = (['running', 'connecting', 'off_here', 'off', 'unavailable_here', 'error'] as const).map((state) => stateText(server({ state }), MACHINES))
  expect(states).toEqual(['running', 'starting…', 'turned off here', 'off', 'not installed here', 'error'])
})

test('187: actions follow the row: Run everywhere toggles, Only on <home>, Move home here, Turn off / on here', () => {
  const acts = (s: McpServer, machines = MACHINES) => homeActions(s, machines).map((a) => [a.action, a.label, a.pressed ?? null])
  // Homed elsewhere and not running here: nothing to turn off.
  expect(acts(server())).toEqual([
    ['run_everywhere', 'Run everywhere', false],
    ['move_here', 'Move home here', null],
  ])
  expect(acts(server({ everywhere: true, state: 'unavailable_here' }))).toEqual([
    ['only_home', 'Run everywhere', true],
    ['only_home', 'Only on 333', null],
    ['move_here', 'Move home here', null],
    ['off_here', 'Turn off here', null],
  ])
  // Homed here and running: nothing to move; turned off here: turn it back on.
  expect(acts(server({ home_here: true, state: 'running' }))).toEqual([
    ['run_everywhere', 'Run everywhere', false],
    ['off_here', 'Turn off here', null],
  ])
  expect(acts(server({ home_here: true, state: 'off_here' }))).toEqual([
    ['run_everywhere', 'Run everywhere', false],
    ['on_here', 'Turn on here', null],
  ])
  // Alone (unpaired, or a reset identity): only a home on a machine nobody knows can be taken.
  const alone = engines({ ...mesh, peers: [] })
  expect(acts(server(), alone)).toEqual([['move_here', 'Move home here', null]])
  expect(acts(server({ home: null }), alone)).toEqual([['move_here', 'Move home here', null]])
  expect(acts(server({ home_here: true, state: 'running' }), alone)).toEqual([])
})
