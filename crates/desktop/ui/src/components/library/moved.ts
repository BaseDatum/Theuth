/** Settings tabs that became sidebar pages — Skills and Secrets under Library; `/settings/<tab>` redirects here. */
export const MOVED_TABS = { skills: '/skills', secrets: '/secrets' } as const

export type LibraryRoute = (typeof MOVED_TABS)[keyof typeof MOVED_TABS]

export function movedTab(tab: string): LibraryRoute | null {
  return Object.hasOwn(MOVED_TABS, tab) ? MOVED_TABS[tab as keyof typeof MOVED_TABS] : null
}
