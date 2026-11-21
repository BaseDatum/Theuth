import { afterEach, expect, test, vi } from 'vitest'
// The registry imports every tab component; some stores read `localStorage` at import; no DOM in the runner.
vi.hoisted(() => {
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })
})
import { CATEGORIES, MOVED_TABS, TAB_IDS, isTab, shownCategories, shownTab, shownTabs } from './registry'

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

test('188: eleven categories in column order (#3 Prompt suffixes after Agent), each with 1 to 6 tabs', () => {
  expect(CATEGORIES.map((category) => category.id)).toEqual(['general', 'models', 'agent', 'suffixes', 'learning', 'voice', 'meetings', 'calendar', 'network', 'channels', 'tools'])
  for (const category of CATEGORIES) {
    expect(category.tabs.length).toBeGreaterThanOrEqual(1)
    expect(category.tabs.length).toBeLessThanOrEqual(6)
  }
})

test('every old id redirects to a tab and is no tab itself', () => {
  for (const [old, to] of Object.entries(MOVED_TABS)) {
    expect(isTab(to), `${old} → ${to}`).toBe(true)
    expect(isTab(old), old).toBe(false)
  }
})

test('Sky 2026-11-11: Tools opens on MCP (the servers theuth uses, at `tools`); Serve over MCP keeps `mcp`', () => {
  const tools = CATEGORIES.find((category) => category.id === 'tools')
  expect(tools?.tabs.map((tab) => [tab.id, tab.label])).toEqual([
    ['tools', 'MCP'],
    ['search', 'Web search'],
    ['mcp', 'Serve over MCP'],
    ['computer', 'Computer use'],
  ])
})

test('S4: with every tab shown, every category is listed and every tab lands on itself', () => {
  hide()
  expect(shownCategories()).toEqual(CATEGORIES)
  for (const id of ids) expect(shownTab(id)).toBe(id)
})

test('S4: a hidden tab leaves its band and lands on its category’s first shown tab', () => {
  hide('machine', 'vms')
  const network = CATEGORIES.find((category) => category.id === 'network')
  expect(network && shownTabs(network).map((tab) => tab.id)).toEqual(['machines', 'devices', 'contacts', 'messaging'])
  expect(shownTab('machine')).toBe('machines')
  expect(shownTab('vms')).toBe('machines')
  expect(shownTab('contacts')).toBe('contacts')
})

test('S4: a category with no shown tab leaves the column, and its tabs land on Appearance', () => {
  hide('signal', 'slack', 'telegram')
  expect(shownCategories().map((category) => category.id)).not.toContain('channels')
  expect(shownCategories()).toHaveLength(CATEGORIES.length - 1)
  expect(shownTab('slack')).toBe('appearance')
})
