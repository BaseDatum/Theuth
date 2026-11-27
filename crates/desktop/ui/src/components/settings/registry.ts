import { agent } from '@/components/settings/categories/agent'
import { calendar } from '@/components/settings/categories/calendar'
import { channels } from '@/components/settings/categories/channels'
import { general } from '@/components/settings/categories/general'
import { learning } from '@/components/settings/categories/learning'
import { meetings } from '@/components/settings/categories/meetings'
import { models } from '@/components/settings/categories/models'
import { network } from '@/components/settings/categories/network'
import { plugins } from '@/components/settings/categories/plugins'
import { prompt } from '@/components/settings/categories/prompt'
import { security } from '@/components/settings/categories/security'
import { sharing } from '@/components/settings/categories/sharing'
import { tools } from '@/components/settings/categories/tools'
import { voice } from '@/components/settings/categories/voice'
import type { SettingsCategory, SettingsTab } from '@/components/settings/types'
import { supports, type Discovery } from '@/rpc/discovery'

/** 188: the categories column, top to bottom. 210: Sharing after Network (public web sharing, §7.4 item 4); 213: its hub. 225: Security (the vault) after Sharing. 216: Plugins last. */
export const CATEGORIES: readonly SettingsCategory[] = [general, models, agent, prompt, learning, voice, meetings, calendar, network, sharing, security, channels, tools, plugins]

/** A tab id and the category it belongs to. */
export type Placed = { category: SettingsCategory; tab: SettingsTab }

const BY_TAB: ReadonlyMap<string, Placed> = new Map(CATEGORIES.flatMap((category) => category.tabs.map((tab) => [tab.id, { category, tab }] as const)))

/** The tab with this URL id and its category, or `undefined`. */
export function placeOf(id: string): Placed | undefined {
  return BY_TAB.get(id)
}

export function isTab(id: string): boolean {
  return BY_TAB.has(id)
}

/** 188: every tab id (its URL), one line per category in column order: what `settings()` and links may name. `registry.test.ts` holds it to `CATEGORIES`. */
export const TAB_IDS = [
  'appearance', 'app', 'shortcuts',
  'models', 'helpers', 'providers',
  'loop', 'memory', 'approvals', 'external-secrets', 'advisor', 'autoresearch', 'harnesses',
  'prompts', 'system',
  'dreaming', 'double', 'decision',
  'voice', 'dictation', 'voice-dictionary', 'voice-key', 'voice-history',
  'meetings', 'detection', 'live', 'speakers', 'templates',
  'calendar', 'briefs', 'upcoming',
  'machine', 'machines', 'devices', 'messaging', 'vms',
  'shared', 'contacts', 'shared-chats', 'shared-notes', 'sharing', 'domains',
  'vault',
  'signal', 'slack', 'telegram',
  'categories', 'tools', 'builtins', 'search', 'mcp', 'computer', 'browser',
  'plugins',
] as const

export type SettingsTabId = (typeof TAB_IDS)[number]

/** 188 S4: whether the tab can work here (no `visible`: everywhere). */
export const isShown = (tab: SettingsTab): boolean => tab.visible?.() ?? true

/** The category's tabs shown here, in band order; empty when the whole category is hidden. */
export const shownTabs = (category: SettingsCategory, discovery?: Discovery): SettingsTab[] => category.tabs.filter(tab => isShown(tab) && (!discovery || !tab.required || supports(discovery, tab.required)))

/** The categories with at least one shown tab, in column order. */
export const shownCategories = (discovery?: Discovery): SettingsCategory[] => CATEGORIES.filter(category => shownTabs(category, discovery).length > 0)

/** Per-machine Settings: a `device` tab is this client's, so the nav lists it under "This <device>", not its category. */
export const isDeviceTab = (tab: SettingsTab): boolean => tab.scope === 'device'

/** The category's shown tabs the nav lists under it: the device tabs sit in their own group. */
export const navTabs = (category: SettingsCategory, discovery?: Discovery): SettingsTab[] => shownTabs(category, discovery).filter(tab => !isDeviceTab(tab))

/** The categories column without the device tabs, in column order; a category left with none leaves the column. */
export const navCategories = (discovery?: Discovery): SettingsCategory[] => CATEGORIES.filter(category => navTabs(category, discovery).length > 0)

/** The "This <device>" group: every shown device tab, in column order. */
export const deviceTabs = (discovery?: Discovery): SettingsTab[] => CATEGORIES.flatMap(category => shownTabs(category, discovery).filter(isDeviceTab))

/** The tabs of the band over `tab`: the device group for a device tab, else its category's nav tabs. */
export const bandTabs = ({ category, tab }: Placed, discovery?: Discovery): SettingsTab[] => (isDeviceTab(tab) ? deviceTabs(discovery) : navTabs(category, discovery))

/** Where `/settings/<id>` of a known tab lands: the tab itself when shown, else its category's first shown tab, else Appearance. */
export function shownTab(id: string, discovery?: Discovery): string {
  const placed = placeOf(id)
  if (!placed || (isShown(placed.tab) && (!discovery || !placed.tab.required || supports(discovery, placed.tab.required)))) return id
  return shownTabs(placed.category, discovery)[0]?.id ?? 'appearance'
}

/**
 * 188: ids of the old 24-tab modal that are no longer tabs, and the tab that holds most of what each one had.
 * `mesh` was already an alias of `network` (183 §12.6). Every other old id is still a tab id.
 */
export const MOVED_TABS: Readonly<Record<string, string>> = {
  general: 'appearance',
  chat: 'models',
  connections: 'providers',
  agents: 'harnesses',
  labs: 'decision',
  network: 'machine',
  mesh: 'machine',
  link: 'devices',
  channels: 'signal',
  communication: 'messaging',
  suffixes: 'prompts',
}
