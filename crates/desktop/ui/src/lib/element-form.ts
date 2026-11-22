// 209 §4.10, D13: one element of a page (typically a sign-in form) lifted into a transcript card. The engine sends a
// sanitised static clone (`ElementFormRequested`); the card frames it in the sandboxed inline-page frame of 120 with a
// document this module builds: the CSP meta first, then the app's own bootstrap, then the clone, re-sanitised here
// (the client trusts nothing in it). The user types into the frame; Submit asks the bootstrap for the values by
// `data-theuth-field` index (`theuth/collect`) and hands them straight to `answer_element_form`. Values live only in
// that call's arguments: never in a store, a log, the transcript or an error message.

import { create } from 'zustand'
import type { ElementFormAnswer, ElementFormRequested, ElementFormResolved } from '@/lib/element-form-wire'
import { useEngines } from '@/link/engines'
import type { Api } from '@/rpc/bindings/client'
import { ready, supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

export type { ElementFormRequested, ElementFormResolved }
export type ElementFormField = ElementFormRequested['fields'][number]
export type Outcome = ElementFormResolved['outcome']

/** The document's whole network policy (§4.10): `theuth-page`'s header carries only `sandbox …`, no `default-src`. */
export const FORM_CSP =
  "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; form-action 'none'; base-uri 'none'"
export const CSP_META = `<meta http-equiv="Content-Security-Policy" content="${FORM_CSP}">`

/** host → frame `{ type, nonce }`; frame → host `{ type, nonce, values: { "<index>": value } }`. */
export const COLLECT = 'theuth/collect'
/** frame → host `{ type }`: Enter in a field, or a submit button of the clone pressed. */
export const SUBMIT_REQUEST = 'theuth/submit-request'
/** frame → host `{ type, height }`: the document's height in CSS px. */
export const SIZE = 'theuth/size'

/** The frame's height bounds (CSS px). */
export const MIN_FRAME = 80
export const MAX_FRAME = 900

/** Kinds that are buttons: never collected, never sent. */
const BUTTON_KINDS = new Set(['submit', 'button', 'reset', 'image'])
export const isButton = (f: Pick<ElementFormField, 'kind'>): boolean => BUTTON_KINDS.has(f.kind.toLowerCase())

/**
 * The bootstrap: answers `theuth/collect` from its parent with every control's value by `data-theuth-field` index
 * (checkbox and radio as `"true"` / `"false"`, buttons skipped), turns Enter in a field and any submit of the clone into
 * `theuth/submit-request` (the real submit is blocked anyway, `form-action 'none'`), and reports the height. It never
 * logs and keeps nothing; the values exist in the page and in the one reply.
 */
export function formBootstrap(): string {
  return `(function () {
var post = function (msg) { try { window.parent.postMessage(msg, '*'); } catch (e) {} };
var SKIP = { submit: 1, button: 1, reset: 1, image: 1, file: 1, hidden: 1 };
var collect = function () {
  var out = {};
  var els = document.querySelectorAll('[data-theuth-field]');
  for (var i = 0; i < els.length; i++) {
    var el = els[i], n = el.getAttribute('data-theuth-field');
    if (!/^[0-9]+$/.test(n || '')) continue;
    var tag = el.tagName;
    if (tag === 'INPUT') {
      var t = (el.getAttribute('type') || 'text').toLowerCase();
      if (SKIP[t]) continue;
      out[n] = t === 'checkbox' || t === 'radio' ? (el.checked ? 'true' : 'false') : String(el.value);
    } else if (tag === 'SELECT' || tag === 'TEXTAREA') out[n] = String(el.value);
  }
  return out;
};
window.addEventListener('message', function (ev) {
  if (ev.source !== window.parent) return;
  var d = ev.data;
  if (!d || d.type !== '${COLLECT}' || typeof d.nonce !== 'string') return;
  post({ type: '${COLLECT}', nonce: d.nonce, values: collect() });
});
var ask = function (ev) { ev.preventDefault(); post({ type: '${SUBMIT_REQUEST}' }); };
document.addEventListener('submit', ask, true);
document.addEventListener('keydown', function (ev) {
  var el = ev.target;
  if (ev.key !== 'Enter' || ev.isComposing || !el || el.tagName !== 'INPUT') return;
  var t = (el.getAttribute('type') || 'text').toLowerCase();
  if (t === 'checkbox' || t === 'radio' || SKIP[t]) return;
  ask(ev);
}, true);
document.addEventListener('click', function (ev) {
  var el = ev.target && ev.target.closest ? ev.target.closest('button, input[type=submit], input[type=image]') : null;
  if (!el) return;
  var t = (el.getAttribute('type') || (el.tagName === 'BUTTON' ? 'submit' : '')).toLowerCase();
  if (t === 'submit' || t === 'image') ask(ev); else ev.preventDefault();
}, true);
var size = function () { post({ type: '${SIZE}', height: Math.ceil(document.documentElement.scrollHeight) }); };
if (typeof ResizeObserver === 'function') new ResizeObserver(size).observe(document.documentElement);
window.addEventListener('load', size);
})();`
}

/** Tags of forms and text (§4.10's allowlist); everything else is unwrapped, or dropped with its content when listed in `DROP`. */
const TAGS = new Set(
  (
    'form fieldset legend label input select option optgroup textarea button div span p a h1 h2 h3 h4 h5 h6 strong em b i u s ' +
    'small mark sub sup br hr ul ol li dl dt dd table caption thead tbody tfoot tr th td section header footer main nav ' +
    'article aside figure figcaption blockquote pre code abbr img'
  ).split(' '),
)
const DROP = new Set(
  (
    'script noscript style link meta base iframe frame frameset object embed svg math template title head video audio ' +
    'source track picture canvas portal applet param dialog'
  ).split(' '),
)
const ATTRS = new Set(['style', 'type', 'name', 'placeholder', 'autocomplete', 'checked', 'selected', 'disabled', 'for', 'id', 'class', 'data-theuth-field'])
/** `url(...)` that is not an inline image. */
const URL_IN_STYLE = /url\(\s*(?!['"]?data:image\/)[^)]*\)/gi
/** Input types that never belong in the card: hidden ones stay in the real form, files cannot be carried. */
const NO_INPUT = new Set(['hidden', 'file'])

/**
 * The clone re-sanitised by the same allowlist as the engine's (§4.10): no script, style sheet, frame, `meta`, `base`,
 * `on*`, link or network URL survives; `img` keeps only a `data:image/` source; a field the event marks `masked` is
 * `type=password` whatever the clone says. Parsed inert (`DOMParser`: nothing runs or loads).
 */
export function sanitizeSnapshot(html: string, masked: ReadonlySet<string> = new Set()): string {
  const doc = new DOMParser().parseFromString(`<!doctype html><body>${html}`, 'text/html')
  const clean = (el: Element): void => {
    for (const child of [...el.children]) {
      const tag = child.localName
      if (DROP.has(tag) || child.namespaceURI !== 'http://www.w3.org/1999/xhtml') {
        child.remove()
        continue
      }
      if (!TAGS.has(tag)) {
        clean(child)
        child.replaceWith(...child.childNodes)
        continue
      }
      for (const attr of [...child.attributes]) {
        const name = attr.name.toLowerCase()
        const keep = ATTRS.has(name) || (tag === 'img' && name === 'src' && /^data:image\//i.test(attr.value.trim()))
        if (!keep) child.removeAttribute(attr.name)
      }
      const style = child.getAttribute('style')
      if (style !== null) child.setAttribute('style', style.replace(URL_IN_STYLE, 'none'))
      if (tag === 'input') {
        const type = (child.getAttribute('type') ?? '').toLowerCase()
        if (NO_INPUT.has(type)) {
          child.remove()
          continue
        }
        const n = child.getAttribute('data-theuth-field')
        if (n !== null && masked.has(n)) child.setAttribute('type', 'password')
      }
      clean(child)
    }
    // Comments and processing leftovers carry nothing the card needs.
    for (const node of [...el.childNodes]) if (node.nodeType === 8) node.remove()
  }
  clean(doc.body)
  return doc.body.innerHTML
}

/** The frame's base look: the clone carries the site's computed styles; this only frames them. */
const FORM_CSS = 'html{color-scheme:light}body{margin:8px;background:#fff;color:#111;font:14px system-ui,sans-serif;overflow-wrap:anywhere}'

/** The framed document: the CSP meta is the first thing in it, before anything the clone could put up front. */
export function formDocument(html: string, fields: readonly Pick<ElementFormField, 'index' | 'masked'>[]): string {
  const masked = new Set(fields.filter((f) => f.masked).map((f) => String(f.index)))
  return (
    `<!doctype html><html><head>${CSP_META}<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">` +
    `<style>${FORM_CSS}</style><script>${formBootstrap()}</script></head><body>${sanitizeSnapshot(html, masked)}</body></html>`
  )
}

/** One frame → host message, or `null` for anything else. */
export type FrameMessage =
  | { type: 'collect'; nonce: string; values: unknown }
  | { type: 'submit-request' }
  | { type: 'size'; height: number }
export function readFrameMessage(data: unknown): FrameMessage | null {
  if (!data || typeof data !== 'object') return null
  const d = data as { type?: unknown; nonce?: unknown; values?: unknown; height?: unknown }
  if (d.type === COLLECT && typeof d.nonce === 'string') return { type: 'collect', nonce: d.nonce, values: d.values }
  if (d.type === SUBMIT_REQUEST) return { type: 'submit-request' }
  if (d.type === SIZE && typeof d.height === 'number' && Number.isFinite(d.height) && d.height > 0) return { type: 'size', height: d.height }
  return null
}

/** The values of the event's non-button fields, by index: a string each (missing → `""`); anything else the frame sent is dropped. */
export function pickValues(fields: readonly ElementFormField[], raw: unknown): Record<string, string> {
  const got = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const out: Record<string, string> = {}
  for (const f of fields) {
    if (isButton(f)) continue
    const key = String(f.index)
    const v = Object.hasOwn(got, key) ? got[key] : undefined
    out[key] = typeof v === 'string' ? v : ''
  }
  return out
}

/** How long Submit waits for the frame's values. */
export const COLLECT_MS = 3_000

/**
 * Ask the frame `win` for its values and resolve with them by index (`pickValues`); `listen` subscribes to the host
 * window's `message` events. Only a reply from `win` carrying this nonce counts. Rejects (without values) on timeout.
 */
export function collectValues(
  win: { postMessage(msg: unknown, target: string): void },
  fields: readonly ElementFormField[],
  listen: (cb: (ev: { source: unknown; data: unknown }) => void) => () => void,
  nonce: string = crypto.randomUUID(),
  ms: number = COLLECT_MS,
): Promise<Record<string, string>> {
  const { promise, resolve, reject } = Promise.withResolvers<Record<string, string>>()
  const timer = setTimeout(() => {
    off()
    reject(new Error('the form did not answer'))
  }, ms)
  const off = listen((ev) => {
    if (ev.source !== win) return
    const m = readFrameMessage(ev.data)
    if (m?.type !== 'collect' || m.nonce !== nonce) return
    clearTimeout(timer)
    off()
    resolve(pickValues(fields, m.values))
  })
  win.postMessage({ type: COLLECT, nonce }, '*')
  return promise
}

/** The app's own reading of the page origin: `scheme://host[:port]` with the host in ASCII (an IDN as `xn--…`, so a
 * homograph shows), never the snapshot's word for it. `null` for an origin that is not http(s) (an opaque `null`). */
export function originLabel(origin: string): { text: string; host: string; secure: boolean } | null {
  let u: URL
  try {
    u = new URL(origin)
  } catch {
    return null
  }
  if (u.protocol !== 'https:' && u.protocol !== 'http:') return null
  return { text: `${u.protocol}//${u.host}`, host: u.host, secure: u.protocol === 'https:' }
}

/** Which browser holds the page: the user's tile or an agent browser of an engine. */
export function sourceLabel(source: string): string {
  if (source.startsWith('web:')) return 'your web view'
  if (source.startsWith('obscura:')) return `agent browser on ${source.slice('obscura:'.length) || 'this engine'}`
  return source
}

/** The outcome read back from the tool's receipt (§4.10) once the event is gone (a reload): `null` when it says none. */
export function outcomeOfReceipt(output: string): Outcome | null {
  if (output.startsWith('submitted')) return 'submitted'
  if (output.startsWith('cancelled')) return 'cancelled'
  if (output.startsWith('the page changed')) return 'stale'
  if (output.startsWith('no answer')) return 'timeout'
  return null
}

/** A resolved card: how it ended and the origin it was shown for (from its event, when this window saw it). */
export type Resolved = { outcome: Outcome; origin: string | null }

/** Pending cards (no values, ever) and the outcomes of resolved ones, by request id; `calls` maps a tool call to its request. */
type ElementFormsStore = {
  pending: Record<string, ElementFormRequested>
  outcomes: Record<string, Resolved>
  calls: Record<string, string>
  add(e: ElementFormRequested): void
  resolve(id: string, outcome: Outcome): void
  /** The engine's `element_forms_pending`: afterwards the pending set is exactly that, for `session` when given (its
   * cards only), else for every session but those in `keep` (sessions loaded on their own: owned by another engine). */
  replace(list: ElementFormRequested[], scope: { session: string } | { keep: ReadonlySet<string> }): void
  clear(): void
}

export const useElementForms = create<ElementFormsStore>()((set) => ({
  pending: {},
  outcomes: {},
  calls: {},
  add: (e) =>
    set((s) => (s.outcomes[e.id] || s.pending[e.id] ? s : { pending: { ...s.pending, [e.id]: e }, calls: { ...s.calls, [e.call_id]: e.id } })),
  resolve: (id, outcome) =>
    set((s) => {
      const { [id]: form, ...pending } = s.pending
      return { pending, outcomes: { ...s.outcomes, [id]: { outcome, origin: form?.origin ?? null } } }
    }),
  replace: (list, scope) =>
    set((s) => {
      const kept = (e: ElementFormRequested) => ('session' in scope ? e.session !== scope.session : scope.keep.has(e.session))
      const pending: Record<string, ElementFormRequested> = {}
      for (const e of Object.values(s.pending)) if (kept(e)) pending[e.id] = e
      const calls = { ...s.calls }
      for (const e of list) {
        if (s.outcomes[e.id] || e.type !== 'ElementFormRequested') continue
        // A card already on screen keeps its object: its frame (and what the user typed) stays.
        pending[e.id] = s.pending[e.id] ?? e
        calls[e.call_id] = e.id
      }
      return { pending, calls }
    }),
  clear: () => set({ pending: {}, outcomes: {}, calls: {} }),
}))

const METHODS = { answer: 'answer_element_form', pending: 'element_forms_pending' } as const
const REQUESTED_TOPIC = 'browser.element-form-requested@1'

/** Whether the connected engine takes answers (an older one has no card to answer). */
export function useCanAnswer(): boolean {
  return useConnection((s) => !!s.api && supports(s.discovery, { methods: [METHODS.answer], topics: [REQUESTED_TOPIC] }))
}

/** The connection's API, or a thrown error the card shows. */
function api(): Api {
  const a = useConnection.getState().api
  if (!a) throw new Error('not connected')
  return a
}

/** Submit: `values` go only into this call's arguments. `session` is the request's: an engine that did not issue the
 * form forwards the answer to the session's owner. `true` when the form was still pending and this answer was taken. */
export function answerElementForm(id: string, values: Record<string, string>, session: string): Promise<boolean> {
  const answer: ElementFormAnswer = { submit: { values } }
  return api().answer_element_form(id, answer, session)
}

/** Cancel: nothing is typed into the page. */
export function cancelElementForm(id: string, session: string): Promise<boolean> {
  const answer: ElementFormAnswer = { cancel: {} }
  return api().answer_element_form(id, answer, session)
}

/** Sessions whose pending cards were asked for by name (a transcript of a session another engine owns), per connection. */
let asked = new Set<string>()
let askedOf: Api | null = null

/** Load the engine's pending cards (`session`: that session's, which the engine forwards to its owner) and put them in
 * the store, leaving the cards of the `keep` sessions alone on a whole load; `false` when unsupported or the call
 * failed (the store is left as it is). */
export async function loadPending(
  a: Api,
  discovery: Parameters<typeof supports>[0],
  session?: string,
  keep: ReadonlySet<string> = asked,
): Promise<boolean> {
  if (!ready(discovery) || !supports(discovery, { methods: [METHODS.pending] })) return false
  const list = await (session === undefined ? a.element_forms_pending() : a.element_forms_pending(session)).catch(() => null)
  if (!list) return false
  useElementForms.getState().replace(list, session === undefined ? { keep: new Set(keep) } : { session })
  return true
}

/** A card whose form this window has not seen (a late attach, a remote-owned session): ask for its session's cards,
 * once per connection. */
export function attachSession(session: string): void {
  const { api: a, state, discovery } = useConnection.getState()
  if (!a || state !== 'open') return
  if (askedOf !== a) {
    asked = new Set()
    askedOf = a
  }
  if (asked.has(session)) return
  asked.add(session)
  void loadPending(a, discovery, session)
}

/**
 * Listen for cards in this window, and ask the engine for the pending ones on every connect, reconnect and engine
 * switch (a client that attached late, or missed events while away, still shows the card); sessions asked for by name
 * are asked again. Returns the stop.
 */
export function startElementForms(): () => void {
  let loadedFrom: Api | null = null
  let engine = useEngines.getState().active
  const onConnection = () => {
    const { api: a, state, discovery } = useConnection.getState()
    if (state !== 'open') loadedFrom = null
    if (!a || state !== 'open' || !ready(discovery) || a === loadedFrom) return
    loadedFrom = a
    // Every (re)connect: the whole engine's cards, then each session asked for by name again (it may have resolved meanwhile).
    const again = [...asked]
    asked = new Set()
    askedOf = a
    void loadPending(a, discovery, undefined, new Set(again)).then(() => {
      for (const s of again) attachSession(s)
    })
  }
  let offConnection = useConnection.subscribe(onConnection)
  const offs = [
    on('browser.element-form-requested@1', 'ElementFormRequested', (e) => useElementForms.getState().add(e)),
    on('browser.element-form-resolved@1', 'ElementFormResolved', (e) => useElementForms.getState().resolve(e.id, e.outcome)),
    useEngines.subscribe((s) => {
      if (s.active === engine) return
      engine = s.active
      useElementForms.getState().clear()
      asked = new Set()
      askedOf = null
      loadedFrom = null
      offConnection()
      offConnection = useConnection.subscribe(onConnection)
      onConnection()
    }),
    () => offConnection(),
  ]
  onConnection()
  return () => {
    for (const off of offs) off()
  }
}
