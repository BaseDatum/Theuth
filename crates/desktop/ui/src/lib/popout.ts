// A chat, an artifact, a note, a notebook, a meeting, a PTY, a file or a diff in its own desktop window: the same
// bundle at the same route with `?window=<label>`. The label (`popout-<kind>-<id>`) is what the Tauri host keys the window on
// (focus-if-open, remembered size/position, the `popout-*` capability); in a browser it is the `window.open` name.

import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { container } from '@/lib/platform'
import { viewTitle } from '@/lib/view-title'
import { isTauri } from '@/lib/tauri'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'

export type PopoutTarget =
  | { kind: 'chat'; session: string }
  | { kind: 'artifact'; callId: string }
  /** `/notes/$id`: the note's editor alone; `fresh` (a note just created) adds `?new=true` so the page selects the title. */
  | { kind: 'note'; doc: string; fresh?: true }
  /** `/notebooks/$id` (144 wave 2, Sky's ruling): the notebook's three columns alone; landscape, ~1100 × 720. */
  | { kind: 'notebook'; notebook: string }
  /** `/meetings/$id`: the control center alone. */
  | { kind: 'meeting'; meeting: string }
  /** `/meetings/$id?pane=note`: the meeting's note editor alone (`?pane` is what tells it from `meeting`). */
  | { kind: 'meeting-note'; meeting: string }
  /** `/pty/$session/$job`: one agent-started terminal alone (`components/chat/TerminalViewer.tsx`). */
  | { kind: 'pty'; session: string; job: string }
  /** `/terminal/$engine/$shell`: a standalone login shell on a mesh/cloud engine (no session, no job); label `popout-terminal-<engine>-<shell>`. */
  | { kind: 'node-terminal'; engine: string; shell: string; name?: string }
  /** `/codemap/$session/$root`: one session's CodeMap on one root alone (73 §6.9). */
  | { kind: 'codemap'; session: string; root: string }
  /** `/talk/$session`: the session's talk view alone (76 §0.12); it attaches to the running talk, never starts one. */
  | { kind: 'talk'; session: string }
  /** `/vm/$name`: one sandbox VM's desktop alone (79 §6.2); label `popout-vm-<name>`. */
  | { kind: 'vm'; name: string }
  /** A sidebar page (`/costs`, `/attention`, `/artifacts`, `/settings/$tab`, …) alone, navigable inside its window; label
   * `popout-page-<route>`, which is also what marks the URL as a page window (a page route has no id to key on). `key`
   * (New chat: one per press) makes the label `popout-page-<route>-<key>`, a new window instead of focusing the open
   * one; like `note.fresh` it lives in the label only, never in the parsed target. */
  | { kind: 'page'; path: string; key?: string }
  /** `/web/$id?url=…&title=…` (205 D6): one web view frame alone; it hosts the shell's `web-<id>` child web view. */
  | { kind: 'web'; id: string; url: string; title: string }
  /** `/file/$session?path=…&line=…&end_line=…` (212): one `show_file` file alone, following the disk. */
  | { kind: 'file'; session: string; path: string; line?: number; end_line?: number }
  /** `/diff/$session?spec=<DiffSpec JSON>` (212): one `show_diff` diff alone; the label keys on a hash of the spec. */
  | { kind: 'diff'; session: string; spec: DiffSpec }
  /** `/tile/$kind/$session?props=<JSON>`: one generic tile (`components/tiles/registry`, kind `tile`) alone, drawn by its
   * kind's own renderer — every tile kind without a dedicated route above (media, diagram, jobs, Context, …). Props too
   * long for a URL ride in localStorage instead (`?stash=<hash>`, `TILE_STASH`); the label keys on a hash of them. */
  | { kind: 'tile'; tile: string; session: string; props: unknown }

/** FNV-1a of `s`, 8 hex digits: a diff spec's short, label-safe name. */
function fnv(s: string): string {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193)
  return (h >>> 0).toString(16).padStart(8, '0')
}

/** Tile props longer than this (a diagram's svg) go through localStorage, not the URL. */
const INLINE_PROPS = 4000
/** The localStorage key prefix of stashed tile props (shared by every window of the origin). */
export const TILE_STASH = 'popout-tile-props:'
/** How many stashes are kept; older ones are dropped as new ones are written. */
const STASH_KEEP = 16
/** The localStorage key prefix recording open standalone node-terminal windows so `popout-closed` can run `shell_close`. */
const NODE_TERMINAL_PREFIX = 'node-terminal-popout:'

const propsJson = (props: unknown): string => JSON.stringify(props ?? null)
/** A stash's id: lettered, so the router never reads it as a number (an all-digit hash would lose its leading zeros). */
const stashId = (json: string): string => `p${fnv(json)}`

const idOf = (t: PopoutTarget): string =>
  t.kind === 'tile' ? `${t.tile}-${t.session}-${fnv(propsJson(t.props))}` :
  t.kind === 'page' ? `${t.path.replace(/^\//, '')}${t.key ? `-${t.key}` : ''}` :
  t.kind === 'web' ? t.id :
  t.kind === 'file' ? `${t.session}-${t.path}` :
  t.kind === 'diff' ? `${t.session}-${fnv(JSON.stringify(t.spec))}` :
  t.kind === 'chat' || t.kind === 'talk'
    ? t.session
    : t.kind === 'artifact'
      ? t.callId
      : t.kind === 'note'
        ? t.doc
        : t.kind === 'notebook'
          ? t.notebook
          : t.kind === 'vm'
            ? t.name
            : t.kind === 'pty'
              ? `${t.session}-${t.job}`
              : t.kind === 'node-terminal'
                ? `${t.engine}-${t.shell}`
                : t.kind === 'codemap'
                  ? `${t.session}-${t.root}`
                  : t.meeting

function tilePath(t: Extract<PopoutTarget, { kind: 'tile' }>): string {
  const json = propsJson(t.props)
  const q = json.length > INLINE_PROPS ? `stash=${stashId(json)}` : `props=${encodeURIComponent(json)}`
  return `/tile/${encodeURIComponent(t.tile)}/${encodeURIComponent(t.session)}?${q}`
}

/** Write `t`'s props where its window will read them, when they are too long for its URL (`tilePath`). */
export function stashTileProps(t: PopoutTarget, store: Storage | undefined = globalThis.localStorage): void {
  if (t.kind !== 'tile' || !store) return
  const json = propsJson(t.props)
  if (json.length <= INLINE_PROPS) return
  try {
    // Each stash is `<written at ms>\n<json>`: storage key order is no age order, so the oldest go by their stamp.
    const old: [string, number][] = []
    for (let i = 0; i < store.length; i++) {
      const k = store.key(i)
      if (k !== null && k.startsWith(TILE_STASH) && k !== TILE_STASH + stashId(json)) old.push([k, parseInt(store.getItem(k) ?? '', 10) || 0])
    }
    old.sort((a, b) => a[1] - b[1])
    for (const [k] of old.slice(0, Math.max(0, old.length - (STASH_KEEP - 1)))) store.removeItem(k)
    store.setItem(TILE_STASH + stashId(json), `${Date.now()}\n${json}`)
  } catch (e) {
    console.warn('popout stash', e)
  }
}

/** Record `{ engine, shell }` for a node-terminal popout label so the main window can end the remote shell on `popout-closed`. */
export function registerNodeTerminalPopout(
  label: string,
  target: { engine: string; shell: string },
  store: Storage | undefined = globalThis.localStorage,
): void {
  if (!store) return
  try {
    store.setItem(NODE_TERMINAL_PREFIX + label, JSON.stringify(target))
  } catch {
    // ignore storage errors
  }
}

/** Consume and remove the `{ engine, shell }` recorded for a closed node-terminal popout label. */
export function takeNodeTerminalPopout(
  label: string,
  store: Storage | undefined = globalThis.localStorage,
): { engine: string; shell: string } | null {
  if (!store) return null
  try {
    const raw = store.getItem(NODE_TERMINAL_PREFIX + label)
    if (!raw) return null
    store.removeItem(NODE_TERMINAL_PREFIX + label)
    const parsed = JSON.parse(raw) as { engine?: unknown; shell?: unknown }
    return typeof parsed.engine === 'string' && typeof parsed.shell === 'string'
      ? { engine: parsed.engine, shell: parsed.shell }
      : null
  } catch {
    return null
  }
}

/** A popped tile's props: its `props` search value (JSON, or the object the router already parsed it into), else the
 * localStorage stash `stash` names; `undefined` when neither is there (a stash dropped since). */
export function tileProps(search: { props?: unknown; stash?: unknown }, store: Storage | undefined = globalThis.localStorage): unknown {
  if (search.props !== undefined && search.props !== null && search.props !== '') {
    if (typeof search.props !== 'string') return search.props
    try {
      return JSON.parse(search.props)
    } catch {
      return search.props
    }
  }
  if (search.stash === undefined || search.stash === null || !store) return undefined
  const stash = store.getItem(TILE_STASH + String(search.stash))
  if (stash === null) return undefined
  try {
    return JSON.parse(stash.slice(stash.indexOf('\n') + 1))
  } catch {
    return undefined
  }
}

const pathOf = (t: PopoutTarget): string =>
  t.kind === 'tile' ? tilePath(t) :
  t.kind === 'page' ? t.path :
  t.kind === 'web' ? `/web/${encodeURIComponent(t.id)}?url=${encodeURIComponent(t.url)}&title=${encodeURIComponent(t.title)}` :
  t.kind === 'file' ? `/file/${encodeURIComponent(t.session)}?path=${encodeURIComponent(t.path)}${t.line === undefined ? '' : `&line=${t.line}`}${t.end_line === undefined ? '' : `&end_line=${t.end_line}`}` :
  t.kind === 'diff' ? `/diff/${encodeURIComponent(t.session)}?spec=${encodeURIComponent(JSON.stringify(t.spec))}` :
  t.kind === 'chat' || t.kind === 'talk'
    ? `/${t.kind === 'chat' ? 'c' : 'talk'}/${encodeURIComponent(t.session)}`
    : t.kind === 'artifact'
      ? `/artifacts/${encodeURIComponent(t.callId)}`
      : t.kind === 'note'
        ? `/notes/${encodeURIComponent(t.doc)}${t.fresh ? '?new=true' : ''}`
        : t.kind === 'notebook'
          ? `/notebooks/${encodeURIComponent(t.notebook)}`
          : t.kind === 'vm'
            ? `/vm/${encodeURIComponent(t.name)}`
            : t.kind === 'pty'
              ? `/pty/${encodeURIComponent(t.session)}/${encodeURIComponent(t.job)}`
              : t.kind === 'node-terminal'
                ? `/terminal/${encodeURIComponent(t.engine)}/${encodeURIComponent(t.shell)}${t.name ? `?name=${encodeURIComponent(t.name)}` : ''}`
                : t.kind === 'codemap'
                  ? `/codemap/${encodeURIComponent(t.session)}/${encodeURIComponent(t.root)}`
                  : `/meetings/${encodeURIComponent(t.meeting)}${t.kind === 'meeting-note' ? '?pane=note' : ''}`

/** Tauri window labels allow `[A-Za-z0-9/:_-]`; ids are passed through the same filter. */
const safe = (id: string) => id.replace(/[^A-Za-z0-9_-]/g, '_')

export const popoutLabel = (t: PopoutTarget): string =>
  t.kind === 'node-terminal'
    ? `popout-terminal-${safe(t.engine)}-${safe(t.shell)}`
    : `popout-${t.kind}-${safe(idOf(t))}`

export const popoutUrl = (t: PopoutTarget): string => {
  const path = pathOf(t)
  return `${path}${path.includes('?') ? '&' : '?'}window=${popoutLabel(t)}`
}

/** A draft's Open in new window (its pane header): a New chat window of its own — one per `key` (the draft's id, or
 * the `/new` page's), so pressing again focuses it — with the draft's project preset kept. */
export function newChatPopout(key: string, preset?: string): PopoutTarget {
  const id = key.startsWith('draft:') ? key.slice('draft:'.length) : key
  return { kind: 'page', path: `/new${preset ? `?project=${encodeURIComponent(preset)}` : ''}`, key: (id.split('?')[0] ?? id).slice(0, 8) }
}

/** 143 D3: where a just-created note opens — its own window on a desktop (or a browser), the page itself on a phone or iOS. */
export const noteOpenMode = (isMobile: boolean, canPopout: boolean): 'popout' | 'navigate' => (!isMobile && canPopout ? 'popout' : 'navigate')

/** A popped file's `line`/`end_line` search value (a string from the URL, a number from the router's JSON reading):
 * a positive integer, else none. */
export function lineParam(v: unknown): number | undefined {
  const n = typeof v === 'number' ? v : typeof v === 'string' && v !== '' ? Number(v) : NaN
  return Number.isInteger(n) && n > 0 ? n : undefined
}

/** A popped diff's `spec` search value: its JSON (from the URL) or the object the router already parsed it into;
 * `null` when it is not a `DiffSpec`. */
export function specParam(v: unknown): DiffSpec | null {
  let spec = v
  if (typeof v === 'string') {
    try {
      spec = JSON.parse(v)
    } catch {
      return null
    }
  }
  return spec !== null && typeof spec === 'object' && !Array.isArray(spec) && typeof (spec as DiffSpec).mode === 'string' ? (spec as DiffSpec) : null
}

/** What a window at `href` shows alone, or `null` for the ordinary shell (no `?window=` or a route that has no popped form). */
export function popoutTarget(href: string): PopoutTarget | null {
  const url = new URL(href, 'http://x')
  const label = url.searchParams.get('window')
  if (!label) return null
  const [, head, id, sub] = url.pathname.split('/')
  if (label.startsWith('popout-page-')) return { kind: 'page', path: url.pathname }
  if (!id) return null
  if (head === 'pty') return sub ? { kind: 'pty', session: decodeURIComponent(id), job: decodeURIComponent(sub) } : null
  if (head === 'terminal') {
    if (!sub) return null
    const name = url.searchParams.get('name') || undefined
    return { kind: 'node-terminal', engine: decodeURIComponent(id), shell: decodeURIComponent(sub), ...(name ? { name } : {}) }
  }
  if (head === 'codemap') return sub ? { kind: 'codemap', session: decodeURIComponent(id), root: decodeURIComponent(sub) } : null
  if (head === 'tile') {
    if (!sub) return null
    const props = tileProps({ props: url.searchParams.get('props') ?? undefined, stash: url.searchParams.get('stash') ?? undefined })
    return { kind: 'tile', tile: decodeURIComponent(id), session: decodeURIComponent(sub), props }
  }
  if (head === 'web') return { kind: 'web', id: decodeURIComponent(id), url: url.searchParams.get('url') ?? '', title: url.searchParams.get('title') ?? '' }
  if (head === 'file') {
    const path = url.searchParams.get('path')
    if (!path) return null
    const line = lineParam(url.searchParams.get('line')), end = lineParam(url.searchParams.get('end_line'))
    return { kind: 'file', session: decodeURIComponent(id), path, ...(line === undefined ? {} : { line }), ...(end === undefined ? {} : { end_line: end }) }
  }
  if (head === 'diff') {
    const spec = specParam(url.searchParams.get('spec'))
    return spec ? { kind: 'diff', session: decodeURIComponent(id), spec } : null
  }
  if (head === 'c') return { kind: 'chat', session: decodeURIComponent(id) }
  if (head === 'talk') return { kind: 'talk', session: decodeURIComponent(id) }
  if (head === 'vm') return { kind: 'vm', name: decodeURIComponent(id) }
  if (head === 'artifacts') return { kind: 'artifact', callId: decodeURIComponent(id) }
  if (head === 'notes') return { kind: 'note', doc: decodeURIComponent(id) }
  if (head === 'notebooks') return { kind: 'notebook', notebook: decodeURIComponent(id) }
  if (head === 'meetings') {
    const meeting = decodeURIComponent(id)
    return url.searchParams.get('pane') === 'note' ? { kind: 'meeting-note', meeting } : { kind: 'meeting', meeting }
  }
  return null
}

/** Read once: the route may drop the query on its first navigation. */
export const POPOUT: PopoutTarget | null = typeof location === 'undefined' ? null : popoutTarget(location.href)

export const isPopout = (): boolean => POPOUT !== null

/**
 * The window app route `href` opens in from a popped window, which has no pane workspace to open it into and must not
 * navigate itself away from what it shows: its subject's own window where the route has one (a chat, a note, …), else
 * a page window on the route. `/` and `/new` are a New chat window. The hash is dropped (`popoutUrl` appends
 * `?window=` to the path, so a hash before it would swallow the label).
 */
export function routeWindow(href: string): PopoutTarget {
  const path = href.split('#')[0] ?? href
  const [pathname = '/'] = path.split('?')
  const [, head, raw, sub] = pathname.split('/')
  const id = raw ? decodeURIComponent(raw) : ''
  if (pathname === '/' || head === 'new') return { kind: 'page', path: head === 'new' ? path : '/new' }
  if (id && head === 'c') return { kind: 'chat', session: id }
  if (id && head === 'talk') return { kind: 'talk', session: id }
  if (id && head === 'notes') return { kind: 'note', doc: id }
  if (id && head === 'notebooks') return { kind: 'notebook', notebook: id }
  if (id && head === 'meetings' && sub === undefined) return { kind: 'meeting', meeting: id }
  if (id && head === 'artifacts') return { kind: 'artifact', callId: id }
  if (id && head === 'vm') return { kind: 'vm', name: id }
  if (id && head === 'terminal' && sub) {
    const name = new URL(path, 'http://x').searchParams.get('name') || undefined
    return { kind: 'node-terminal', engine: id, shell: decodeURIComponent(sub), ...(name ? { name } : {}) }
  }
  return { kind: 'page', path }
}

/**
 * A window that shows one thing alone (a chat, artifact, note, notebook, meeting, …): its own title bar, no section band,
 * closed when its subject is deleted. A sidebar page window (`page`, e.g. Library) is not one — it is the shell's column
 * without the rail, navigable between pages, so the pages render there exactly as in the main window.
 */
export const soloPopout = (t: PopoutTarget | null): boolean => t !== null && t.kind !== 'page'

export const isSoloPopout = (): boolean => soloPopout(POPOUT)

/**
 * Whether `/artifacts` renders its Explorer (rail + viewer outlet) in window `t`: everywhere but an artifact window,
 * which shows its viewer alone. The Library page window lands on `/artifacts` with no `$callId`, so a bare outlet
 * there renders nothing at all (the black Library window).
 */
export const artifactsRail = (t: PopoutTarget | null): boolean => t?.kind !== 'artifact'

/** The meeting a popped window is about (`meeting` or `meeting-note`), else `null`: a `Deleted` event for it closes the window. */
export const popoutMeeting = (t: PopoutTarget | null): string | null =>
  t !== null && (t.kind === 'meeting' || t.kind === 'meeting-note') ? t.meeting : null

/** The session a popped window belongs to (`chat`, or the PTY's, map's, file's, diff's or tile's owning session; a draft's
 * tile has none yet), else `null`:
 * `SessionDeleted` for it closes the window. */
export const popoutSession = (t: PopoutTarget | null): string | null =>
  t !== null && (t.kind === 'chat' || t.kind === 'talk' || t.kind === 'pty' || t.kind === 'codemap' || t.kind === 'file' || t.kind === 'diff' || (t.kind === 'tile' && !t.session.startsWith('draft:'))) ? t.session : null

/**
 * Whether this container can open a second window at all: a desktop shell (`open_window`) or a plain browser
 * (`window.open`). The iOS shell has one webview and no window manager, so every pop-out affordance — buttons, menu
 * rows, ⌥-click, "in new window" text — is hidden there. A narrow layout is not the criterion: a narrow desktop
 * window still pops out.
 */
export const canPopout = (): boolean => container() !== 'ios'

/** 144 wave 2: a notebook's three columns want a landscape window (~1100 × 720), clamped to the screen; the Tauri
 * host sizes by label in `crates/desktop/src/commands.rs::open_window`, a browser takes `window.open` features. */
export function popoutSize(t: PopoutTarget, screen: { availWidth: number; availHeight: number }): { width: number; height: number } | null {
  if (t.kind !== 'notebook') return null
  return { width: Math.min(1100, screen.availWidth), height: Math.min(720, screen.availHeight) }
}

/** The title `t`'s window opens with, before its page sets `document.title` (the host appends " — theuth" to both, and
 * keeps this one while the page's title is still empty): the subject when the target names it, else its kind. */
export function popoutTitle(t: PopoutTarget): string {
  switch (t.kind) {
    case 'chat': return 'Chat'
    case 'artifact': return 'Artifact'
    case 'note': return 'Note'
    case 'notebook': return 'Notebook'
    case 'meeting': return 'Meeting'
    case 'meeting-note': return 'Meeting note'
    case 'pty': return 'Terminal'
    case 'node-terminal': return t.name?.trim() || 'Terminal'
    case 'codemap': return 'CodeMap'
    case 'talk': return 'Talk'
    case 'vm': return t.name
    case 'page': return t.path.split(/[?#]/)[0] === '/new' ? 'New chat' : viewTitle(t.path)
    case 'web': return t.title.trim() || 'Web'
    case 'file': return t.path.split(/[\\/]/).filter(Boolean).pop() ?? 'File'
    case 'diff': return 'Diff'
    case 'tile': return t.tile.charAt(0).toUpperCase() + t.tile.slice(1)
  }
}

/** Open `t` in its own window (or focus it); a browser gets a named `window.open`. */
export function openPopout(t: PopoutTarget): Promise<void> {
  stashTileProps(t)
  if (t.kind === 'node-terminal') registerNodeTerminalPopout(popoutLabel(t), { engine: t.engine, shell: t.shell })
  if (isTauri()) return invoke<void>('open_window', { label: popoutLabel(t), url: popoutUrl(t), title: popoutTitle(t) })
  const size = popoutSize(t, window.screen)
  if (size) window.open(popoutUrl(t), popoutLabel(t), `width=${size.width},height=${size.height}`)
  else window.open(popoutUrl(t), popoutLabel(t))
  return Promise.resolve()
}

/** `t` is open in its own window right now (desktop shell only; a browser cannot tell). */
export async function popoutOpen(t: PopoutTarget): Promise<boolean> {
  return isTauri() && (await WebviewWindow.getByLabel(popoutLabel(t))) !== null
}

/** Close this popped window: what it showed is gone (its chat, meeting or note was deleted). */
export function closePopout(): void {
  if (isTauri()) void getCurrentWindow().close()
  else window.close()
}
