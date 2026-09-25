import { type BlockNoteEditor, createExtension } from '@blocknote/core'
import type { Node as PmNode } from 'prosemirror-model'
import { Plugin, PluginKey, type Transaction } from 'prosemirror-state'
import { Decoration, DecorationSet } from 'prosemirror-view'
import { ySyncPluginKey, yUndoPluginKey } from 'y-prosemirror'
import type { UndoManager } from 'yjs'
import type { Authored, EngineProvider } from '@/lib/doc/provider'

/** 59 §6.4: unmask at `max(24 chars/s, len / 1.5 s)`. */
export const MIN_CHARS_PER_S = 24
export const MAX_REVEAL_MS = 1500
/** Reduced motion: the text appears at once and the block glows for this long. */
export const FLASH_MS = 600
/** An `Authored` whose blocks never arrive is forgotten after this. */
const PENDING_MS = 5000

/** How long revealing `len` characters takes, in ms. */
export function revealDuration(len: number): number {
  return Math.min(MAX_REVEAL_MS, (len / MIN_CHARS_PER_S) * 1000)
}

/** Characters visible after `elapsedMs` of a `len`-char reveal (monotone, clamped). */
export function revealedChars(len: number, elapsedMs: number): number {
  if (len <= 0 || elapsedMs <= 0) return 0
  const total = revealDuration(len)
  if (elapsedMs >= total) return len
  return Math.floor((elapsedMs / total) * len)
}

/** Who last wrote a block, for the hover chip (59 §5.5). Nothing from Yjs internals. */
export type Attribution = { who: 'agent' | 'owner'; at: number; session?: string }
export type AttributionMap = Record<string, Attribution>

/** Fold one authorship event into the map: agent blocks from `Authored`, owner blocks from local edits. */
export function attribute(
  map: AttributionMap,
  ev: { who: 'agent'; blocks: string[]; at: number; session?: string } | { who: 'owner'; blocks: string[]; at: number },
): AttributionMap {
  if (ev.blocks.length === 0) return map
  const next = { ...map }
  for (const id of ev.blocks) next[id] = ev.who === 'agent' ? { who: 'agent', at: ev.at, session: ev.session } : { who: 'owner', at: ev.at }
  return next
}

/** "theuth · 14:02" */
export function attributionLabel(a: Attribution): string {
  const hhmm = new Date(a.at).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  return `${a.who === 'agent' ? 'theuth' : 'you'} · ${hhmm}`
}

type Revealing = { start: number; len: number }

type State = {
  /** Announced by `Authored`, not yet in the document. */
  pending: { ids: string[]; at: number }[]
  /** Block id → reveal in progress. */
  revealing: Record<string, Revealing>
  attribution: AttributionMap
  decorations: DecorationSet
  reduced: boolean
}

type Meta =
  | { type: 'authored'; blocks: string[]; at: number; session?: string }
  | { type: 'tick'; now: number }

export const revealKey = new PluginKey<State>('theuth-reveal')

/** Block containers by id, with the position of the node. */
function blockNodes(doc: PmNode, ids: Iterable<string>): Map<string, { node: PmNode; pos: number }> {
  const wanted = new Set(ids)
  const out = new Map<string, { node: PmNode; pos: number }>()
  if (wanted.size === 0) return out
  doc.descendants((node, pos) => {
    if (out.size === wanted.size) return false
    if (node.type.name === 'blockContainer' && typeof node.attrs.id === 'string' && wanted.has(node.attrs.id)) {
      out.set(node.attrs.id, { node, pos })
      return false
    }
    return true
  })
  return out
}

/** Document position of the `n`-th character inside `node` (which starts at `pos`); the node's end when `n` ≥ its length. */
function charPos(node: PmNode, pos: number, n: number): number {
  let seen = 0
  let found = -1
  node.descendants((child, childPos) => {
    if (found !== -1) return false
    if (child.isText) {
      const len = child.text?.length ?? 0
      if (seen + len > n) {
        found = pos + 1 + childPos + (n - seen)
        return false
      }
      seen += len
    }
    return true
  })
  return found === -1 ? pos + node.nodeSize : found
}

function build(doc: PmNode, s: State, now: number, color: string): { decorations: DecorationSet; revealing: Record<string, Revealing> } {
  const decos: Decoration[] = []
  const revealing: Record<string, Revealing> = {}
  const nodes = blockNodes(doc, [...Object.keys(s.revealing), ...Object.keys(s.attribution)])
  let first = true
  for (const [id, r] of Object.entries(s.revealing)) {
    const hit = nodes.get(id)
    if (!hit) continue
    const elapsed = now - r.start
    if (s.reduced) {
      if (elapsed >= FLASH_MS) continue
      revealing[id] = r
      decos.push(Decoration.node(hit.pos, hit.pos + hit.node.nodeSize, { class: 'theuth-reveal-flash', style: `--theuth-agent: ${color}` }))
      continue
    }
    const shown = revealedChars(r.len, elapsed)
    if (shown >= r.len) continue
    revealing[id] = r
    const from = charPos(hit.node, hit.pos, shown)
    const to = hit.pos + hit.node.nodeSize - 1
    if (from < to) decos.push(Decoration.inline(from, to, { class: 'theuth-reveal-hidden' }))
    decos.push(Decoration.node(hit.pos, hit.pos + hit.node.nodeSize, { class: 'theuth-revealing', style: `--theuth-agent: ${color}` }))
    if (first) {
      first = false
      decos.push(
        Decoration.widget(
          hit.pos + 1,
          () => {
            const el = document.createElement('span')
            el.className = 'theuth-agent-caret'
            el.style.setProperty('--theuth-agent', color)
            el.textContent = 'theuth'
            el.contentEditable = 'false'
            return el
          },
          { side: -1, key: 'theuth-caret' },
        ),
      )
    }
  }
  for (const [id, a] of Object.entries(s.attribution)) {
    if (a.who !== 'agent') continue
    const hit = nodes.get(id)
    if (!hit) continue
    decos.push(Decoration.node(hit.pos, hit.pos + hit.node.nodeSize, { 'data-authored': attributionLabel(a), style: `--theuth-agent: ${color}` }))
  }
  return { decorations: DecorationSet.create(doc, decos), revealing }
}

/** Which block containers the selection touches (attribution of local edits). */
function selectedBlocks(tr: Transaction): string[] {
  const ids: string[] = []
  const { $from, $to } = tr.selection
  for (const $p of [$from, $to]) {
    for (let d = $p.depth; d > 0; d--) {
      const n = $p.node(d)
      if (n.type.name === 'blockContainer' && typeof n.attrs.id === 'string') {
        if (!ids.includes(n.attrs.id)) ids.push(n.attrs.id)
        break
      }
    }
  }
  return ids
}

/**
 * The reveal plugin (59 §6.4): on `Authored{by: agent}` the touched blocks are masked as soon as
 * the update lands and unmasked left to right; layout is final from the first frame (the mask is
 * `color: transparent`, never a reflow). A selection inside a revealing block ends its reveal.
 * Also keeps the hover attribution (§5.5) as a `data-authored` node decoration.
 */
export function revealPlugin(color: string, reduced = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches): Plugin<State> {
  let raf = 0
  return new Plugin<State>({
    key: revealKey,
    state: {
      init: () => ({ pending: [], revealing: {}, attribution: {}, decorations: DecorationSet.empty, reduced: reduced() }),
      apply(tr, prev) {
        const meta = tr.getMeta(revealKey) as Meta | undefined
        const now = meta?.type === 'tick' ? meta.now : performance.now()
        let s: State = { ...prev, decorations: prev.decorations.map(tr.mapping, tr.doc) }
        if (meta?.type === 'authored') {
          s.pending = [...s.pending.filter((p) => now - p.at < PENDING_MS), { ids: meta.blocks, at: now }]
          s.attribution = attribute(s.attribution, { who: 'agent', blocks: meta.blocks, at: meta.at, session: meta.session })
        }
        let dirty = meta?.type === 'tick' || meta?.type === 'authored'
        if (tr.docChanged) {
          dirty = true
          // Blocks announced earlier are in the document now: start their reveal.
          if (s.pending.length > 0) {
            const present = blockNodes(tr.doc, s.pending.flatMap((p) => p.ids))
            if (present.size > 0) {
              const revealing = { ...s.revealing }
              for (const [id, { node }] of present) revealing[id] = { start: now, len: node.textContent.length }
              s = {
                ...s,
                revealing,
                pending: s.pending.map((p) => ({ ...p, ids: p.ids.filter((id) => !present.has(id)) })).filter((p) => p.ids.length > 0),
              }
            }
          }
          // A local edit (not y-sync replaying a remote update) attributes its blocks to the owner.
          const sync: unknown = tr.getMeta(ySyncPluginKey)
          const remote = !!sync && typeof sync === 'object' && 'isChangeOrigin' in sync && sync.isChangeOrigin === true
          if (!remote) {
            const ids = selectedBlocks(tr)
            if (ids.length > 0) s.attribution = attribute(s.attribution, { who: 'owner', blocks: ids, at: Date.now() })
          }
        }
        if (tr.selectionSet && !tr.docChanged && Object.keys(s.revealing).length > 0) {
          const inside = selectedBlocks(tr).filter((id) => id in s.revealing)
          if (inside.length > 0) {
            const revealing = { ...s.revealing }
            for (const id of inside) delete revealing[id]
            s = { ...s, revealing }
            dirty = true
          }
        }
        if (!dirty) return s
        const built = build(tr.doc, s, now, color)
        return { ...s, decorations: built.decorations, revealing: built.revealing }
      },
    },
    props: {
      decorations(state) {
        return revealKey.getState(state)?.decorations ?? DecorationSet.empty
      },
    },
    view(view) {
      const tick = () => {
        raf = 0
        const s = revealKey.getState(view.state)
        if (!s || Object.keys(s.revealing).length === 0) return
        view.dispatch(view.state.tr.setMeta(revealKey, { type: 'tick', now: performance.now() } satisfies Meta))
      }
      return {
        update() {
          const s = revealKey.getState(view.state)
          if (s && Object.keys(s.revealing).length > 0 && raf === 0) raf = requestAnimationFrame(tick)
        },
        destroy() {
          if (raf !== 0) cancelAnimationFrame(raf)
        },
      }
    },
  })
}

/**
 * y-prosemirror's undo plugin keeps its `Y.UndoManager` in plugin *state* but destroys it in
 * `view.destroy` (detaching it from the doc). Any view remount — `editable` flipping when the
 * document goes live, React StrictMode — leaves Cmd-Z dead. Re-attach the detached manager.
 * ponytail: two Yjs fields (`afterTransactionHandler`, `trackedOrigins`), stable since 13.5; drop when upstream fixes it.
 */
function reviveUndo(editor: BlockNoteEditor<any, any, any>): void {
  const um = yUndoPluginKey.getState(editor.prosemirrorState)?.undoManager as UndoManager | undefined
  if (!um) return
  const attached = um.doc._observers.get('afterTransaction')?.has(um.afterTransactionHandler) ?? false
  if (attached) return
  um.trackedOrigins.add(um)
  um.doc.on('afterTransaction', um.afterTransactionHandler)
}

/** BlockNote extension: the plugin plus the `Authored` subscription that feeds it. `createExtension` returns a creator; called once here. */
export function revealExtension(provider: EngineProvider, color: string) {
  return createExtension(({ editor }: { editor: BlockNoteEditor<any, any, any> }) => {
    const plugin = revealPlugin(color)
    return {
      key: 'theuthReveal',
      prosemirrorPlugins: [plugin],
      mount() {
        reviveUndo(editor)
        return provider.onAuthored((a: Authored) => {
          if (a.by.who !== 'agent') return
          const view = editor.prosemirrorView
          view.dispatch(view.state.tr.setMeta(revealKey, { type: 'authored', blocks: a.blocks, at: a.at, session: a.by.session } satisfies Meta))
        })
      },
    }
  })()
}
