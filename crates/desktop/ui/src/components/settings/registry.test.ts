import { afterEach, expect, test, vi } from 'vitest'
// The registry imports every tab component; some stores read `localStorage` at import; no DOM in the runner.
vi.hoisted(() => {
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })
})
import { CATEGORIES, MOVED_TABS, TAB_IDS, isTab, shownCategories, shownTab, shownTabs } from './registry'
import type { Discovery } from '@/rpc/discovery'

const tabs = CATEGORIES.flatMap((category) => [...category.tabs])
const ids = tabs.map((tab) => tab.id)

// Each case sets every tab's `visible` itself: a real one (`hasLocal()`) reads the container, which node has not.
const original = new Map(tabs.map((tab) => [tab, tab.visible]))
const hide = (...hidden: string[]) => {
  for (const tab of tabs) tab.visible = () => !hidden.includes(tab.id)
}
afterEach(() => {
  for (const [tab, visible] of original) tab.visible = visible
})

test('tab ids are unique across categories, and TAB_IDS lists them in column order', () => {
  expect(new Set(ids).size).toBe(ids.length)
  expect(ids).toEqual([...TAB_IDS])
})

test('188: thirteen categories in column order (#3 Prompt after Agent, 210 Sharing after Network, 216 Plugins last), each with 1 to 6 tabs', () => {
  expect(CATEGORIES.map((category) => category.id)).toEqual(['general', 'models', 'agent', 'prompt', 'learning', 'voice', 'meetings', 'calendar', 'network', 'sharing', 'channels', 'tools', 'plugins'])
  for (const category of CATEGORIES) {
    expect(category.tabs.length).toBeGreaterThanOrEqual(1)
    expect(category.tabs.length).toBeLessThanOrEqual(7)
  }
})

test('every old id redirects to a tab and is no tab itself', () => {
  for (const [old, to] of Object.entries(MOVED_TABS)) {
    expect(isTab(to), `${old} → ${to}`).toBe(true)
    expect(isTab(old), old).toBe(false)
  }
})

test('Sky 2026-11-11: Tools opens on MCP (the servers theuth uses, at `tools`), then 206 Builtins; Serve over MCP keeps `mcp`; 209 Browser last', () => {
  const tools = CATEGORIES.find((category) => category.id === 'tools')
  expect(tools?.tabs.map((tab) => [tab.id, tab.label])).toEqual([
    ['categories', 'Categories'],
    ['tools', 'MCP'],
    ['builtins', 'Builtins'],
    ['search', 'Web search'],
    ['mcp', 'Serve over MCP'],
    ['computer', 'Computer use'],
    ['browser', 'Browser'],
  ])
})

test('Prompt holds Suffixes and System, and System declares the prompt section methods', () => {
  const prompt = CATEGORIES.find((category) => category.id === 'prompt')!
  expect(prompt.tabs.map((tab) => [tab.id, tab.label])).toEqual([
    ['suffixes', 'Suffixes'],
    ['system', 'System'],
  ])
  const system = prompt.tabs.find((tab) => tab.id === 'system')!
  expect(system.required?.methods).toEqual(['list_prompt_sections', 'set_prompt_section', 'reset_prompt_section'])
})

test('216: Plugins is one tab, at `plugins`, needing the plugins methods and the `plugins.ui` topic', () => {
  const plugins = CATEGORIES.find((category) => category.id === 'plugins')!
  expect(plugins.tabs.map((tab) => [tab.id, tab.label])).toEqual([['plugins', 'Plugins']])
  expect(plugins.tabs[0].required?.methods).toContain('plugin_approve')
  expect(plugins.tabs[0].required?.topics).toEqual(['plugins.ui@1'])
})

test('S4: with every tab shown, every category is listed and every tab lands on itself', () => {
  hide()
  expect(shownCategories()).toEqual(CATEGORIES)
  for (const id of ids) expect(shownTab(id)).toBe(id)
})

test('S4: a hidden tab leaves its band and lands on its category’s first shown tab', () => {
  hide('machine', 'vms')
  const network = CATEGORIES.find((category) => category.id === 'network')
  expect(network && shownTabs(network).map((tab) => tab.id)).toEqual(['machines', 'devices', 'messaging'])
  expect(shownTab('machine')).toBe('machines')
  expect(shownTab('vms')).toBe('machines')
  expect(shownTab('contacts')).toBe('contacts')
})

test('213: Sharing holds the shared items in contract order; Network keeps the switches', () => {
  const tabsOf = (id: string) => CATEGORIES.find((category) => category.id === id)?.tabs.map((tab) => [tab.id, tab.label])
  expect(tabsOf('sharing')).toEqual([
    ['shared', 'Overview'],
    ['contacts', 'Contacts'],
    ['shared-chats', 'Chats and agents'],
    ['shared-notes', 'Notes and notebooks'],
    ['sharing', 'Public shares'],
    ['domains', 'Domains'],
  ])
  expect(tabsOf('network')?.map(([id]) => id)).toEqual(['machine', 'machines', 'devices', 'messaging', 'vms'])
})

test('213: an engine without sharing_list hides Overview and Notes; Contacts leads Sharing', () => {
  hide()
  const sharing = CATEGORIES.find((category) => category.id === 'sharing')!
  const methods = [...new Set(sharing.tabs.flatMap((tab) => tab.required?.methods ?? []))]
  const engine = (without: string[]): Discovery => ({
    phase: 'ready',
    capabilities: null,
    unavailable: new Set(),
    document: { methods: methods.filter((m) => !without.includes(m)).map((name) => ({ name, params: [], result: { schema: {} } })), 'x-theuth': { events: [], subscriptions: [] } } as never,
  })
  expect(shownTabs(sharing, engine([])).map((tab) => tab.id)).toEqual(['shared', 'contacts', 'shared-chats', 'shared-notes', 'sharing', 'domains'])
  const old = engine(['sharing_list', 'sharing_revoke'])
  expect(shownTabs(sharing, old).map((tab) => tab.id)).toEqual(['contacts', 'shared-chats', 'sharing', 'domains'])
  expect(shownTab('shared', old)).toBe('contacts')
})

test('S4: a category with no shown tab leaves the column, and its tabs land on Appearance', () => {
  hide('signal', 'slack', 'telegram')
  expect(shownCategories().map((category) => category.id)).not.toContain('channels')
  expect(shownCategories()).toHaveLength(CATEGORIES.length - 1)
  expect(shownTab('slack')).toBe('appearance')
})
