// 209 §4.10, D13: one element of a page (typically a sign-in form) lifted into a transcript card. The engine sends a
// sanitised static clone (`ElementFormRequested`); the card frames it in the sandboxed inline-page frame of 120 with a
// document this module builds: the CSP meta first, then the app's own bootstrap, then the clone, re-sanitised here
// (the client trusts nothing in it). The user types into the frame; Submit asks the bootstrap for the values of the
// controls the user changed, by `data-theuth-field` index (`theuth/collect`), and hands them straight to
// `answer_element_form`. Values live only in that call's arguments: never in a store, a log, the transcript or an error
// message. They do cross the app window's `message` bus once (the frame's reply, posted to `'*'`: an opaque-origin
// frame has no origin to name): `collectValues` is the only consumer, matching the frame's window and a fresh nonce;
// no other `message` listener of the app may log `ev.data`.

import type { ElementFormAnswer, ElementFormRequested } from '@/lib/element-form-wire'
import { useEngines } from '@/link/engines'
import type { Api } from '@/rpc/bindings/client'
import { type Discovery, ready, supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import type { FormOutcome } from '@/store/element-forms'
import { active } from '@/store/engine-scope'

export type { ElementFormRequested }
export type ElementFormField = ElementFormRequested['fields'][number]

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
 * The bootstrap: answers `theuth/collect` from its parent with, by `data-theuth-field` index, the value of every
 * control the user changed (an `input` or `change` event fired on it) and the state of every checkbox and radio
 * (`"true"` / `"false"`); an untouched control is left out, so the page keeps what it had there (ruling B). Buttons
 * are skipped. It turns Enter in a field and any submit of the clone into `theuth/submit-request` (the real submit is
 * blocked anyway, `form-action 'none'`), and reports the height. It never logs and keeps nothing; the values exist in
 * the page and in the one reply.
 */
export function formBootstrap(): string {
  return `(function () {
var post = function (msg) { try { window.parent.postMessage(msg, '*'); } catch (e) {} };
var SKIP = { submit: 1, button: 1, reset: 1, image: 1, file: 1, hidden: 1 };
var touched = {};
var mark = function (ev) {
  var el = ev.target && ev.target.closest ? ev.target.closest('[data-theuth-field]') : null;
  if (el) touched[el.getAttribute('data-theuth-field')] = 1;
};
document.addEventListener('input', mark, true);
document.addEventListener('change', mark, true);
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
      if (t === 'checkbox' || t === 'radio') out[n] = el.checked ? 'true' : 'false';
      else if (touched[n]) out[n] = String(el.value);
    } else if ((tag === 'SELECT' || tag === 'TEXTAREA') && touched[n]) out[n] = String(el.value);
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

const words = (s: string): string[] => s.trim().split(/\s+/)
/** Tags of forms and text (§4.10's allowlist, the engine's `show_element.allow.json` `tags`); everything else is
 * unwrapped, or dropped with its content when listed in `DROP_TAGS`. */
export const ALLOWED_TAGS: ReadonlySet<string> = new Set(
  words(
    'form fieldset legend label input select option optgroup textarea button output div span p a h1 h2 h3 h4 h5 h6 strong ' +
      'em b i u s q cite small mark sub sup br wbr hr ul ol li dl dt dd table caption colgroup col thead tbody tfoot tr th td ' +
      'section header footer main nav article aside address figure figcaption blockquote pre code abbr time img',
  ),
)
/** Tags dropped with everything inside them (the engine's `drop`). */
export const DROP_TAGS: ReadonlySet<string> = new Set(
  words(
    'script noscript style link meta base iframe frame frameset noframes object embed svg math template title head video ' +
      'audio source track picture canvas portal applet param dialog xmp plaintext noembed',
  ),
)
/** Attributes kept on any allowed tag (the engine's `attrs`); `img` also keeps `alt` and an inline-image `src`. */
export const ALLOWED_ATTRS: ReadonlySet<string> = new Set(
  words('style type name placeholder autocomplete checked selected disabled for id class data-theuth-field'),
)
/** `url(...)` that is not an inline image. */
const URL_IN_STYLE = /url\(\s*(?!['"]?data:image\/)[^)]*\)/gi
/** Input types that never belong in the card: hidden ones stay in the real form, files cannot be carried. */
const NO_INPUT = new Set(['hidden', 'file'])

/**
 * The clone re-sanitised by the same allowlist as the engine's (§4.10): no script, style sheet, frame, `meta`, `base`,
 * `on*`, link or network URL survives; `img` keeps its `alt` and only a `data:image/` source; a field the event marks
 * `masked` is `type=password` whatever the clone says. Parsed inert (`DOMParser`: nothing runs or loads).
 */
export function sanitizeSnapshot(html: string, masked: ReadonlySet<string> = new Set()): string {
  const doc = new DOMParser().parseFromString(`<!doctype html><body>${html}`, 'text/html')
  const clean = (el: Element): void => {
    for (const child of [...el.children]) {
      const tag = child.localName
      if (DROP_TAGS.has(tag) || child.namespaceURI !== 'http://www.w3.org/1999/xhtml') {
        child.remove()
        continue
      }
      if (!ALLOWED_TAGS.has(tag)) {
        clean(child)
        child.replaceWith(...child.childNodes)
        continue
      }
      for (const attr of [...child.attributes]) {
        const name = attr.name.toLowerCase()
        const keep =
          ALLOWED_ATTRS.has(name) ||
          (tag === 'img' && name === 'alt') ||
          (tag === 'img' && name === 'src' && /^data:image\//i.test(attr.value.trim()))
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

/** The string values the frame sent for the event's non-button fields, by index; an index the frame left out (the user
 * did not touch it) stays out, and anything else the frame sent is dropped. */
export function pickValues(fields: readonly ElementFormField[], raw: unknown): Record<string, string> {
  const got = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const out: Record<string, string> = {}
  for (const f of fields) {
    if (isButton(f)) continue
    const key = String(f.index)
    const v = Object.hasOwn(got, key) ? got[key] : undefined
    if (typeof v === 'string') out[key] = v
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

/** The app's own reading of an origin: `scheme://host[:port]` with the host in ASCII (an IDN as `xn--…`, so a homograph
 * shows), never the snapshot's word for it; `head` + `tail` = `text`, where `tail` is the part a phishing suffix
 * cannot hide (the host's last two labels and the port, or the whole host for an IP address), drawn untruncated. */
export type OriginLabel = { text: string; host: string; secure: boolean; head: string; tail: string }

/** `origin` as the app shows it; `null` for an origin that is not http(s) (an opaque `null`). */
export function originLabel(origin: string): OriginLabel | null {
  let u: URL
  try {
    u = new URL(origin)
  } catch {
    return null
  }
  if (u.protocol !== 'https:' && u.protocol !== 'http:') return null
  const text = `${u.protocol}//${u.host}`
  const name = u.hostname
  const ip = name.startsWith('[') || /^[0-9.]+$/.test(name)
  const labels = name.split('.')
  const tail = (ip || labels.length <= 2 ? name : labels.slice(-2).join('.')) + (u.port ? `:${u.port}` : '')
  return { text, host: u.host, secure: u.protocol === 'https:', head: text.slice(0, text.length - tail.length), tail }
}

/** Which browser holds the page: the user's tile or an agent browser of an engine. */
export function sourceLabel(source: string): string {
  if (source.startsWith('web:')) return 'your web view'
  if (source.startsWith('obscura:')) return `agent browser on ${source.slice('obscura:'.length) || 'this engine'}`
  return source
}

/** The outcome read back from the tool's receipt (§4.10) once the event is gone (a reload), from its first line:
 * `null` when it says none. */
export function outcomeOfReceipt(output: string): FormOutcome | null {
  const line = output.trimStart().split('\n', 1)[0] ?? ''
  if (line.startsWith('submitted')) return 'submitted'
  if (line.startsWith('cancelled') || line.startsWith('no app showed the form')) return 'cancelled'
  if (line.startsWith('the page changed')) return 'stale'
  if (line.startsWith('no answer')) return 'timeout'
  return null
}

const METHODS = { answer: 'answer_element_form', pending: 'element_forms_pending', shown: 'element_form_shown' } as const
const REQUESTED_TOPIC = 'browser.element-form-requested@1'

/** Whether the card's engine takes answers (an older one has no card to answer). */
export function useCanAnswer(): boolean {
  return useConnection((s) => !!s.api && supports(s.discovery, { methods: [METHODS.answer], topics: [REQUESTED_TOPIC] }))
}

/** Submit on `api` (the card's own engine): `values` go only into this call's arguments. `session` is the request's: an
 * engine that did not issue the form forwards the answer to the session's owner. `true` when the form was still pending
 * and this answer was taken. */
export function answerElementForm(api: Api, id: string, values: Record<string, string>, session: string): Promise<boolean> {
  const answer: ElementFormAnswer = { submit: { values } }
  return api.answer_element_form(id, answer, session)
}

/** Cancel: nothing is typed into the page. */
export function cancelElementForm(api: Api, id: string, session: string): Promise<boolean> {
  const answer: ElementFormAnswer = { cancel: {} }
  return api.answer_element_form(id, answer, session)
}

/** Cards each connection was told are on screen. */
const shownOn = new WeakMap<Api, Set<string>>()

/** Ruling D: tell the engine a card is on screen (`element_form_shown`), once per card per connection; an engine
 * whose cards no app shows gives up on them. A failed call is tried again on the next render. */
export function markShown(api: Api, discovery: Discovery, form: Pick<ElementFormRequested, 'id' | 'session'>): void {
  if (!supports(discovery, { methods: [METHODS.shown] })) return
  let ids = shownOn.get(api)
  if (!ids) shownOn.set(api, (ids = new Set()))
  if (ids.has(form.id)) return
  ids.add(form.id)
  api.element_form_shown(form.id, form.session).catch(() => ids.delete(form.id))
}

/** How many sessions `asked` remembers: the oldest is forgotten (asked again if its card is still missing). */
const MAX_ASKED = 64

/** Sessions whose pending cards were asked for by name (a transcript of a session another engine owns), per connection. */
let asked = new Set<string>()
let askedOf: Api | null = null

/** Load the engine's pending cards (`session`: that session's, which the engine forwards to its owner) into the active
 * engine's store, leaving the cards of the `keep` sessions alone on a whole load; `false` when unsupported or the call
 * failed (the store is left as it is). */
export async function loadPending(a: Api, discovery: Discovery, session?: string, keep: ReadonlySet<string> = asked): Promise<boolean> {
  if (!ready(discovery) || !supports(discovery, { methods: [METHODS.pending] })) return false
  const store = active().elementForms
  const list = await (session === undefined ? a.element_forms_pending() : a.element_forms_pending(session)).catch(() => null)
  if (!list) return false
  store.getState().replace(list, session === undefined ? { keep: new Set(keep) } : { session })
  return true
}

/** A card whose form this window has not seen (a late attach, a remote-owned session): ask for its session's cards,
 * once per connection while that answer stands (a failed ask is forgotten, so the next render asks again). */
export function attachSession(session: string): void {
  const { api: a, state, discovery } = useConnection.getState()
  if (!a || state !== 'open') return
  if (askedOf !== a) {
    asked = new Set()
    askedOf = a
  }
  if (asked.has(session)) return
  asked.add(session)
  if (asked.size > MAX_ASKED) asked.delete(asked.values().next().value as string)
  const of = asked
  void loadPending(a, discovery, session).then((ok) => {
    if (!ok) of.delete(session)
  })
}

/** Ask a session's cards again now (the card waited long after its answer was taken: a lost `ElementFormResolved`). */
export function reloadSession(session: string): void {
  const { api: a, state, discovery } = useConnection.getState()
  if (a && state === 'open') void loadPending(a, discovery, session)
}

/**
 * Ask the engine for the pending cards on every connect, reconnect and engine switch (a client that attached late, or
 * missed events while away, still shows the card); sessions asked for by name are asked again. The events themselves
 * reach each engine's store through `dispatch`. Returns the stop.
 */
export function startElementForms(): () => void {
  let loadedFrom: Api | null = null
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
  let engine = useEngines.getState().active
  // Each engine keeps its own cards (`Stores.elementForms`); a switch re-binds to the new engine's connection and loads.
  const offEngine = useEngines.subscribe((s) => {
    if (s.active === engine) return
    engine = s.active
    asked = new Set()
    askedOf = null
    loadedFrom = null
    offConnection()
    offConnection = useConnection.subscribe(onConnection)
    onConnection()
  })
  onConnection()
  return () => {
    offConnection()
    offEngine()
  }
}
