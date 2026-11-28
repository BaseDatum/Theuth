import type { RuntimeControlOp } from './types'

/**
 * Strict iframe sandbox attribute for Deck HTML sections:
 * - NO `allow-same-origin` (opaque origin, cannot touch host DOM/storage/cookies)
 * - NO `allow-forms` (no form exfiltration)
 * - NO `allow-popups` / `allow-top-navigation`
 */
export const DECK_HTML_SANDBOX = 'allow-scripts'

/**
 * Strict CSP enforced both via `theuth-page` HTTP header (`profile: "deck"`)
 * AND injected `<meta http-equiv="Content-Security-Policy">` for srcdoc fallback.
 */
export const DECK_HTML_CSP =
  "default-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; object-src 'none'; frame-src 'none'; img-src data:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'"

export const DECK_BRIDGE_CHANNEL = 'theuth-deck'

const ALLOWED_CONTROL_OPS = new Set<RuntimeControlOp>(['start', 'pause', 'resume', 'stop', 'run_once'])

export type DeckGuestIntent =
  | { kind: 'ready' }
  | { kind: 'action'; action: string; args: Record<string, unknown> }
  | { kind: 'control'; op: RuntimeControlOp }

/**
 * Strict host-side message validator for Deck HTML guest iframes.
 * Rejects anything outside the narrow declared Deck intent protocol (never admits arbitrary tool/MCP/RPC calls).
 */
export function parseDeckGuestIntent(raw: unknown, expectedNonce: string): DeckGuestIntent | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const msg = raw as Record<string, unknown>
  if (msg.channel !== DECK_BRIDGE_CHANNEL) return null
  if (typeof msg.nonce !== 'string' || msg.nonce !== expectedNonce) return null

  if (msg.kind === 'ready') {
    return { kind: 'ready' }
  }

  if (msg.kind === 'action') {
    if (typeof msg.action !== 'string') return null
    const action = msg.action.trim()
    if (!action || action.length > 120 || !/^[a-zA-Z0-9_.-]+$/.test(action)) return null
    const args = msg.args === undefined ? {} : msg.args
    if (!args || typeof args !== 'object' || Array.isArray(args)) return null
    return { kind: 'action', action, args: args as Record<string, unknown> }
  }

  if (msg.kind === 'control') {
    if (typeof msg.op !== 'string' || !ALLOWED_CONTROL_OPS.has(msg.op as RuntimeControlOp)) return null
    return { kind: 'control', op: msg.op as RuntimeControlOp }
  }

  return null
}

/**
 * Inject strict CSP meta + narrow `window.theuthDeck` guest bridge into a Deck HTML section.
 * Strips `<base>` tags and any author CSP meta tags before prepending the locked-down policy.
 */
export function buildDeckHtmlDocument(rawHtml: string, nonce: string): string {
  const safeNonce = JSON.stringify(nonce)
  const sanitized = rawHtml
    .replace(/<base\b[^>]*>/gi, '')
    .replace(/<meta\b[^>]*http-equiv\s*=\s*["']?content-security-policy["']?[^>]*>/gi, '')

  const bridgeHead = [
    `<meta http-equiv="Content-Security-Policy" content="${DECK_HTML_CSP.replace(/"/g, '&quot;')}">`,
    '<script>',
    '(function(){',
    `  var NONCE = ${safeNonce};`,
    `  var CH = ${JSON.stringify(DECK_BRIDGE_CHANNEL)};`,
    '  var listeners = [];',
    '  var lastPayload = null;',
    '  window.addEventListener("message", function(ev){',
    '    var d = ev && ev.data;',
    '    if (!d || d.channel !== CH || d.nonce !== NONCE || d.kind !== "data") return;',
    '    lastPayload = d.payload;',
    '    for (var i = 0; i < listeners.length; i++) {',
    '      try { listeners[i](d.payload); } catch (_) {}',
    '    }',
    '  });',
    '  var api = Object.freeze({',
    '    onData: function(cb){',
    '      if (typeof cb !== "function") return;',
    '      listeners.push(cb);',
    '      if (lastPayload !== null) { try { cb(lastPayload); } catch (_) {} }',
    '    },',
    '    action: function(name, args){',
    '      if (typeof name !== "string" || !name) return;',
    '      window.parent.postMessage({ channel: CH, nonce: NONCE, kind: "action", action: name, args: (args && typeof args === "object" && !Array.isArray(args)) ? args : {} }, "*");',
    '    },',
    '    control: function(op){',
    '      window.parent.postMessage({ channel: CH, nonce: NONCE, kind: "control", op: op }, "*");',
    '    }',
    '  });',
    '  Object.defineProperty(window, "theuthDeck", { value: api, writable: false, configurable: false });',
    '  window.addEventListener("DOMContentLoaded", function(){',
    '    window.parent.postMessage({ channel: CH, nonce: NONCE, kind: "ready" }, "*");',
    '  });',
    '})();',
    '</script>',
  ].join('\n')

  if (/<head\b[^>]*>/i.test(sanitized)) {
    return sanitized.replace(/<head\b[^>]*>/i, (m) => `${m}\n${bridgeHead}`)
  }
  return `<!doctype html><html><head><meta charset="utf-8">${bridgeHead}</head><body>${sanitized}</body></html>`
}
