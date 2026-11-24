// Tiled panes on Trellis (`@danfessler/trellis`): the `Workspace` that `Panes` mounts owns the layout — splits,
// dividers, drag and drop, tabs, persistence — and this module is the facade the rest of the app calls. Every pane is a
// Trellis view whose id is its leaf: a session id, a draft (`draft:<uuid>[?project=<label>]`, 25 §12.3: a new chat whose
// session does not exist until its first send), a web frame (`web:<id>`, 205 D5, `lib/web-views.ts`) or a generic
// tile (`tile:<id>`, `lib/tiles.ts`). Web frames and tiles are frames: never focused, never in the URL.
// The store mirrors the workspace's document (`leaves`, `views`); it keeps the last document while no workspace is
// mounted (a phone, another page), and removals then edit the persisted copy.

import { useEffect } from 'react'
import { create } from 'zustand'
import { type LayoutDocument, type LayoutNode, type PanelNode, type Placement, sanitize, type ViewInfo, type ViewRecord, type WorkspaceHandle } from '@danfessler/trellis'
import { useEngines } from '@/link/engines'
import { openPopout } from '@/lib/popout'
import { isWeb, type WebView, useWebViews, webId, webLeaf } from '@/lib/web-views'

export type Edge = 'left' | 'right' | 'top' | 'bottom' | 'center'
/** The pane under the pointer and which of its edges (or its middle) the drop snaps to. */
export type Target = { session: string; edge: Edge; rect: DOMRect }
/** A sidebar row (a session, a draft for New chat) dragged onto the panes. */
export type Drag = { session: string; title: string; x: number; y: number; target: Target | null }

const DRAFT = 'draft:'
/** A pane id for a not-yet-created chat; `leaves` lists it beside real sessions, so check before an RPC. */
export const isDraft = (id: string | null | undefined): id is string => id?.startsWith(DRAFT) ?? false
/** A fresh draft id; `project` (the project header's "New chat in <project>") rides along for `draftProject`. */
export const newDraft = (project?: string | null): string =>
  `${DRAFT}${crypto.randomUUID()}${project ? `?project=${encodeURIComponent(project)}` : ''}`
/** The picker preset a draft id carries, if any. */
export function draftProject(id: string): string | undefined {
  const q = id.indexOf('?')
  return q < 0 ? undefined : (new URLSearchParams(id.slice(q)).get('project') ?? undefined)
}

const TILE = 'tile:'
/** A pane leaf holding a generic tile (`lib/tiles.ts`), not a session; check before an RPC. */
export const isTile = (leaf: string | null | undefined): leaf is string => leaf?.startsWith(TILE) ?? false
export const tileLeaf = (id: string): string => `${TILE}${id}`
export const tileOf = (leaf: string): string => leaf.slice(TILE.length)
/** A leaf that is not a chat: a web frame or a generic tile. Never focused, never in the URL. */
export const isFrame = (leaf: string | null | undefined): leaf is string => isWeb(leaf) || isTile(leaf)

/** `leaves` without web frames and tiles: the chat panes (drafts included), for whatever expects a session id. */
export const chatLeaves = (leaves: readonly string[]): string[] => leaves.filter((l) => !isFrame(l))

/** The one leaf a phone shows (it does not tile, 25 §12.3): the focused pane while it is a chat of the layout, else its
 * first chat leaf, else the route's `session`. */
export const shown = (leaves: readonly string[], focused: string | null, session: string): string => {
  const all = chatLeaves(leaves)
  return focused !== null && all.includes(focused) ? focused : (all[0] ?? session)
}

/** Trellis view types: one per leaf kind; a tile's is `tile:<kind>`. */
export const CHAT = 'chat'
export const DRAFT_TYPE = 'draft'
export const WEB = 'web'
export const tileType = (kind: string): string => `${TILE}${kind}`
/** What `open()` needs for a leaf that is not open yet. Every view records the engine it belongs to (`prepareStored`). */
export type ViewSpec = { type: string; params: Record<string, unknown> }
const engine = (): string => useEngines.getState().active
export const chatSpec = (leaf: string): ViewSpec => ({ type: isDraft(leaf) ? DRAFT_TYPE : CHAT, params: { engine: engine() } })
const webSpec = (leaf: string): ViewSpec => ({ type: WEB, params: { engine: engine(), session: useWebViews.getState().views[webId(leaf)]?.session ?? '' } })
export const tileSpec = (kind: string, session: string, props: unknown): ViewSpec => ({ type: tileType(kind), params: { engine: engine(), session, props } })

/** The chat a view belongs to: a frame's `session` param, else the leaf itself. */
export function ownerOf(leaf: string, record: ViewRecord | undefined): string {
  if (!isFrame(leaf)) return leaf
  const s = record?.params?.session
  return typeof s === 'string' ? s : ''
}

/** Persisted per machine (localStorage) by Trellis itself; `VERSION` drops documents of an older shape. */
export const STORAGE_KEY = 'theuth.panes'
export const VERSION = 1

type PanesStore = {
  /** The mounted workspace, `null` while none is (a phone, another page, tests that do not tile). */
  ws: WorkspaceHandle | null
  /** Every view, in layout order (left-to-right, top-to-bottom); a view mid-drag included. */
  leaves: string[]
  /** The document's view records: type and params, by leaf. */
  views: Record<string, ViewRecord>
  /** The top-left view: the one that carries the sidebar toggle and the traffic-light inset. */
  first: string | null
  /** A Trellis drag (a pane moved by its header or tab) is in flight. */
  dragging: boolean
  /** Pane that window-level actions (hotkeys, `document.title`) address; the route keeps the URL on it. Always a chat
   * (or draft) — never a frame, so no RPC, composer or sidebar highlight ever sees a `tile:`/`web:` id. */
  focused: string | null
  /** The pane the user last picked, of any kind (a press inside it, its tab, a route landing): the one that is lit
   * while the others dim, and the one a sidebar pick or a newly opened surface takes over when it is a frame. A stale
   * value (no longer laid out) counts as none. */
  selected: string | null
  drag: Drag | null
  focus(session: string): void
  select(leaf: string): void
  /** The route landed on `session`: focus its pane, or show it in the selected frame's pane (taking it over), else in
   * the focused chat's pane, else left of the frames when only frames are left. */
  show(session: string): void
  /** Show `session` at `edge` of the pane showing `target` (`center` replaces it). A pane already showing `session` moves. */
  place(target: string, session: string, edge: Edge): void
  /** New chat where the user is looking: with ≥ 2 chat panes the focused one (fallback: the first) becomes a fresh
   * draft; a focused draft is kept, never stacked. Returns the draft to type into, or `null` for a single pane: go to `/`. */
  fresh(project?: string | null): string | null
  /** Close the pane showing `session`; returns the session the URL should move to, or `null` to stay. */
  close(session: string): string | null
  startDrag(session: string, title: string, x: number, y: number): void
  moveDrag(x: number, y: number): void
  /** Drop (or cancel with `drop = false`); returns the dropped session when a pane took it. */
  endDrag(drop: boolean): string | null
}

/** Nested calls into the workspace made by this facade: Trellis's own focus events are not user picks then. */
let internal = 0
function op<T>(fn: () => T): T {
  internal++
  try {
    return fn()
  } finally {
    internal--
  }
}

/** The route's session, remembered for a workspace that mounts after the route landed (`attachWorkspace`). */
let wanted: string | null = null
/** Moves the URL to a chat the user picked or closed into (`Panes` wires the router). */
let go: ((session: string) => void) | null = null

function panelOf(root: LayoutNode | null | undefined, leaf: string): PanelNode | null {
  if (!root) return null
  if (root.kind === 'panel') return root.views.includes(leaf) ? root : null
  if (root.kind === 'stage') return panelOf(root.child, leaf)
  for (const c of root.children) {
    const p = panelOf(c, leaf)
    if (p) return p
  }
  return null
}
function firstPanel(root: LayoutNode | null | undefined): PanelNode | null {
  if (!root) return null
  if (root.kind === 'panel') return root
  if (root.kind === 'stage') return firstPanel(root.child)
  return firstPanel(root.children[0])
}
function order(doc: LayoutDocument, extra: readonly string[] = []): string[] {
  const out: string[] = []
  const walk = (n: LayoutNode | undefined) => {
    if (!n) return
    if (n.kind === 'panel') out.push(...n.views)
    else if (n.kind === 'stage') walk(n.child)
    else n.children.forEach(walk)
  }
  walk(doc.root ?? undefined)
  for (const f of doc.floating) out.push(...f.panel.views)
  for (const h of doc.hidden) out.push(...h.panel.views)
  for (const id of extra) if (!out.includes(id)) out.push(id)
  return out
}
const same = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((x, i) => x === b[i])

/** The store follows `doc` (and the views a drag is carrying). */
function mirror(doc: LayoutDocument, carried: readonly string[] = [], dragging = false): void {
  const s = usePanes.getState()
  const leaves = order(doc, carried)
  usePanes.setState({
    leaves: same(s.leaves, leaves) ? s.leaves : leaves,
    views: doc.views,
    first: firstPanel(doc.root)?.selected ?? null,
    dragging,
  })
}

/** Open `leaf` (not laid out yet) per `spec` at `placement`, unfocused: the caret stays where the user put it. */
function openView(ws: WorkspaceHandle, leaf: string, spec: ViewSpec, placement: Placement): void {
  ws.open(spec.type, { id: leaf, params: spec.params, placement, focus: false })
}

/** `leaf` takes `old`'s place: same panel, same tab position; `old` closes. An open `leaf` moves there. */
function replace(ws: WorkspaceHandle, old: string, leaf: string, spec: ViewSpec): void {
  const doc = ws.getDocument()
  const panel = panelOf(doc.root, old)
  if (!panel || old === leaf) return
  const index = panel.views.indexOf(old)
  if (doc.views[leaf]) ws.dock(leaf, { into: panel.id, index })
  else openView(ws, leaf, spec, { into: panel.id, index })
  void ws.close(old, { force: true })
  const { focused, selected } = usePanes.getState()
  usePanes.setState({ focused: focused === old ? null : focused, selected: selected === old ? leaf : selected })
}

/** `leaf` at `edge` of `target`'s panel (`center` takes it over); beside the whole layout when `target` is not laid out. */
function put(ws: WorkspaceHandle, target: string, leaf: string, edge: Edge, spec: ViewSpec): void {
  const doc = ws.getDocument()
  const panel = panelOf(doc.root, target)
  const open = doc.views[leaf] !== undefined
  if (!panel) {
    if (open) return
    if (doc.root) openView(ws, leaf, spec, { beside: doc.root.id, edge: edge === 'center' ? 'right' : edge })
    else openView(ws, leaf, spec, 'side')
  } else if (edge === 'center') replace(ws, target, leaf, spec)
  else if (open) ws.dock(leaf, { beside: panel.id, edge })
  else openView(ws, leaf, spec, { beside: panel.id, edge })
}

/** The focused pane `id` closed: the URL moves to the first real chat left (the return value), else focus stays on
 * the first draft left. */
function refocus(id: string): string | null {
  const { focused, leaves } = usePanes.getState()
  if (focused !== id) return null
  const to = leaves.find((s) => !isDraft(s) && !isFrame(s)) ?? null
  usePanes.setState({ focused: to ?? chatLeaves(leaves)[0] ?? null })
  return to
}

/** Without a workspace: `ids` leave the persisted document (and the mirror). */
function forget(ids: readonly string[]): void {
  const s = usePanes.getState()
  if (!ids.some((id) => s.views[id] || s.leaves.includes(id))) return
  const views = { ...s.views }
  for (const id of ids) delete views[id]
  usePanes.setState({ leaves: s.leaves.filter((l) => !ids.includes(l)), views })
  editStored((doc) => {
    for (const id of ids) delete doc.views[id]
    return doc
  })
}

/** Close `ids` for good: through the workspace when one is mounted, else in the persisted document. */
export function closeLeaves(ids: readonly string[]): void {
  const { ws } = usePanes.getState()
  if (!ws) return forget(ids)
  op(() => {
    for (const id of ids) void ws.close(id, { force: true })
  })
}

/** Relabel leaves in place (`from → to`): a draft becoming its session, its tiles retargeted. Each `to` takes its
 * `from`'s place; a `to` already laid out moves there. Focus and selection follow. */
export function rename(map: Record<string, { leaf: string; spec: ViewSpec }>): void {
  const { ws, focused, selected } = usePanes.getState()
  if (!ws) return
  op(() => {
    for (const [from, { leaf, spec }] of Object.entries(map)) replace(ws, from, leaf, spec)
  })
  const after = (id: string | null) => (id && map[id] ? map[id].leaf : id)
  usePanes.setState({ focused: after(focused), selected: after(selected) })
}

export const usePanes = create<PanesStore>()((set, get) => ({
  ws: null,
  leaves: [],
  views: {},
  first: null,
  dragging: false,
  focused: null,
  selected: null,
  drag: null,
  focus(session) {
    if (get().focused !== session && !isFrame(session)) set({ focused: session })
  },
  select(leaf) {
    if (get().selected !== leaf) set({ selected: leaf })
  },
  show(session) {
    wanted = session
    const { ws, leaves, focused, selected } = get()
    if (ws && !leaves.includes(session)) {
      const chats = chatLeaves(leaves)
      const target = focused && chats.includes(focused) ? focused : chats[0]
      op(() => {
        // The picked pane is a tile: the chat takes it over, as it would a picked chat pane.
        if (selected && isFrame(selected) && leaves.includes(selected)) replace(ws, selected, session, chatSpec(session))
        else if (target) replace(ws, target, session, chatSpec(session))
        // Only frames left: the chat goes left of all of them, not into one corner.
        else put(ws, '', session, 'left', chatSpec(session))
      })
    }
    set({ focused: session, selected: session })
  },
  place(target, session, edge) {
    placeView(target, session, edge, isWeb(session) ? webSpec(session) : chatSpec(session))
  },
  fresh(project) {
    const { ws, leaves, focused } = get()
    const all = chatLeaves(leaves)
    const target = focused && all.includes(focused) ? focused : all[0]
    if (target && isDraft(target)) return target
    if (!ws || all.length < 2 || !target) return null
    const draft = newDraft(project)
    op(() => replace(ws, target, draft, chatSpec(draft)))
    set({ focused: draft })
    return draft
  },
  close(session) {
    closeLeaves([session])
    return refocus(session)
  },
  startDrag(session, title, x, y) {
    document.body.classList.add('select-none')
    set({ drag: { session, title, x, y, target: null } })
  },
  moveDrag(x, y) {
    const { drag } = get()
    if (drag) set({ drag: { ...drag, x, y, target: targetAt(x, y) } })
  },
  endDrag(drop) {
    document.body.classList.remove('select-none')
    const { drag } = get()
    set({ drag: null })
    if (!drop || !drag?.target) return null
    get().place(drag.target.session, drag.session, drag.target.edge)
    return drag.session
  },
}))

/**
 * `leaf` (opened per `spec` when new) at `edge` of `target` (`center` replaces it; an open `leaf` moves). A frame never
 * takes focus (205 D5): the URL stays on the chat beside it, and it is selected only when it took over the selected
 * pane; a placed chat is focused and selected. `false` without a workspace.
 */
export function placeView(target: string, leaf: string, edge: Edge, spec: ViewSpec): boolean {
  const { ws, selected } = usePanes.getState()
  if (!ws) return false
  if (target === leaf) {
    if (!isFrame(leaf)) usePanes.setState({ focused: leaf })
    return true
  }
  op(() => put(ws, target, leaf, edge, spec))
  const over = edge === 'center' && selected === target
  // `replace` already dropped focus off a taken-over chat; a chat split beside keeps it.
  const kept = usePanes.getState().focused ?? (isFrame(target) || edge === 'center' ? null : target)
  usePanes.setState({ focused: isFrame(leaf) ? kept : leaf, selected: !isFrame(leaf) || over ? leaf : usePanes.getState().selected })
  return true
}

/** The user picked pane `id` (a press in it, its tab, the keyboard): it is selected; a chat is focused too and, unless
 * a draft, the URL moves to it. */
export function picked(id: string): void {
  const { focused, select, focus } = usePanes.getState()
  select(id)
  if (focused === id || isFrame(id)) return
  focus(id)
  if (!isDraft(id)) go?.(id)
}

/** Edge bands are this share of the pane's width/height; the rest is `center` (replace). */
const BAND = 0.25

/** The pane under (`x`, `y`) and the drop edge for that point; `null` off any pane. */
export function targetAt(x: number, y: number): Target | null {
  const el = document.elementFromPoint(x, y)?.closest<HTMLElement>('[data-pane]')
  const session = el?.dataset.pane
  if (!el || !session) return null
  const rect = el.getBoundingClientRect()
  const fx = (x - rect.left) / rect.width
  const fy = (y - rect.top) / rect.height
  const edge: Edge = fx < BAND ? 'left' : fx > 1 - BAND ? 'right' : fy < BAND ? 'top' : fy > 1 - BAND ? 'bottom' : 'center'
  return { session, edge, rect }
}

/** The region a drop at `t` would fill, in viewport px. */
export function zone({ edge, rect: r }: Target): { left: number; top: number; width: number; height: number } {
  const w = r.width / 2
  const h = r.height / 2
  switch (edge) {
    case 'left':
      return { left: r.left, top: r.top, width: w, height: r.height }
    case 'right':
      return { left: r.left + w, top: r.top, width: w, height: r.height }
    case 'top':
      return { left: r.left, top: r.top, width: r.width, height: h }
    case 'bottom':
      return { left: r.left, top: r.top + h, width: r.width, height: h }
    case 'center':
      return { left: r.left, top: r.top, width: r.width, height: r.height }
  }
}

/** The press being dispatched (cleared once it is done): a Trellis focus it causes inside a pane's content is that
 * pane's own pick (`ViewBody`), which may decline it (a control in a chat while a frame is selected). */
let pressing: Element | null = null

/**
 * `ws` is the layout now: the store follows its document; Trellis focus (a tab, the keyboard) is a user pick; a view
 * closed by Trellis (its tab's ×, ⌥⌘W) moves the URL off it; a web frame closed for good (not popped out) loses its
 * native view. Returns the detach, which flushes the document to storage and keeps the mirror.
 */
export function attachWorkspace(ws: WorkspaceHandle, navigate?: (session: string) => void): () => void {
  go = navigate ?? null
  const sync = () => {
    const snap = ws.getSnapshot()
    mirror(snap.document, snap.views.map((v) => v.id), snap.dragging)
  }
  const down = (e: Event) => {
    pressing = e.target instanceof Element ? e.target : null
    setTimeout(() => (pressing = null), 0)
  }
  window.addEventListener('pointerdown', down, true)
  const offs = [
    ws.subscribe(sync),
    ws.on('focus', (id) => {
      if (internal || id === null || !ws.getDocument().views[id]) return
      if (pressing?.closest('[data-pane]')) return
      picked(id)
    }),
    ws.on('close', (v: ViewInfo) => {
      if (isWeb(v.id) && !useWebViews.getState().views[webId(v.id)]?.popped) useWebViews.getState().close(webId(v.id))
      if (internal) return
      const to = refocus(v.id)
      if (to) go?.(to)
    }),
  ]
  usePanes.setState({ ws })
  sync()
  if (wanted) usePanes.getState().show(wanted)
  return () => {
    for (const off of offs) off()
    window.removeEventListener('pointerdown', down, true)
    if (usePanes.getState().ws !== ws) return
    try {
      const doc = ws.getDocument()
      mirror(doc)
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...doc, version: VERSION }))
    } catch {
      // Storage full or unavailable: Trellis's own debounced save is all there is.
    }
    usePanes.setState({ ws: null, dragging: false })
  }
}

/** The engine was switched or the app forgot its panes: nothing focused, nothing selected, no drag. */
export function resetPanes(): void {
  wanted = null
  usePanes.setState({ focused: null, selected: null, drag: null })
}

/** `fn` applied to the persisted document; a garbled one is removed. */
function editStored(fn: (doc: LayoutDocument) => LayoutDocument): void {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return
    const parsed = JSON.parse(raw) as LayoutDocument
    if (parsed?.schema !== 1 || parsed.version !== VERSION) throw new Error('stale panes document')
    const next = sanitize(fn(sanitize(parsed)))
    // Nothing left: the workspace starts from its default (the route's chat), not from an empty layout.
    if (Object.keys(next.views).length === 0) localStorage.removeItem(STORAGE_KEY)
    else localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...next, version: VERSION }))
  } catch {
    try {
      localStorage.removeItem(STORAGE_KEY)
    } catch {
      // No storage: nothing persisted either.
    }
  }
}

/**
 * Before a workspace mounts on the persisted layout: views of another engine, web frames whose page this window no
 * longer holds (a restart), and — once the engine's session list is known (`sessions`) — chats and frames of sessions
 * that no longer exist are dropped (a draft's are kept). A garbled or stale document is removed, so the workspace
 * starts from its default: the route's single chat.
 */
export function prepareStored(o: { engine: string; sessions: ReadonlySet<string> | null; types: ReadonlySet<string> }): void {
  editStored((doc) => {
    for (const [id, record] of Object.entries(doc.views)) {
      const owner = ownerOf(id, record)
      const gone =
        record.params?.engine !== o.engine ||
        !o.types.has(record.type) ||
        (isWeb(id) && !useWebViews.getState().views[webId(id)]) ||
        (o.sessions !== null && !isDraft(owner) && !o.sessions.has(owner))
      if (gone) delete doc.views[id]
    }
    return doc
  })
}

/**
 * 205 D5: web frame `id` opens to the right of `session`'s pane. An open tile is left where it is. `false` when
 * `session` has no pane here: the frame then waits for its card.
 */
export function placeWeb(session: string, id: string): boolean {
  const { leaves } = usePanes.getState()
  const leaf = webLeaf(id)
  if (leaves.includes(leaf)) return true
  if (!leaves.includes(session)) return false
  return placeView(session, leaf, 'right', webSpec(leaf))
}

/** Close web frame `id` for good: its tile, its store entry and its native web view. */
export function closeWeb(id: string): void {
  usePanes.getState().close(webLeaf(id))
  useWebViews.getState().close(id)
}

/**
 * 205 D6: frame `view.id` moves into its own window (or focuses the one that holds it). Only once the window opened is
 * the frame recorded (created first when the store lost it, e.g. after a restart) as popped and its tile closed; a
 * rejection leaves everything as it was.
 */
export async function popOutWeb(view: Pick<WebView, 'id' | 'session' | 'url' | 'title'>): Promise<void> {
  await openPopout({ kind: 'web', id: view.id, url: view.url, title: view.title })
  const store = useWebViews.getState()
  if (!store.views[view.id]) store.open(view)
  store.patch(view.id, { popped: true })
  usePanes.getState().close(webLeaf(view.id))
}

/** The route's chat is shown: on a URL change, and again when the layout is left holding only frames (or nothing)
 * while the URL stayed (the chat closed beside a web tile, so a sidebar click on that same URL changes nothing else). */
export function useRoutedPane(session: string): void {
  const stranded = usePanes((s) => s.ws !== null && s.leaves.every((l) => isFrame(l)))
  useEffect(() => usePanes.getState().show(session), [session, stranded])
}
