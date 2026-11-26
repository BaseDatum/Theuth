// The `view` card (a live app route inside a card): which paths it takes, its title, and where inner navigation leaves
// the card. The engine checks the same paths on `wb_place`/`wb_update` (`document::wb::engine::view_path`).

import { availableViews, validateViewPath } from '@/lib/view-catalog'
import { viewTitle } from '@/lib/view-title'

/** `/whiteboards/<id>` is refused: a whiteboard inside one of its own cards would hold itself. */
const NESTED_BOARD = /^\/whiteboards\/[^/?#]+/

/** The path a `view` card shows, normalized, or null when it is not one of the app's view routes. */
export function viewCardPath(raw: unknown): string | null {
  if (typeof raw !== 'string' || raw.length > 512) return null
  const v = validateViewPath(raw)
  return v.ok && !NESTED_BOARD.test(v.normalized) ? v.normalized : null
}

/** The task graph of `project` (`null`: every project). */
export const taskGraphPath = (project: string | null): string => `/tasks/graph?project=${encodeURIComponent(project ?? '')}`

/** The route's title for the card header: the catalog's name, the task graph with its project. */
export function viewCardTitle(path: string): string {
  const q = path.search(/[?#]/)
  const pathname = q >= 0 ? path.slice(0, q) : path
  if (pathname === '/tasks/graph') {
    const project = q >= 0 ? new URLSearchParams(path.slice(q).replace(/#.*$/, '')).get('project') : null
    return project ? `Task graph · ${project}` : 'Task graph'
  }
  return availableViews().find((v) => v.path === pathname)?.title ?? viewTitle(path)
}

/**
 * Where a navigation inside the card goes instead of the card: a chat (`/c/<id>`) or a new chat to the workspace, a
 * whiteboard to a pane (never nested in the card). `null`: it stays in the card.
 */
export function leavesCard(href: string): { kind: 'chat'; session: string; href: string } | { kind: 'outer'; href: string } | { kind: 'pane'; href: string } | null {
  const chat = /^\/c\/([^/?#]+)/.exec(href)
  if (chat?.[1]) return { kind: 'chat', session: decodeURIComponent(chat[1]), href }
  if (href === '/' || /^\/new(?:$|[?#/])/.test(href)) return { kind: 'outer', href }
  if (NESTED_BOARD.test(href)) return { kind: 'pane', href }
  return null
}
