import { agent } from '@/components/settings/categories/agent'
import { calendar } from '@/components/settings/categories/calendar'
import { channels } from '@/components/settings/categories/channels'
import { general } from '@/components/settings/categories/general'
import { learning } from '@/components/settings/categories/learning'
import { meetings } from '@/components/settings/categories/meetings'
import { models } from '@/components/settings/categories/models'
import { network } from '@/components/settings/categories/network'
import { tools } from '@/components/settings/categories/tools'
import { voice } from '@/components/settings/categories/voice'
import type { SettingsCategory, SettingsTab } from '@/components/settings/types'

/** 188: the categories column, top to bottom. */
export const CATEGORIES: readonly SettingsCategory[] = [general, models, agent, learning, voice, meetings, calendar, network, channels, tools]

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
  'loop', 'memory', 'approvals', 'advisor', 'autoresearch', 'harnesses',
  'dreaming', 'double', 'decision',
  'voice', 'dictation', 'voice-key', 'voice-history',
  'meetings', 'detection', 'live', 'speakers', 'templates',
  'calendar', 'briefs', 'upcoming',
  'machine', 'machines', 'devices', 'contacts', 'messaging', 'vms',
  'signal', 'slack', 'telegram',
  'search', 'mcp', 'computer',
] as const

export type SettingsTabId = (typeof TAB_IDS)[number]

/** 188 S4: whether the tab can work here (no `visible`: everywhere). */
export const isShown = (tab: SettingsTab): boolean => tab.visible?.() ?? true

/** The category's tabs shown here, in band order; empty when the whole category is hidden. */
export const shownTabs = (category: SettingsCategory): SettingsTab[] => category.tabs.filter(isShown)

/** The categories with at least one shown tab, in column order. */
export const shownCategories = (): SettingsCategory[] => CATEGORIES.filter((category) => category.tabs.some(isShown))

/** Where `/settings/<id>` of a known tab lands: the tab itself when shown, else its category's first shown tab, else Appearance. */
export function shownTab(id: string): string {
  const placed = placeOf(id)
  if (!placed || isShown(placed.tab)) return id
  return shownTabs(placed.category)[0]?.id ?? 'appearance'
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
}
