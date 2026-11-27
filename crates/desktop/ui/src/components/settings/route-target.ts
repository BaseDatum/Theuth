import { type LibraryRoute, movedTab } from '@/components/library/moved'
import { movedSettingsTab } from '@/components/routines/helpers'
import { MOVED_TABS, isTab, shownTab } from '@/components/settings/registry'

/** `/settings/$tab`'s search: `focus` names a control the tab focuses (Memory: `model` | `spend`); `progress` names a cloud node whose status dialog Machines opens; `session` scopes MCP's (`tools`) per-chat rows, and `peer` comes only on legacy links to the per-chat rows. */
export type SettingsSearch = { session?: string; peer?: string; focus?: string; progress?: string }

/** What `/settings/<tab>` does: render the tab, or redirect (replacing the entry) to another page, a chat or a tab. */
export type RouteTarget =
  | { kind: 'render' }
  | { kind: 'page'; to: LibraryRoute | '/routines'; search: { session?: string } }
  | { kind: 'chat'; session: string; search: { messaging: '1'; peer?: string } }
  | { kind: 'tab'; tab: string; search: { focus?: string } }

const toTab = (tab: string, focus: string | undefined): RouteTarget => ({ kind: 'tab', tab, search: focus ? { focus } : {} })

/** 188 S5: where `/settings/<tab>` lands. `beforeLoad` acts on it; every redirect replaces, so Back skips the old URL. */
export function routeTarget(tab: string, search: SettingsSearch): RouteTarget {
  // Skills and Secrets became Library pages, and Schedules became Routines; old links and bookmarks follow.
  const moved = movedTab(tab) ?? movedSettingsTab(tab)
  if (moved) return { kind: 'page', to: moved, search: search.session ? { session: search.session } : {} }
  // The per-chat rows left Settings: an old link that named a chat opens that chat's Messaging… sheet.
  if (tab === 'communication' && search.session) return { kind: 'chat', session: search.session, search: { messaging: '1', ...(search.peer && { peer: search.peer }) } }
  // Tabs of the old modal that were renamed or split (and 183's `mesh`); `hasOwn`, so `constructor` is no old tab.
  const renamed = Object.hasOwn(MOVED_TABS, tab) ? MOVED_TABS[tab] : undefined
  if (renamed) return toTab(shownTab(renamed), search.focus)
  if (!isTab(tab)) return toTab('appearance', search.focus)
  // S4: a tab that cannot work here (no local node) gives way to the next one that can.
  const shown = shownTab(tab)
  return shown === tab ? { kind: 'render' } : toTab(shown, search.focus)
}
