import { expect, test } from 'vitest'
import { placement, pluginInfo, snap } from '@/components/plugins/test-fixtures'
import { EMPTY_PLUGINS, commandsOf, placementKey, placementsOf, reduce, seed } from './plugins'

const turns = pluginInfo('turns', [placement('turns')])
const seeded = seed(EMPTY_PLUGINS, 0, [turns], [snap('turns', 'turns', { revision: 3 })])
const k = placementKey('turns', 'turns')

test('a seed takes the list and the snapshot of its epoch', () => {
  expect(seeded.plugins).toEqual([turns])
  expect(seeded.status[k]?.dot).toBe('ok')
  expect(seeded.revision[k]).toBe(3)
  expect(seeded.loaded).toBe(0)
})

test('PluginStatus replaces dot, badge, label and tooltip, keeping the content revision', () => {
  const s = reduce(seeded, { type: 'PluginStatus', plugin: 'turns', placement: 'turns', dot: 'warn', badge: 51, label: '51/50', tooltip: 'Turns today: 51' })
  expect(s.status[k]).toEqual(snap('turns', 'turns', { dot: 'warn', badge: 51, label: '51/50', tooltip: 'Turns today: 51', revision: 3 }))
  // A placement never seen before gets a fresh snapshot.
  const fresh = reduce(EMPTY_PLUGINS, { type: 'PluginStatus', plugin: 'x', placement: 'y', dot: 'error', badge: null, label: null, tooltip: 'exited' })
  expect(fresh.status[placementKey('x', 'y')]).toMatchObject({ dot: 'error', tooltip: 'exited', revision: 0 })
})

test('PluginSurface announces a newer revision only; an older or equal one changes nothing', () => {
  const s = reduce(seeded, { type: 'PluginSurface', plugin: 'turns', placement: 'turns', format: 'openui', revision: 7 })
  expect(s.revision[k]).toBe(7)
  expect(s.status[k]).toMatchObject({ revision: 7, format: 'openui' })
  expect(reduce(s, { type: 'PluginSurface', plugin: 'turns', placement: 'turns', format: 'openui', revision: 7 })).toBe(s)
  expect(reduce(s, { type: 'PluginSurface', plugin: 'turns', placement: 'turns', format: 'openui', revision: 2 })).toBe(s)
})

test('PluginInvalidate counts per placement', () => {
  const once = reduce(seeded, { type: 'PluginInvalidate', plugin: 'turns', placement: 'turns' })
  expect(reduce(once, { type: 'PluginInvalidate', plugin: 'turns', placement: 'turns' }).invalidated[k]).toBe(2)
  expect(once.invalidated[placementKey('turns', 'other')]).toBeUndefined()
})

test('PluginState patches the plugin in place; an unknown plugin and PluginsChanged reload the list', () => {
  const s = reduce(seeded, { type: 'PluginState', plugin: 'turns', state: 'backoff', error: 'exit 1' })
  expect(s.plugins?.[0]).toMatchObject({ state: 'backoff', error: 'exit 1' })
  expect(s.epoch).toBe(seeded.epoch)
  expect(reduce(seeded, { type: 'PluginState', plugin: 'new', state: 'starting', error: null }).epoch).toBe(seeded.epoch + 1)
  const changed = reduce(seeded, { type: 'PluginsChanged' })
  expect(changed.epoch).toBe(seeded.epoch + 1)
  expect(changed.loaded).not.toBe(changed.epoch)
})

test('placements: ordered by `order`, hidden ones and unapproved or disabled plugins left out', () => {
  const plugins = [
    pluginInfo('b', [placement('b1'), placement('b2', { hidden: true })], { order: 1 }),
    pluginInfo('a', [placement('a1'), placement('a-status', { kind: 'status' })], { order: 0 }),
    pluginInfo('c', [placement('c1')], { order: 2, state: 'awaiting_approval', needs_approval: true }),
    pluginInfo('d', [placement('d1')], { order: 3, enabled: false, state: 'disabled' }),
    pluginInfo('e', [placement('e1')], { order: 4, state: 'failed' }),
  ]
  expect(placementsOf(plugins, {}, 'sidebar').map((e) => e.placement.id)).toEqual(['a1', 'b1', 'e1'])
  expect(placementsOf(plugins, {}, 'status').map((e) => e.placement.id)).toEqual(['a-status'])
})

test('commands: a hidden command placement still runs', () => {
  const info = { name: 'reset', slash: '/turns:reset', description: 'Reset the count', args: null }
  const plugins = [pluginInfo('turns', [placement('reset', { kind: 'command', hidden: true, format: null, command: info })])]
  expect(commandsOf(plugins)).toEqual([{ plugin: 'turns', command: info }])
})
