/** Derive a clean human title for an app route path (e.g. `/tasks` -> `Tasks`, `/attention` -> `Inbox`). */
const ROUTE_TITLES: readonly [prefix: string, title: string][] = [
  ['/meetings/new', 'Meeting'],
  ['/tasks', 'Tasks'],
  ['/notebooks', 'Notebooks'],
  ['/notes', 'Notes'],
  ['/agents', 'Agents'],
  ['/routines', 'Routines'],
  ['/meetings', 'Meetings'],
  ['/knowledge', 'Knowledge'],
  ['/skills', 'Skills'],
  ['/artifacts', 'Artifacts'],
  ['/whiteboards', 'Whiteboards'],
  ['/autoresearch', 'Autoresearch'],
  ['/attention', 'Inbox'],
  ['/costs', 'Costs'],
  ['/settings', 'Settings'],
  ['/secrets', 'Secrets'],
  ['/library', 'Library'],
  ['/tools', 'Tools'],
  ['/sources', 'Source'],
]

/** Normalise a view path to start with `/` and trim whitespace. */
export function normalizeViewPath(raw: string): string {
  const trimmed = raw.trim()
  if (!trimmed) return '/'
  return trimmed.startsWith('/') ? trimmed : `/${trimmed}`
}

/** Human-readable title for a `view` tile from explicit title or route path. */
export function viewTitle(path: string, explicit?: string): string {
  const custom = explicit?.trim()
  if (custom) return custom
  const pathname = normalizeViewPath(path).split(/[?#]/)[0] || '/'
  for (const [prefix, title] of ROUTE_TITLES) {
    if (pathname === prefix || pathname.startsWith(`${prefix}/`)) return title
  }
  const seg = pathname.split('/').filter(Boolean)[0]
  if (!seg) return 'View'
  return seg.charAt(0).toUpperCase() + seg.slice(1)
}

export { availableViews, validateViewPath, type ViewCatalogEntry } from './view-catalog'
