// Test-only: a headless Trellis workspace in place of the one `Panes` mounts, attached to the facade (`lib/panes`).
import '../components/settings/test-dom'
import { createWorkspace, layout as L, type WorkspaceHandle } from '@danfessler/trellis'
import { attachWorkspace, CHAT, chatSpec, DRAFT_TYPE, isTile, resetPanes, STORAGE_KEY, tileLeaf, tileOf, tileType, usePanes, VERSION, WEB } from './panes'

/** The kinds `components/tiles/registry` draws (not imported here: it pulls every kind's body). */
export const TILE_KIND_NAMES = ['artifact', 'browser', 'codemap', 'diagram', 'diff', 'file', 'jobs', 'job-log', 'job-terminal', 'media', 'memory', 'note', 'prompt', 'skill', 'terminals', 'vm']
export const TYPES = [CHAT, DRAFT_TYPE, WEB, ...TILE_KIND_NAMES.map(tileType)]

/** Tile `id`'s props while it is laid out. */
export const tileProps = (id: string): unknown => usePanes.getState().views[tileLeaf(id)]?.params?.props
/** The ids of the laid-out tiles. */
export const tileIds = (): string[] => usePanes.getState().leaves.filter((l) => isTile(l)).map(tileOf)

/** How often each view's content was mounted (Trellis mounts a view once; a move, dock or tab switch never again). */
export const mounts = new Map<string, number>()

let current: { ws: WorkspaceHandle; host: HTMLElement; detach(): void } | null = null

/**
 * A workspace showing the chats `leaves` side by side (none: empty), persisting like the app's, with `leaves[0]`
 * focused; `navigate` gets the URL moves. Replaces the previous one. `persisted`: start from the stored document.
 */
export function tiling(leaves: string[] = [], o: { navigate?(session: string): void; persisted?: boolean } = {}): WorkspaceHandle {
  unmount()
  // jsdom's `addEventListener` takes only its own `AbortSignal` (Trellis listens with one).
  Object.assign(globalThis, { AbortController: window.AbortController, AbortSignal: window.AbortSignal })
  if (!o.persisted) localStorage.removeItem(STORAGE_KEY)
  const host = document.body.appendChild(document.createElement('div'))
  const ws = createWorkspace(host, {
    types: Object.fromEntries(
      TYPES.map((t) => [
        t,
        {
          mount: (_el: HTMLElement, view: { id: string }) => {
            mounts.set(view.id, (mounts.get(view.id) ?? 0) + 1)
            return () => undefined
          },
        },
      ]),
    ),
    floating: false,
    navigation: 'focus',
    persist: { key: STORAGE_KEY, version: VERSION },
    onMissingType: () => 'drop',
    defaultLayout: leaves.length ? L.row(leaves.map((l) => L.view(chatSpec(l).type, { id: l, params: chatSpec(l).params }))) : null,
  })
  usePanes.setState({ focused: leaves[0] ?? usePanes.getState().focused, selected: null, drag: null })
  current = { ws, host, detach: attachWorkspace(ws, o.navigate) }
  return ws
}

/** `Panes` unmounting (another page, a phone): the document is flushed to storage, the mirror kept. */
export function unmount(): void {
  if (!current) return
  current.detach()
  current.ws.destroy()
  current.host.remove()
  current = null
}

/** Unmount the workspace (its document flushed to storage, as `Panes` unmounting does) and forget the panes. */
export function untile(): void {
  unmount()
  resetPanes()
  usePanes.setState({ leaves: [], views: {}, first: null })
}
