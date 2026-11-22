// Tiled chats: a binary split tree over session ids, the focused pane (the URL follows it), and the
// sidebar → pane drag in flight. `root === null` is the plain single chat the route names. In memory only.
// A leaf may also hold a draft id (`draft:<uuid>[?project=<label>]`, 25 §12.3): a new chat opened from the sidebar whose
// session does not exist until the first send; `place(draft, session, 'center')` then swaps it in.
// A leaf may also hold a web frame (`web:<id>`, 205 D5, `lib/web-views.ts`) or a generic tile (`tile:<id>`, `lib/tiles.ts`):
// both are frames — never focused, never in the URL.

import { useEffect } from 'react'
import { create } from 'zustand'
import { openPopout } from '@/lib/popout'
import { isWeb, type WebView, useWebViews, webId, webLeaf } from '@/lib/web-views'

export type Edge = 'left' | 'right' | 'top' | 'bottom' | 'center'
export type Node = { session: string } | { dir: 'row' | 'col'; ratio: number; a: Node; b: Node }
export type Path = ('a' | 'b')[]
/** The pane under the pointer and which of its edges (or its middle) the drop snaps to. */
export type Target = { session: string; edge: Edge; rect: DOMRect }
/** `session`: the leaf being dragged (a sidebar row's session or draft, or any open leaf when `move`). `move`: a tile
 * header drag rearranging an open leaf (`usePanes.move`); otherwise a sidebar row placing a chat (`place`). */
export type Drag = { session: string; title: string; x: number; y: number; target: Target | null; move?: boolean }

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

/** Sessions shown, left-to-right, top-to-bottom. */
export const leaves = (n: Node): string[] => ('session' in n ? [n.session] : [...leaves(n.a), ...leaves(n.b)])
/** `leaves` without web frames and tiles: the chat panes (drafts included), for whatever expects a session id. */
export const chatLeaves = (n: Node): string[] => leaves(n).filter((l) => !isFrame(l))

/** The one leaf a phone shows (it does not tile, 25 §12.3): the focused pane while the tree has it, else its first chat leaf; the route's `session` without a tree (or with only frames). */
export const shown = (root: Node | null, focused: string | null, session: string): string => {
  const all = root ? chatLeaves(root) : []
  return focused !== null && all.includes(focused) ? focused : (all[0] ?? session)
}

/** The tree with the leaf showing `session` swapped for `f(leaf)`. */
function replace(n: Node, session: string, f: (leaf: Node) => Node): Node {
  if ('session' in n) return n.session === session ? f(n) : n
  return { ...n, a: replace(n.a, session, f), b: replace(n.b, session, f) }
}

/** The tree without the leaf showing `session`; its sibling takes the split's place. `null` when nothing is left. */
function remove(n: Node, session: string): Node | null {
  if ('session' in n) return n.session === session ? null : n
  const a = remove(n.a, session)
  const b = remove(n.b, session)
  if (a === n.a && b === n.b) return n
  return a && b ? { ...n, a, b } : a ?? b
}

/** `f` applied to the split at `path`. */
function at(n: Node, path: Path, f: (split: Extract<Node, { dir: string }>) => Node): Node {
  if ('session' in n) return n
  const [head, ...rest] = path
  if (head === undefined) return f(n)
  return head === 'a' ? { ...n, a: at(n.a, rest, f) } : { ...n, b: at(n.b, rest, f) }
}

/** The tree with the leaves showing `x` and `y` trading places. */
function swap(n: Node, x: string, y: string): Node {
  if ('session' in n) return n.session === x ? { session: y } : n.session === y ? { session: x } : n
  return { ...n, a: swap(n.a, x, y), b: swap(n.b, x, y) }
}

/** `n` with `leaf` split off at `edge` of the leaf showing `target` (`center` replaces that leaf). */
function insert(n: Node, target: string, leaf: string, edge: Edge): Node {
  const fresh: Node = { session: leaf }
  if (edge === 'center') return replace(n, target, () => fresh)
  return replace(n, target, (old) => {
    const first = edge === 'left' || edge === 'top'
    return { dir: edge === 'left' || edge === 'right' ? 'row' : 'col', ratio: 0.5, a: first ? fresh : old, b: first ? old : fresh }
  })
}

/** Edge bands are this share of the pane's width/height; the rest is `center` (replace; a tile move swaps). */
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

/** A region of the panes area in percent of it: where `Panes` absolutely positions a leaf or a divider. */
export type Box = { left: number; top: number; width: number; height: number }
/** The tree flattened for rendering: every leaf with its box (and whether it is the top-left one), every split with its box. */
export type Layout = {
  leaves: { session: string; box: Box; first: boolean }[]
  splits: { path: Path; dir: 'row' | 'col'; ratio: number; box: Box }[]
}

/** `n`'s leaves and splits with their boxes; leaves in `leaves` order. */
export function layout(n: Node, box: Box = { left: 0, top: 0, width: 100, height: 100 }, path: Path = [], out: Layout = { leaves: [], splits: [] }): Layout {
  if ('session' in n) {
    out.leaves.push({ session: n.session, box, first: path.every((p) => p === 'a') })
    return out
  }
  out.splits.push({ path, dir: n.dir, ratio: n.ratio, box })
  const row = n.dir === 'row'
  const a = row ? { ...box, width: box.width * n.ratio } : { ...box, height: box.height * n.ratio }
  const b = row
    ? { ...box, left: box.left + a.width, width: box.width - a.width }
    : { ...box, top: box.top + a.height, height: box.height - a.height }
  layout(n.a, a, [...path, 'a'], out)
  layout(n.b, b, [...path, 'b'], out)
  return out
}

type PanesStore = {
  root: Node | null
  /** Pane that window-level actions (hotkeys, `document.title`) address; the route keeps the URL on it. Always a chat
   * (or draft) — never a frame, so no RPC, composer or sidebar highlight ever sees a `tile:`/`web:` id. */
  focused: string | null
  /** The pane the user last picked, of any kind (a press inside it, a route landing): the one that is lit while the
   * others dim, and the one a sidebar pick or a newly opened surface takes over when it is a frame. A stale value (no
   * longer in the tree) counts as none. */
  selected: string | null
  drag: Drag | null
  focus(session: string): void
  select(leaf: string): void
  /** The route landed on `session`: focus its pane, or show it in the selected frame's pane (taking it over), else in
   * the focused chat's pane, else left of the frames when only frames are left. */
  show(session: string): void
  /** Show `session` at `edge` of the pane showing `target` (`center` replaces it). A pane already showing `session` moves. */
  place(target: string, session: string, edge: Edge): void
  /** A header drag (any leaf): `source` moves to `edge` of `target`; `center` swaps the two. Focus never moves. */
  move(source: string, target: string, edge: Edge): void
  /** Relabel leaves in place, all in one update (`from → to`): a draft becoming its session together with its tiles.
   * Focus and selection follow relabelled leaves. */
  rename(map: Record<string, string>): void
  /** New chat where the user is looking: with ≥ 2 panes the focused one (fallback: the first) becomes a fresh draft;
   * a focused draft is kept, never stacked. Returns the draft to type into, or `null` for a single pane: go to `/`. */
  fresh(project?: string | null): string | null
  /** Close the pane showing `session`; returns the session the URL should move to, or `null` to stay (also
   * when only a draft is left: the URL then names a chat no longer shown until the draft's first send replaces it). */
  close(session: string): string | null
  resize(path: Path, ratio: number): void
  /** `move`: a tile header drag (`Drag.move`). */
  startDrag(session: string, title: string, x: number, y: number, move?: boolean): void
  moveDrag(x: number, y: number): void
  /** Drop (or cancel with `drop = false`); returns the dropped session when a pane took it from the sidebar (a tile
   * move returns `null`: nothing to navigate). */
  endDrag(drop: boolean): string | null
}

/** A lone real leaf is the single-chat mode; a lone draft or frame stays a tree, since no route names it (closing
 * the chat beside a web tile leaves that tile on screen, and the URL on the closed chat, like a draft). */
const normal = (n: Node | null): Node | null => (n && 'session' in n && !isDraft(n.session) && !isFrame(n.session) ? null : n)

/** A web frame taken over (its pane given to something else) is closed for good: store entry and native view. Generic
 * tiles' payloads go with their leaf (`lib/tiles.ts`). */
const dispose = (leaf: string) => isWeb(leaf) && useWebViews.getState().close(webId(leaf))

export const usePanes = create<PanesStore>()((set, get) => ({
  root: null,
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
    const { root, focused, selected } = get()
    if (root && !leaves(root).includes(session)) {
      const all = leaves(root)
      const chats = chatLeaves(root)
      const target = focused && chats.includes(focused) ? focused : chats[0]
      if (selected && isFrame(selected) && all.includes(selected)) {
        // The picked pane is a tile: the chat takes it over, as it would a picked chat pane.
        set({ root: normal(replace(root, selected, () => ({ session }))) })
        dispose(selected)
      }
      // Only frames left: the chat goes left of all of them (the root splits), not into one corner.
      else set({ root: target ? normal(replace(root, target, () => ({ session }))) : { dir: 'row', ratio: 0.5, a: { session }, b: root } })
    }
    set({ focused: session, selected: session })
  },
  place(target, session, edge) {
    if (target === session) {
      get().focus(session)
      return
    }
    const { selected } = get()
    // Moving an open pane: lift it first so the target's leaf is still there to split.
    const root = (get().root && remove(get().root as Node, session)) ?? { session: target }
    const next = insert(root, target, session, edge)
    // A frame never takes focus (205 D5): the URL stays on the chat beside it. It is selected when it took over the
    // selected pane; a placed chat is focused and selected.
    const focused = get().focused ?? (isFrame(target) ? null : target)
    const over = edge === 'center' && selected === target
    set({ root: normal(next), focused: isFrame(session) ? focused : session, selected: !isFrame(session) || over ? session : selected })
    if (edge === 'center') dispose(target)
  },
  move(source, target, edge) {
    const { root } = get()
    if (!root || source === target) return
    const all = leaves(root)
    if (!all.includes(source) || !all.includes(target)) return
    if (edge === 'center') return set({ root: swap(root, source, target) })
    // Both leaves stay, so the tree keeps ≥ 2 leaves and `remove` cannot empty it.
    set({ root: insert(remove(root, source) as Node, target, source, edge) })
  },
  rename(map) {
    const { root, focused, selected } = get()
    const relabel = (n: Node): Node => ('session' in n ? (map[n.session] ? { session: map[n.session] as string } : n) : { ...n, a: relabel(n.a), b: relabel(n.b) })
    set({ root: root && normal(relabel(root)), focused: (focused && map[focused]) ?? focused, selected: (selected && map[selected]) ?? selected })
  },
  fresh(project) {
    const { root, focused } = get()
    const all = root ? chatLeaves(root) : []
    const target = focused && all.includes(focused) ? focused : all[0]
    if (target && isDraft(target)) return target
    if (all.length < 2) return null
    const draft = newDraft(project)
    set({ root: replace(root as Node, target as string, () => ({ session: draft })), focused: draft })
    return draft
  },
  close(session) {
    const { root, focused } = get()
    if (!root) return null
    const next = remove(root, session)
    set({ root: normal(next) })
    if (focused !== session || !next) return null
    const to = leaves(next).find((s) => !isDraft(s) && !isFrame(s)) ?? null
    set({ focused: to ?? chatLeaves(next)[0] ?? null })
    return to
  },
  resize(path, ratio) {
    const { root } = get()
    if (root) set({ root: at(root, path, (s) => ({ ...s, ratio })) })
  },
  startDrag(session, title, x, y, move) {
    document.body.classList.add('select-none')
    set({ drag: { session, title, x, y, target: null, ...(move ? { move } : {}) } })
  },
  moveDrag(x, y) {
    const { drag } = get()
    if (!drag) return
    let target = targetAt(x, y)
    // A tile dropped on itself goes nowhere: no zone.
    if (target && drag.move && target.session === drag.session) target = null
    set({ drag: { ...drag, x, y, target } })
  },
  endDrag(drop) {
    document.body.classList.remove('select-none')
    const { drag } = get()
    set({ drag: null })
    if (!drop || !drag?.target) return null
    if (drag.move) {
      get().move(drag.session, drag.target.session, drag.target.edge)
      return null
    }
    get().place(drag.target.session, drag.session, drag.target.edge)
    return drag.session
  },
}))

/**
 * 205 D5: web frame `id` opens to the right of `session`'s pane (the single chat on screen counts as one). An open
 * tile is left where it is. `false` when `session` has no pane here: the frame then waits for its card.
 */
export function placeWeb(session: string, id: string): boolean {
  const { root, place } = usePanes.getState()
  const leaf = webLeaf(id)
  if (root && leaves(root).includes(leaf)) return true
  if (root && !leaves(root).includes(session)) return false
  place(session, leaf, 'right')
  return true
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

/** The route's chat is shown: on a URL change, and again when the tree is left holding only frames while the URL
 * stayed (the chat closed beside a web tile, so a sidebar click on that same URL changes nothing else). */
export function useRoutedPane(session: string): void {
  const stranded = usePanes((s) => s.root !== null && leaves(s.root).every((l) => isFrame(l)))
  useEffect(() => usePanes.getState().show(session), [session, stranded])
}
