/**
 * Strict iframe sandbox attribute for Deck HTML sections:
 * - NO `allow-same-origin` (opaque origin, cannot touch host DOM/storage/cookies)
 * - NO `allow-forms` (no form exfiltration)
 * - NO `allow-popups` / `allow-top-navigation`
 */
export const DECK_HTML_SANDBOX = 'allow-scripts'

/**
 * Strict CSP enforced both via `theuth-page` HTTP header (`profile: "deck"`)
 * AND injected `<meta http-equiv="Content-Security-Policy">` (for non-Tauri `srcDoc` fallback
 * and defense-in-depth).
 *
 * Navigation boundary note:
 * - In Tauri desktop/iOS, Deck HTML sections are NEVER rendered with `srcDoc`; they load ONLY
 *   from `theuth-page` with the immutable `DeckStrict` HTTP CSP + `X-DNS-Prefetch-Control: off`.
 * - Outside Tauri (plain web/relay browser fallback), `srcDoc` enforces this `<meta>` CSP,
 *   strips `<base>`, `<meta http-equiv=...>`, and `<noscript>`, and intercepts link clicks and
 *   form submissions in capture phase. Top-level and popup navigations are blocked by the
 *   `sandbox="allow-scripts"` iframe boundary (though in-frame `location` mutations cannot be
 *   fully prevented by `<meta>` CSP alone without a server-enforced navigation policy).
 */
export const DECK_HTML_CSP =
  "default-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; media-src 'none'; img-src data:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'"

export const DECK_BRIDGE_CHANNEL = 'theuth-deck'

const MAX_ACTION_ARGS_DEPTH = 8
const MAX_ACTION_ARGS_NODES = 200
const MAX_ACTION_ARGS_BYTES = 16 * 1024
const FORBIDDEN_KEYS = new Set(['__proto__', 'constructor', 'prototype'])

export const GUEST_ACTION_WINDOW_MS = 2_000
export const GUEST_ACTION_MAX_PER_WINDOW = 5

/**
 * Narrow guest-to-host intent protocol for Deck HTML sections.
 * Runtime stream controls (`start`/`pause`/`resume`/`stop`/`run_once`) are strictly
 * host-controlled and NEVER accepted from unapproved guest HTML.
 */
export type DeckGuestIntent =
  | { kind: 'ready' }
  | { kind: 'action'; action: string; args: Record<string, unknown> }

/** Generate a cryptographically random 128-bit hex nonce (never falls back to `Math.random`). */
export function generateDeckNonce(): string {
  const c = typeof globalThis !== 'undefined' ? globalThis.crypto : undefined
  if (!c || typeof c.getRandomValues !== 'function') {
    throw new Error('Cryptographic RNG (crypto.getRandomValues) is required for Deck HTML nonces')
  }
  const bytes = new Uint8Array(16)
  c.getRandomValues(bytes)
  let hex = ''
  for (let i = 0; i < bytes.length; i++) {
    hex += bytes[i]!.toString(16).padStart(2, '0')
  }
  return hex
}

/**
 * Escape JSON safely for embedding inside `<script>` tags so no value (including a nonce or payload)
 * can break out with `</script>`, `<!--`, or JS line separators.
 */
export function serializeSafeJson(value: unknown): string {
  const json = JSON.stringify(value ?? null)
  if (json === undefined) return 'null'
  return json
    .replace(/</g, '\\u003c')
    .replace(/>/g, '\\u003e')
    .replace(/&/g, '\\u0026')
    .replace(/\u2028/g, '\\u2028')
    .replace(/\u2029/g, '\\u2029')
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return false
  const proto = Object.getPrototypeOf(value)
  return proto === Object.prototype || proto === null
}

type JsonBudget = { nodes: number; chars: number }

function clonePlainJsonValue(
  value: unknown,
  depth: number,
  ancestors: Set<object>,
  budget: JsonBudget,
): { ok: true; value: unknown } | { ok: false } {
  if (depth > MAX_ACTION_ARGS_DEPTH) return { ok: false }
  budget.nodes += 1
  if (budget.nodes > MAX_ACTION_ARGS_NODES) return { ok: false }

  if (value === null || typeof value === 'boolean') {
    budget.chars += 5
    return budget.chars <= MAX_ACTION_ARGS_BYTES ? { ok: true, value } : { ok: false }
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) return { ok: false }
    budget.chars += String(value).length + 1
    return budget.chars <= MAX_ACTION_ARGS_BYTES ? { ok: true, value } : { ok: false }
  }
  if (typeof value === 'string') {
    budget.chars += value.length + 2
    return budget.chars <= MAX_ACTION_ARGS_BYTES ? { ok: true, value } : { ok: false }
  }
  if (typeof value !== 'object') {
    // Reject bigint, symbol, function, undefined
    return { ok: false }
  }
  if (ancestors.has(value)) return { ok: false }

  if (Array.isArray(value)) {
    ancestors.add(value)
    const out: unknown[] = []
    for (let i = 0; i < value.length; i++) {
      if (!Object.prototype.hasOwnProperty.call(value, i)) {
        ancestors.delete(value)
        return { ok: false }
      }
      const item = clonePlainJsonValue(value[i], depth + 1, ancestors, budget)
      if (!item.ok) {
        ancestors.delete(value)
        return { ok: false }
      }
      out.push(item.value)
    }
    ancestors.delete(value)
    return { ok: true, value: out }
  }

  if (!isPlainObject(value)) {
    // Reject Date, Map, Set, RegExp, Blob, ArrayBuffer, typed arrays, class instances, etc.
    return { ok: false }
  }

  // Reject any own symbol properties or forbidden prototype-pollution property names.
  if (Object.getOwnPropertySymbols(value).length > 0) return { ok: false }
  const names = Object.getOwnPropertyNames(value)
  for (const name of names) {
    if (FORBIDDEN_KEYS.has(name)) return { ok: false }
  }

  ancestors.add(value)
  const out: Record<string, unknown> = {}
  for (const key of Object.keys(value)) {
    if (FORBIDDEN_KEYS.has(key)) {
      ancestors.delete(value)
      return { ok: false }
    }
    budget.chars += key.length + 4
    if (budget.chars > MAX_ACTION_ARGS_BYTES) {
      ancestors.delete(value)
      return { ok: false }
    }
    const child = clonePlainJsonValue(value[key], depth + 1, ancestors, budget)
    if (!child.ok) {
      ancestors.delete(value)
      return { ok: false }
    }
    out[key] = child.value
  }
  ancestors.delete(value)
  return { ok: true, value: out }
}

function sanitizePlainJsonObject(raw: unknown): Record<string, unknown> | null {
  if (!isPlainObject(raw)) return null
  const res = clonePlainJsonValue(raw, 0, new Set(), { nodes: 0, chars: 0 })
  if (!res.ok || !isPlainObject(res.value)) return null
  try {
    const serialized = JSON.stringify(res.value)
    if (serialized === undefined || serialized.length > MAX_ACTION_ARGS_BYTES) return null
  } catch {
    return null
  }
  return res.value
}

/**
 * Strict host-side message validator for Deck HTML guest iframes.
 * Rejects empty nonces, non-plain/cyclic/oversized/BigInt/`__proto__` payloads,
 * runtime `control` ops, and anything outside the narrow declared Deck intent protocol.
 */
export function parseDeckGuestIntent(raw: unknown, expectedNonce: string): DeckGuestIntent | null {
  if (typeof expectedNonce !== 'string' || expectedNonce.trim().length === 0) return null
  if (!isPlainObject(raw)) return null
  const msg = raw
  if (msg.channel !== DECK_BRIDGE_CHANNEL) return null
  if (typeof msg.nonce !== 'string' || msg.nonce !== expectedNonce) return null

  if (msg.kind === 'ready') {
    return { kind: 'ready' }
  }

  if (msg.kind === 'action') {
    if (typeof msg.action !== 'string') return null
    const action = msg.action.trim()
    if (!action || action.length > 120 || !/^[a-zA-Z0-9_.-]+$/.test(action)) return null
    const cleanArgs = sanitizePlainJsonObject(msg.args === undefined ? {} : msg.args)
    if (!cleanArgs) return null
    return { kind: 'action', action, args: cleanArgs }
  }

  // `control` and any other intent kinds are strictly rejected from guest HTML.
  return null
}

/** Sliding-window rate limiter for guest-triggered Deck actions. */
export function createGuestActionRateLimiter(
  maxActions = GUEST_ACTION_MAX_PER_WINDOW,
  windowMs = GUEST_ACTION_WINDOW_MS,
) {
  const timestamps: number[] = []
  return {
    allow(now = Date.now()): boolean {
      const cutoff = now - windowMs
      while (timestamps.length > 0 && timestamps[0]! <= cutoff) {
        timestamps.shift()
      }
      if (timestamps.length >= maxActions) return false
      timestamps.push(now)
      return true
    },
    reset(): void {
      timestamps.length = 0
    },
  }
}

export type DeckHtmlMessageContext = {
  expectedSource: unknown
  expectedNonce: string
  /** True when the iframe currently owns document focus (`document.activeElement === iframe`). */
  focused: boolean
  /** True ONLY when `navigator.userActivation?.isActive === true`. */
  userActivationActive: boolean
  /** True when viewing a pinned historical snapshot (blocks all guest actions). */
  pinned?: boolean
  /** Optional rate-limit predicate; returns true to permit the action. */
  allowAction?: () => boolean
  onReady: () => void
  onAction?: (action: string, args: Record<string, unknown>) => void
}

/**
 * Pure host-side bridge dispatcher for Deck HTML iframes.
 * Enforces opaque origin (`'null'`), exact iframe `contentWindow` source, matching per-revision nonce,
 * and for `action` intents: non-pinned view, trusted focus + active user activation, and rate-limiting.
 */
export function handleDeckHtmlMessage(
  ev: { source: unknown; origin?: unknown; data: unknown },
  ctx: DeckHtmlMessageContext,
): boolean {
  if (!ctx.expectedSource || ev.source !== ctx.expectedSource) return false
  // Sandboxed iframes without `allow-same-origin` (both `theuth-page` and `srcDoc`) always have opaque `'null'` origin.
  if (ev.origin !== 'null') return false
  const intent = parseDeckGuestIntent(ev.data, ctx.expectedNonce)
  if (!intent) return false

  if (intent.kind === 'ready') {
    ctx.onReady()
    return true
  }

  if (intent.kind === 'action') {
    if (ctx.pinned || !ctx.onAction) return false
    if (!ctx.focused || !ctx.userActivationActive) return false
    if (ctx.allowAction && !ctx.allowAction()) return false
    ctx.onAction(intent.action, intent.args)
    return true
  }

  return false
}

// ---------------------------------------------------------------------------
// Host-controlled HTML document assembly (real DOMParser + quote-aware fallback)
// ---------------------------------------------------------------------------

function buildBridgeScriptSource(nonce: string): string {
  const safeNonce = serializeSafeJson(nonce)
  const safeChannel = serializeSafeJson(DECK_BRIDGE_CHANNEL)
  return `(function(){
  var NONCE = ${safeNonce};
  var CH = ${safeChannel};
  var listeners = [];
  var hasPayload = false;
  var lastPayload = undefined;
  var readySent = false;
  function sendReady() {
    if (readySent) return;
    readySent = true;
    try {
      if (window.parent) {
        window.parent.postMessage({ channel: CH, nonce: NONCE, kind: "ready" }, "*");
      }
    } catch (_) {}
  }
  document.addEventListener("click", function(ev) {
    var target = ev.target;
    var el = target && target.closest ? target.closest("a[href], area[href]") : null;
    if (!el) return;
    var href = (el.getAttribute("href") || "").trim();
    if (href && href.charAt(0) !== "#") {
      ev.preventDefault();
      ev.stopImmediatePropagation();
    }
  }, true);
  document.addEventListener("submit", function(ev) {
    ev.preventDefault();
    ev.stopImmediatePropagation();
  }, true);
  window.addEventListener("message", function(ev) {
    if (!ev || ev.source !== window.parent) return;
    var d = ev.data;
    if (!d || d.channel !== CH || d.nonce !== NONCE || d.kind !== "data") return;
    hasPayload = true;
    lastPayload = d.payload;
    for (var i = 0; i < listeners.length; i++) {
      try { listeners[i](d.payload); } catch (_) {}
    }
  });
  var api = Object.freeze({
    onData: function(cb) {
      if (typeof cb !== "function") return;
      listeners.push(cb);
      if (hasPayload) { try { cb(lastPayload); } catch (_) {} }
    },
    action: function(name, args) {
      if (typeof name !== "string" || !name) return;
      if (window.parent) {
        window.parent.postMessage({ channel: CH, nonce: NONCE, kind: "action", action: name, args: (args && typeof args === "object" && !Array.isArray(args)) ? args : {} }, "*");
      }
    }
  });
  Object.defineProperty(window, "theuthDeck", { value: api, writable: false, configurable: false });
  if (document.readyState === "loading") {
    window.addEventListener("DOMContentLoaded", sendReady, { once: true });
  } else {
    sendReady();
  }
})();`
}

function stripDangerousDomNodes(root: ParentNode): void {
  const elements = Array.from(root.querySelectorAll('*'))
  for (const el of elements) {
    const tag = el.localName.toLowerCase()
    if (tag === 'base' || tag === 'noscript') {
      el.remove()
      continue
    }
    if (tag === 'meta' && el.hasAttribute('http-equiv')) {
      el.remove()
      continue
    }
    if (tag === 'template' && 'content' in el && (el as HTMLTemplateElement).content) {
      stripDangerousDomNodes((el as HTMLTemplateElement).content)
    }
  }
}

const ASCII_WS = new Set([' ', '\t', '\n', '\r', '\f'])
const LEADING_DOCTYPE_RE = /^<!doctype\s+html\s*>/i
const ATTR_NAME_RE = /^[a-zA-Z_:][a-zA-Z0-9_.:-]*/
const UNQUOTED_FORBIDDEN = new Set(['"', "'", '=', '<', '>', '`', '/'])

function isAsciiWs(ch: string | undefined): boolean {
  return ch !== undefined && ASCII_WS.has(ch)
}

function skipAsciiWs(input: string, pos: number): number {
  let i = pos
  while (i < input.length && isAsciiWs(input[i])) i++
  return i
}

function scanSafeLeadingStartTag(input: string, start: number, tagName: string): number | null {
  if (input[start] !== '<') return null
  const nameEnd = start + 1 + tagName.length
  if (input.slice(start + 1, nameEnd).toLowerCase() !== tagName) return null
  if (nameEnd >= input.length) return null
  const afterName = input[nameEnd]
  if (afterName === '>') return nameEnd + 1
  if (!isAsciiWs(afterName)) return null

  let i = nameEnd
  while (i < input.length) {
    i = skipAsciiWs(input, i)
    if (i >= input.length) return null
    const ch = input[i]
    if (ch === '>') return i + 1
    if (ch === '<' || ch === '/' || ch === '"' || ch === "'" || ch === '=') return null

    const nameMatch = ATTR_NAME_RE.exec(input.slice(i))
    if (!nameMatch) return null
    i += nameMatch[0].length
    i = skipAsciiWs(input, i)

    if (i < input.length && input[i] === '=') {
      i = skipAsciiWs(input, i + 1)
      if (i >= input.length) return null
      const valStart = input[i]
      if (valStart === '"' || valStart === "'") {
        const close = input.indexOf(valStart, i + 1)
        if (close === -1) return null
        i = close + 1
        if (i < input.length && !isAsciiWs(input[i]) && input[i] !== '>') return null
      } else {
        const startVal = i
        while (i < input.length && !isAsciiWs(input[i]) && input[i] !== '>') {
          const vch = input.charAt(i)
          if (UNQUOTED_FORBIDDEN.has(vch)) return null
          i++
        }
        if (i === startVal) return null
      }
    }
  }
  return null
}

function scanLeadingDocumentPrefix(input: string): { kind: 'head' | 'html'; end: number } | null {
  let pos = skipAsciiWs(input, 0)
  const doctypeMatch = LEADING_DOCTYPE_RE.exec(input.slice(pos))
  if (doctypeMatch) {
    pos = skipAsciiWs(input, pos + doctypeMatch[0].length)
  }

  const afterHtml = scanSafeLeadingStartTag(input, pos, 'html')
  if (afterHtml === null) return null

  const headStart = skipAsciiWs(input, afterHtml)
  if (/^<head(?:[\s/>]|$)/i.test(input.slice(headStart))) {
    const afterHead = scanSafeLeadingStartTag(input, headStart, 'head')
    if (afterHead === null) return null
    return { kind: 'head', end: afterHead }
  }

  return { kind: 'html', end: afterHtml }
}

function scanStartTag(
  input: string,
  attrStart: number,
): { end: number; hasHttpEquiv: boolean } {
  let i = attrStart
  let hasHttpEquiv = false
  const len = input.length

  while (i < len) {
    while (i < len && (isAsciiWs(input[i]) || input[i] === '/')) {
      if (input[i] === '/' && input[i + 1] === '>') {
        return { end: i + 2, hasHttpEquiv }
      }
      i++
    }
    if (i >= len) return { end: len, hasHttpEquiv }
    if (input[i] === '>') return { end: i + 1, hasHttpEquiv }

    const nameStart = i
    while (
      i < len &&
      !isAsciiWs(input[i]) &&
      input[i] !== '=' &&
      input[i] !== '/' &&
      input[i] !== '>'
    ) {
      i++
    }
    const attrName = input.slice(nameStart, i).toLowerCase()
    if (attrName === 'http-equiv') {
      hasHttpEquiv = true
    }

    i = skipAsciiWs(input, i)
    if (i < len && input[i] === '=') {
      i = skipAsciiWs(input, i + 1)
      if (i >= len) return { end: len, hasHttpEquiv }
      const ch = input[i]
      if (ch === '"' || ch === "'") {
        const close = input.indexOf(ch, i + 1)
        if (close === -1) return { end: len, hasHttpEquiv }
        i = close + 1
      } else {
        while (i < len && !isAsciiWs(input[i]) && input[i] !== '>') {
          i++
        }
      }
    } else if (i === nameStart) {
      i++
    }
  }
  return { end: len, hasHttpEquiv }
}

function findRawTextClose(input: string, from: number, tagName: string): number {
  const len = input.length
  let i = from
  while (i < len) {
    const lt = input.indexOf('</', i)
    if (lt === -1) return len
    const nameEnd = lt + 2 + tagName.length
    if (input.slice(lt + 2, nameEnd).toLowerCase() === tagName) {
      const next = input[nameEnd]
      if (next === '>' || next === '/' || isAsciiWs(next)) {
        const gt = input.indexOf('>', nameEnd)
        return gt === -1 ? len : gt + 1
      }
    }
    i = lt + 2
  }
  return len
}

function stripDangerousTagsOnce(html: string): string {
  const src = html.replace(/^\uFEFF/, '')
  const len = src.length
  let out = ''
  let lastEmit = 0
  let i = 0

  while (i < len) {
    const lt = src.indexOf('<', i)
    if (lt === -1) break

    if (src.startsWith('<!--', lt)) {
      if (src[lt + 4] === '>') {
        i = lt + 5
      } else if (src.startsWith('<!--->', lt)) {
        i = lt + 6
      } else {
        const close = src.indexOf('-->', lt + 4)
        i = close === -1 ? len : close + 3
      }
      continue
    }

    const first = src[lt + 1]
    if (!first || !/[a-zA-Z]/.test(first)) {
      i = lt + 1
      continue
    }

    let nameEnd = lt + 2
    while (nameEnd < len && /[a-zA-Z0-9-]/.test(src.charAt(nameEnd))) nameEnd++
    const boundary = src[nameEnd]
    if (boundary !== undefined && !isAsciiWs(boundary) && boundary !== '/' && boundary !== '>') {
      i = nameEnd
      continue
    }

    const tagName = src.slice(lt + 1, nameEnd).toLowerCase()
    const scanned = scanStartTag(src, nameEnd)

    if (tagName === 'base' || (tagName === 'meta' && scanned.hasHttpEquiv)) {
      out += src.slice(lastEmit, lt)
      lastEmit = scanned.end
      i = scanned.end
      continue
    }

    if (tagName === 'noscript') {
      const closeEnd = findRawTextClose(src, scanned.end, 'noscript')
      out += src.slice(lastEmit, lt)
      lastEmit = closeEnd
      i = closeEnd
      continue
    }

    if (
      tagName === 'script' ||
      tagName === 'style' ||
      tagName === 'textarea' ||
      tagName === 'title' ||
      tagName === 'xmp'
    ) {
      i = findRawTextClose(src, scanned.end, tagName)
      continue
    }

    i = scanned.end
  }

  return lastEmit === 0 ? src : out + src.slice(lastEmit)
}

function stripDangerousTagsFallback(html: string): string {
  // Run to a small fixed point so nested tag reconstruction like `<ba<base>se>` can never survive.
  let cur = html
  for (let pass = 0; pass < 4; pass++) {
    const next = stripDangerousTagsOnce(cur)
    if (next === cur) break
    cur = next
  }
  return cur
}

/**
 * Quote-balanced HTML5 scanner fallback (used only when `DOMParser` is unavailable, e.g. SSR).
 * Exported so unit tests can independently verify both the real `DOMParser` path and the fallback.
 */
export function buildDeckHtmlDocumentFallback(rawHtml: string, nonce: string): string {
  if (typeof nonce !== 'string' || nonce.trim().length === 0) {
    throw new Error('Deck HTML nonce must be non-empty')
  }
  const sanitized = stripDangerousTagsFallback(rawHtml)
  const bridgeScript = buildBridgeScriptSource(nonce)
  const headInject = [
    `<meta http-equiv="Content-Security-Policy" content="${DECK_HTML_CSP.replace(/"/g, '&quot;')}">`,
    '<meta charset="utf-8">',
    `<script>${bridgeScript}</script>`,
  ].join('\n')

  const leading = scanLeadingDocumentPrefix(sanitized)
  if (leading?.kind === 'head') {
    const prefix = sanitized.slice(0, leading.end)
    const rest = sanitized.slice(leading.end)
    return `${prefix}\n${headInject}\n${rest}`
  }
  if (leading?.kind === 'html') {
    const prefix = sanitized.slice(0, leading.end)
    const rest = sanitized.slice(leading.end)
    return `${prefix}\n<head>\n${headInject}\n</head>\n${rest}`
  }

  return `<!doctype html>\n<html>\n<head>\n${headInject}\n</head>\n<body>\n${sanitized}\n</body>\n</html>`
}

/**
 * Assemble a host-controlled, strict-CSP HTML document with the narrow `window.theuthDeck`
 * guest bridge for a Deck HTML section.
 *
 * Uses the browser/webview's real HTML5 `DOMParser` when available to eliminate regex parsing
 * bypasses (comments, rawtext, quoted `>`, entity-encoded `http-equiv`, `<base>`, `<noscript>`
 * mXSS differentials, malformed leading tags, and `$`-pattern replacement hazards), placing the
 * host-controlled CSP `<meta>` and bridge `<script>` as the very first children of `<head>`.
 * Falls back to the quote-balanced HTML5 scanner only in non-DOM environments.
 */
export function buildDeckHtmlDocument(rawHtml: string, nonce: string): string {
  if (typeof nonce !== 'string' || nonce.trim().length === 0) {
    throw new Error('Deck HTML nonce must be non-empty')
  }
  const cleanSrc = rawHtml.replace(/^\uFEFF/, '')

  if (typeof DOMParser === 'undefined') {
    return buildDeckHtmlDocumentFallback(cleanSrc, nonce)
  }

  const doc = new DOMParser().parseFromString(cleanSrc, 'text/html')
  stripDangerousDomNodes(doc)

  const head = doc.head ?? doc.createElement('head')
  if (!doc.head && doc.documentElement) {
    doc.documentElement.insertBefore(head, doc.documentElement.firstChild)
  }

  const cspMeta = doc.createElement('meta')
  cspMeta.setAttribute('http-equiv', 'Content-Security-Policy')
  cspMeta.setAttribute('content', DECK_HTML_CSP)

  const charsetMeta = doc.createElement('meta')
  charsetMeta.setAttribute('charset', 'utf-8')

  const bridgeScript = doc.createElement('script')
  bridgeScript.textContent = buildBridgeScriptSource(nonce)

  head.prepend(cspMeta, charsetMeta, bridgeScript)
  return `<!doctype html>\n${doc.documentElement.outerHTML}`
}
