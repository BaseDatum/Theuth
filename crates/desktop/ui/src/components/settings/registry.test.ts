import { afterEach, expect, test, vi } from 'vitest'
// The registry imports every tab component; some stores read `localStorage` at import; no DOM in the runner.
vi.hoisted(() => {
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })
})
import { CATEGORIES, MOVED_TABS, TAB_IDS, bandTabs, deviceTabs, isTab, navCategories, navTabs, placeOf, shownCategories, shownTab, shownTabs } from './registry'
import { type EngineCaps, engineCaps, notApplicable } from './scope'
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

test('188: fourteen categories in column order (#3 Prompt after Agent, 210 Sharing after Network, 225 Security after Sharing, 216 Plugins last), each with 1 to 7 tabs', () => {
  expect(CATEGORIES.map((category) => category.id)).toEqual(['general', 'models', 'agent', 'prompt', 'learning', 'voice', 'meetings', 'calendar', 'network', 'sharing', 'security', 'channels', 'tools', 'plugins'])
  for (const category of CATEGORIES) {
    expect(category.tabs.length).toBeGreaterThanOrEqual(1)
    expect(category.tabs.length).toBeLessThanOrEqual(7)
  }
})

test('225: Security holds Vault, at `vault`, needing the twelve vault methods', () => {
  const security = CATEGORIES.find((category) => category.id === 'security')!
  expect(security.tabs.map((tab) => [tab.id, tab.label])).toEqual([['vault', 'Vault']])
  expect(security.tabs[0].required?.methods).toHaveLength(12)
  expect(security.tabs[0].required?.methods).toContain('vault_unseal_requests')
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

test('Prompt holds Prompts and System, and System declares the prompt section methods', () => {
  const prompt = CATEGORIES.find((category) => category.id === 'prompt')!
  expect(prompt.tabs.map((tab) => [tab.id, tab.label])).toEqual([
    ['prompts', 'Prompts'],
    ['system', 'System'],
  ])
  const system = prompt.tabs.find((tab) => tab.id === 'system')!
  expect(system.required?.methods).toEqual([
    'list_prompt_sections',
    'set_prompt_section',
    'set_prompt_section_enabled',
    'reset_prompt_section',
  ])
})

test('216: Plugins is one tab, at `plugins`, needing the plugins methods and the `plugins.ui` topic', () => {
  const plugins = CATEGORIES.find((category) => category.id === 'plugins')!
  expect(plugins.tabs.map((tab) => [tab.id, tab.label])).toEqual([['plugins', 'Plugins']])
  expect(plugins.tabs[0].required?.methods).toContain('plugin_approve')
  expect(plugins.tabs[0].required?.topics).toEqual(['plugins.ui@1'])
})

test('221 §8: Settings › Agent › Secrets sits after Approvals at `external-secrets` (`secrets` still redirects to the page) and needs the harness-wide setting methods', () => {
  const agent = CATEGORIES.find((category) => category.id === 'agent')!
  expect(agent.tabs.map((tab) => [tab.id, tab.label])).toEqual([
    ['loop', 'Loop'],
    ['memory', 'Context'],
    ['approvals', 'Approvals'],
    ['external-secrets', 'Secrets'],
    ['advisor', 'Advisor'],
    ['autoresearch', 'Autoresearch'],
    ['harnesses', 'Harnesses'],
  ])
  expect(isTab('secrets')).toBe(false)
  expect(agent.tabs.find((tab) => tab.id === 'external-secrets')?.required?.methods).toEqual(['get_external_settings', 'set_external_setting'])
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

// Per-machine Settings piece 1: the picker and "Set on all machines" read these; a new tab must say where it lives.
test('ratchet: every tab declares a scope, copyable, and only a machine tab copies; a machine tab that does not says why', () => {
  for (const tab of tabs) {
    expect(['machine', 'everywhere', 'hub', 'device'], tab.id).toContain(tab.scope)
    expect(typeof tab.copyable, tab.id).toBe('boolean')
    if (tab.copyable) expect(tab.scope, tab.id).toBe('machine')
    if (tab.scope === 'machine' && !tab.copyable) expect(tab.copyNote, tab.id).toBeTruthy()
    for (const need of tab.needs ?? []) expect(['display', 'user', 'mic'], tab.id).toContain(need)
  }
})

test('the scope of every tab, as the settings recon placed it', () => {
  const by = (scope: string, copyable?: boolean) => tabs.filter((t) => t.scope === scope && (copyable === undefined || t.copyable === copyable)).map((t) => t.id)
  expect(by('device')).toEqual(['appearance', 'shortcuts'])
  expect(by('everywhere')).toEqual(['prompts', 'machines', 'devices', 'shared', 'contacts', 'shared-chats', 'shared-notes', 'sharing', 'domains', 'tools', 'browser'])
  expect(by('hub')).toEqual(['dreaming', 'double', 'signal', 'slack', 'telegram'])
  expect(by('machine', true)).toEqual(['models', 'helpers', 'providers', 'loop', 'memory', 'approvals', 'advisor', 'autoresearch', 'system', 'voice-dictionary', 'templates', 'briefs', 'categories', 'builtins', 'search', 'plugins'])
  expect(by('machine', false)).toEqual(['app', 'external-secrets', 'harnesses', 'decision', 'voice', 'dictation', 'voice-key', 'voice-history', 'meetings', 'detection', 'live', 'speakers', 'calendar', 'upcoming', 'machine', 'messaging', 'vms', 'vault', 'mcp', 'computer'])
})

test('notApplicable: a headless engine dims the mic, display and user tabs only; the desktop daemon dims none', () => {
  const na = (caps: EngineCaps) => tabs.filter((t) => notApplicable(t, caps)).map((t) => t.id)
  expect(na({ display: false, user: false, mic: false })).toEqual(['voice', 'dictation', 'meetings', 'detection', 'live', 'computer'])
  expect(notApplicable(placeOf('voice')!.tab, { display: false, user: false, mic: false })).toEqual(['mic', 'user'])
  // A remote engine whose mic list came back empty: only the mic is known missing.
  expect(na(engineCaps({ local: false, inputDevices: [] }))).toEqual(['voice'])
  expect(na(engineCaps({ local: false }))).toEqual([])
  expect(na(engineCaps({ local: true, inputDevices: [] }))).toEqual([])
  // 227: the picked machine's `engine_caps` wins, even over `local`: a headless node dims the same tabs.
  const headless = { display: false, mic: false, interactive_user: false }
  expect(na(engineCaps({ local: true, host: headless }))).toEqual(['voice', 'dictation', 'meetings', 'detection', 'live', 'computer'])
  expect(na(engineCaps({ local: false, host: { display: true, mic: true, interactive_user: true } }))).toEqual([])
})

test('the nav: device tabs leave General for the "This <device>" group, whose band is the group; nothing else moves', () => {
  hide()
  expect(deviceTabs().map((t) => t.id)).toEqual(['appearance', 'shortcuts'])
  expect(navTabs(CATEGORIES[0]!).map((t) => t.id)).toEqual(['app'])
  expect(navCategories()).toEqual(CATEGORIES)
  expect(bandTabs(placeOf('shortcuts')!).map((t) => t.id)).toEqual(['appearance', 'shortcuts'])
  expect(bandTabs(placeOf('app')!).map((t) => t.id)).toEqual(['app'])
  expect(navTabs(CATEGORIES[1]!).map((t) => t.id)).toEqual(['models', 'helpers', 'providers'])
  hide('app')
  expect(navCategories().map((c) => c.id)).not.toContain('general')
  expect(deviceTabs().map((t) => t.id)).toEqual(['appearance', 'shortcuts'])
})
