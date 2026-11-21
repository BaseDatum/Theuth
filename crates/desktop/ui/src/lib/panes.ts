// Tiled chats: a binary split tree over session ids, the focused pane (the URL follows it), and the
// sidebar → pane drag in flight. `root === null` is the plain single chat the route names. In memory only.
// A leaf may also hold a draft id (`draft:<uuid>[?project=<label>]`, 25 §12.3): a new chat opened from the sidebar whose
// session does not exist until the first send; `place(draft, session, 'center')` then swaps it in.
// A leaf may also hold a web frame (`web:<id>`, 205 D5, `lib/web-views.ts`): never focused, never in the URL.

import { create } from 'zustand'
import { isWeb, useWebViews, webLeaf } from '@/lib/web-views'

export type Edge = 'left' | 'right' | 'top' | 'bottom' | 'center'
export type Node = { session: string } | { dir: 'row' | 'col'; ratio: number; a: Node; b: Node }
export type Path = ('a' | 'b')[]
/** The pane under the pointer and which of its edges (or its middle) the drop snaps to. */
export type Target = { session: string; edge: Edge; rect: DOMRect }
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

/** Sessions shown, left-to-right, top-to-bottom. */
export const leaves = (n: Node): string[] => ('session' in n ? [n.session] : [...leaves(n.a), ...leaves(n.b)])
/** `leaves` without web frames: the chat panes (drafts included), for whatever expects a session id. */
export const chatLeaves = (n: Node): string[] => leaves(n).filter((l) => !isWeb(l))

/** The one leaf a phone shows (it does not tile, 25 §12.3): the focused pane while the tree has it, else its first chat leaf; the route's `session` without a tree (or with only web frames). */
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

type PanesStore = {
  root: Node | null
  /** Pane that window-level actions (hotkeys, `document.title`) address; the route keeps the URL on it. */
  focused: string | null
  drag: Drag | null
  focus(session: string): void
  /** The route landed on `session`: focus its pane, or show it in the focused pane when it has none. */
  show(session: string): void
  /** Show `session` at `edge` of the pane showing `target` (`center` replaces it). A pane already showing `session` moves. */
  place(target: string, session: string, edge: Edge): void
  /** New chat where the user is looking: with ≥ 2 panes the focused one (fallback: the first) becomes a fresh draft;
   * a focused draft is kept, never stacked. Returns the draft to type into, or `null` for a single pane: go to `/`. */
  fresh(project?: string | null): string | null
  /** Close the pane showing `session`; returns the session the URL should move to, or `null` to stay (also
   * when only a draft is left: the URL then names a chat no longer shown until the draft's first send replaces it). */
  close(session: string): string | null
  resize(path: Path, ratio: number): void
  startDrag(session: string, title: string, x: number, y: number): void
  moveDrag(x: number, y: number): void
  /** Drop (or cancel with `drop = false`); returns the dropped session when a pane took it. */
  endDrag(drop: boolean): string | null
}

/** A lone real leaf is the single-chat mode; a lone draft or web frame stays a tree, since no route names it (closing
 * the chat beside a web tile leaves that tile on screen, and the URL on the closed chat, like a draft). */
const normal = (n: Node | null): Node | null => (n && 'session' in n && !isDraft(n.session) && !isWeb(n.session) ? null : n)

export const usePanes = create<PanesStore>()((set, get) => ({
  root: null,
  focused: null,
  drag: null,
  focus(session) {
    if (get().focused !== session && !isWeb(session)) set({ focused: session })
  },
  show(session) {
    const { root, focused } = get()
    if (root && !leaves(root).includes(session)) {
      const chats = chatLeaves(root)
      const target = focused && chats.includes(focused) ? focused : chats[0]
      // Only web frames left: the chat goes beside them instead of over one.
      const lone = leaves(root)[0] as string
      set({ root: target ? normal(replace(root, target, () => ({ session }))) : replace(root, lone, (leaf) => ({ dir: 'row', ratio: 0.5, a: { session }, b: leaf })) })
    }
    set({ focused: session })
  },
  place(target, session, edge) {
    if (target === session) {
      get().focus(session)
      return
    }
    // Moving an open pane: lift it first so the target's leaf is still there to split.
    const root = (get().root && remove(get().root as Node, session)) ?? { session: target }
    const fresh: Node = { session }
    const next =
      edge === 'center'
        ? replace(root, target, () => fresh)
        : replace(root, target, (leaf) => {
            const first = edge === 'left' || edge === 'top'
            return { dir: edge === 'left' || edge === 'right' ? 'row' : 'col', ratio: 0.5, a: first ? fresh : leaf, b: first ? leaf : fresh }
          })
    // A web frame never takes focus (205 D5): the URL stays on the chat beside it.
    set({ root: normal(next), focused: isWeb(session) ? (get().focused ?? target) : session })
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
    const to = leaves(next).find((s) => !isDraft(s) && !isWeb(s)) ?? null
    set({ focused: to ?? chatLeaves(next)[0] ?? null })
    return to
  },
  resize(path, ratio) {
    const { root } = get()
    if (root) set({ root: at(root, path, (s) => ({ ...s, ratio })) })
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
