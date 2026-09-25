/** Settings tabs that became Library pages in the sidebar; `/settings/<tab>` redirects here. */
export const MOVED_TABS = { skills: '/skills', tools: '/tools', secrets: '/secrets' } as const

export type LibraryRoute = (typeof MOVED_TABS)[keyof typeof MOVED_TABS]

export function movedTab(tab: string): LibraryRoute | null {
  return Object.hasOwn(MOVED_TABS, tab) ? MOVED_TABS[tab as keyof typeof MOVED_TABS] : null
}
