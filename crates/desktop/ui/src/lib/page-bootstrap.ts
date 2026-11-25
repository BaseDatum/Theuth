// Design 120 §4.4, §5: the bootstrap the UI injects into an `.html` artifact at render time (the stored bytes stay
// pristine). It themes the page from the first paint, reports its height, routes links to the host and forwards its
// console — JSON-RPC 2.0 over `postMessage` with T3 Code's MCP-Apps-shaped names, so a T3 page works unchanged.
// Pure functions; `HtmlPage.tsx` (inline) and the preview hook (§11) are the two callers and share these parsers.

import { type Appearance, type PageTheme, pageDefaults } from './page-theme'

export const HOST_CONTEXT = 'ui/notifications/host-context-changed'
export const SIZE_CHANGED = 'ui/notifications/size-changed'
export const OPEN_LINK = 'ui/open-link'
export const CONSOLE = 'theuth/notifications/console'
/** 218 §8.7: host → page request `{ x, y }` (frame CSS px) → `{ selector, role, label, rect }` of what is there. */
export const DESCRIBE_POINT = 'theuth/describe-point'

export const CONSOLE_MAX = 20
export const CONSOLE_CHARS = 500

export type ConsoleLevel = 'log' | 'info' | 'warning' | 'error'
export type ConsoleLine = { level: ConsoleLevel; text: string }

/** `host-context-changed` params (T3 `htmlRender.ts:269-274`); also the `#theuth-theme=` fragment payload. */
export type HostContext = { theme: Appearance; styles: { variables: Record<string, string> } }

const NAME = /^--[a-z0-9-]+$/

/** Names whitelisted, values stripped of `;{}<>`: a hostile payload cannot close the style element (§5.1). */
export function sanitizeVariables(vars: unknown): Record<string, string> {
  const out: Record<string, string> = {}
  if (!vars || typeof vars !== 'object') return out
  for (const [k, v] of Object.entries(vars as Record<string, unknown>)) {
    if (!NAME.test(k) || (typeof v !== 'string' && typeof v !== 'number')) continue
    const value = String(v).replace(/[;{}<>]/g, '').trim()
    if (value) out[k] = value
  }
  return out
}

export const hostContext = (theme: PageTheme): HostContext => ({
  theme: theme.appearance,
  styles: { variables: sanitizeVariables(theme.variables) },
})

/** The host → page message posted on `load` and on every theme flip. */
export const hostContextMessage = (theme: PageTheme) => ({ jsonrpc: '2.0', method: HOST_CONTEXT, params: hostContext(theme) })

/** `#theuth-theme=<urlencoded JSON>`: read by the bootstrap before first paint, never sent to the handler. */
export const themeFragment = (theme: PageTheme): string => `#theuth-theme=${encodeURIComponent(JSON.stringify(hostContext(theme)))}`

/** The answer to a page's `ui/open-link` request (T3 `:302-304`). */
export const result = (id: string | number) => ({ jsonrpc: '2.0', id, result: {} })

const block = (vars: Record<string, string>): string =>
  Object.entries(vars)
    .map(([k, v]) => `${k}:${v}`)
    .join(';')

/** §7.4 / §6.3: the base rules under the variables; the page's own scrollbar is hidden (it scrolls inside only when capped). */
export const BASE_CSS =
  'html{background:var(--background);color:var(--foreground);font-family:var(--font-sans);font-size:14px;line-height:1.5;-webkit-text-size-adjust:100%;scrollbar-width:none}' +
  'html::-webkit-scrollbar{display:none}body{margin:0}code,kbd,pre,samp{font-family:var(--font-mono)}'

/** The default stylesheet: dark `:root`, a light `prefers-color-scheme` block, then `BASE_CSS` — a direct load follows the OS. */
export const DEFAULT_STYLE =
  `:root{color-scheme:dark;${block(pageDefaults('dark'))}}` +
  `@media (prefers-color-scheme: light){:root{color-scheme:light;${block(pageDefaults('light'))}}}` +
  BASE_CSS

/**
 * The in-page script body. Runs as `(function(window, document, console, THEME){…})` so a node test can hand it fakes;
 * every global goes through `window.`. Speaks §5: applies `THEME` (and a `#theuth-theme=` fragment, which wins and is
 * stripped), listens for `host-context-changed`, posts `size-changed` from a ResizeObserver, intercepts `a[href]` clicks
 * into `ui/open-link` (http(s) only), and forwards the console (§5.4: 20 × 500, then one `… N more` warning).
 */
export const BOOTSTRAP_JS = `
var NAME = /^--[a-z0-9-]+$/, STRIP = /[;{}<>]/g, BASE = ${JSON.stringify(BASE_CSS)};
var MAX = ${CONSOLE_MAX}, CHARS = ${CONSOLE_CHARS};
var post = function (msg) { try { window.parent.postMessage(msg, '*'); } catch (e) {} };
var notify = function (method, params) { post({ jsonrpc: '2.0', method: method, params: params }); };

var apply = function (ctx) {
  if (!ctx || typeof ctx !== 'object') return;
  var vars = ctx.styles && ctx.styles.variables, k, v, css = '';
  if (vars && typeof vars === 'object') {
    for (k in vars) {
      v = vars[k];
      if (!NAME.test(k) || (typeof v !== 'string' && typeof v !== 'number')) continue;
      v = String(v).replace(STRIP, '');
      if (v) css += k + ':' + v + ';';
    }
  }
  var el = document.getElementById('theuth-theme');
  if (!el) return;
  el.textContent = ':root{color-scheme:' + (ctx.theme === 'light' ? 'light' : 'dark') + ';' + css + '}' + BASE;
};

apply(THEME);
var hash = window.location && window.location.hash;
if (hash && hash.indexOf('#theuth-theme=') === 0) {
  try { apply(JSON.parse(decodeURIComponent(hash.slice(14)))); } catch (e) {}
  try { window.history.replaceState(null, '', window.location.pathname + window.location.search); } catch (e) {}
}

// 218 §8.7: what sits at a point — the nearest meaningful element's role, label (with an ordinal among same-role
// siblings of a tab bar, list, menu or nav), a short CSS path and its rect. The host sanitises every field again.
var IMPLICIT = { A: 'link', BUTTON: 'button', SELECT: 'combobox', TEXTAREA: 'textbox', IMG: 'img', SVG: 'img', CANVAS: 'img',
  NAV: 'navigation', UL: 'list', OL: 'list', MENU: 'list', LI: 'listitem', TABLE: 'table', TR: 'row', TD: 'cell', TH: 'columnheader',
  HEADER: 'banner', FOOTER: 'contentinfo', MAIN: 'main', FORM: 'form', SECTION: 'region', ASIDE: 'complementary', DIALOG: 'dialog',
  SUMMARY: 'button', OPTION: 'option', LABEL: 'label', P: 'paragraph', FIGURE: 'figure', VIDEO: 'video', AUDIO: 'audio' };
var INPUTS = { checkbox: 'checkbox', radio: 'radio', button: 'button', submit: 'button', reset: 'button', image: 'button', range: 'slider' };
var GROUPS = { tablist: 'tab bar', list: 'list', navigation: 'navigation', menu: 'menu', menubar: 'menu bar', listbox: 'list', tree: 'tree', toolbar: 'toolbar', radiogroup: 'options' };
var attr = function (el, n) { return el && el.getAttribute ? el.getAttribute(n) || '' : ''; };
var tagOf = function (el) { return String(el && el.tagName || '').toUpperCase(); };
var roleOf = function (el) {
  var r = attr(el, 'role').split(' ')[0], t = tagOf(el);
  if (r) return r;
  if (t === 'INPUT') return INPUTS[(attr(el, 'type') || 'text').toLowerCase()] || 'textbox';
  if (/^H[1-6]$/.test(t)) return 'heading';
  return IMPLICIT[t] || 'generic';
};
var squash = function (s) { return String(s || '').replace(/\\s+/g, ' ').trim().slice(0, 200); };
var ownLabel = function (el) { return squash(attr(el, 'aria-label') || attr(el, 'alt') || attr(el, 'title')); };
var kids = function (el) { var out = [], c = el && el.children, i; if (c) for (i = 0; i < c.length; i++) out.push(c[i]); return out; };
var ordinal = function (el) {
  var item = el, depth, parent, group, same, i;
  for (depth = 0; item && depth < 4; depth++, item = item.parentElement) {
    parent = item.parentElement;
    group = parent && GROUPS[roleOf(parent)];
    if (!group) continue;
    same = kids(parent).filter(function (k) { return roleOf(k) === roleOf(item); });
    if (same.length < 2) return '';
    for (i = 0; i < same.length; i++) if (same[i] === item) return (ownLabel(parent) || group) + ', item ' + (i + 1);
    return '';
  }
  return '';
};
var selectorOf = function (el) {
  var parts = [], part, cls, same, i;
  for (i = 0; el && el.tagName && i < 4 && tagOf(el) !== 'HTML'; i++, el = el.parentElement) {
    if (/^[A-Za-z][\\w-]*$/.test(el.id || '')) { parts.unshift('#' + el.id); break; }
    part = String(el.tagName).toLowerCase();
    cls = String(attr(el, 'class')).split(/\\s+/)[0];
    if (/^[A-Za-z_][\\w-]*$/.test(cls)) part += '.' + cls;
    same = el.parentElement ? kids(el.parentElement).filter(function (k) { return k.tagName === el.tagName; }) : [];
    if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(el) + 1) + ')';
    parts.unshift(part);
  }
  return parts.join(' '); // descendant combinators: the host strips '>' with the other markup characters
};
var describe = function (x, y) {
  if (typeof document.elementFromPoint !== 'function' || !isFinite(x) || !isFinite(y)) return null;
  var hit = document.elementFromPoint(x, y), el = hit, depth;
  if (!hit) return null;
  for (depth = 0; el && depth < 6 && tagOf(el) !== 'BODY'; depth++, el = el.parentElement)
    if (roleOf(el) !== 'generic' || ownLabel(el)) break;
  if (!el || tagOf(el) === 'BODY' || depth >= 6) el = hit;
  var n = ordinal(el);
  var label = ownLabel(el) || squash(el.innerText || el.textContent || attr(el, 'placeholder') || attr(el, 'value'));
  var b = el.getBoundingClientRect ? el.getBoundingClientRect() : { left: x, top: y, width: 0, height: 0 };
  return {
    selector: selectorOf(el),
    role: roleOf(el),
    label: n ? (label ? label + ' (' + n + ')' : n) : label,
    rect: [Math.round(b.left), Math.round(b.top), Math.round(b.width), Math.round(b.height)]
  };
};

window.addEventListener('message', function (ev) {
  var d = ev.data;
  if (!d || d.jsonrpc !== '2.0') return;
  if (d.method === ${JSON.stringify(HOST_CONTEXT)}) apply(d.params);
  else if (d.method === ${JSON.stringify(DESCRIBE_POINT)} && (typeof d.id === 'string' || typeof d.id === 'number')) {
    var p = d.params || {}, out = null;
    try { out = describe(Number(p.x), Number(p.y)); } catch (e) {}
    post({ jsonrpc: '2.0', id: d.id, result: out });
  }
});

var last = 0;
var measure = function () {
  var root = document.documentElement;
  if (!root) return;
  var h = root.scrollHeight > root.clientHeight ? root.scrollHeight : root.getBoundingClientRect().height;
  h = Math.ceil(h);
  if (!(h > 0) || h === last) return;
  last = h;
  notify(${JSON.stringify(SIZE_CHANGED)}, { height: h });
};
var observe = function () {
  if (typeof window.ResizeObserver === 'function') {
    var ro = new window.ResizeObserver(measure);
    ro.observe(document.documentElement);
    if (document.body) ro.observe(document.body);
  }
  measure();
};
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', observe);
else observe();
window.addEventListener('load', measure);

var links = 0;
document.addEventListener('click', function (ev) {
  if (!ev.isTrusted || ev.defaultPrevented || ev.button) return;
  var a = ev.target && ev.target.closest ? ev.target.closest('a[href]') : null;
  if (!a) return;
  var href = a.getAttribute('href') || '';
  if (href.charAt(0) === '#') return;
  ev.preventDefault();
  var url;
  try { url = new window.URL(href, document.baseURI).href; } catch (e) { return; }
  if (!/^https?:/i.test(url)) return;
  links += 1;
  post({ jsonrpc: '2.0', id: 'theuth-link-' + links, method: ${JSON.stringify(OPEN_LINK)}, params: { url: url } });
}, true);

var sent = 0, omitted = 0, timer = 0;
var text = function (args) {
  var out = [], i, a;
  for (i = 0; i < args.length; i++) {
    a = args[i];
    if (a && typeof a === 'object' && !(a instanceof Error)) { try { a = JSON.stringify(a); } catch (e) { a = String(a); } }
    else if (a instanceof Error) a = a.stack || String(a);
    out.push(String(a));
  }
  var s = out.join(' ');
  return s.length > CHARS ? s.slice(0, CHARS) : s;
};
var emit = function (level, args) {
  if (sent >= MAX) {
    omitted += 1;
    window.clearTimeout(timer);
    timer = window.setTimeout(function () {
      notify(${JSON.stringify(CONSOLE)}, { level: 'warning', text: '\\u2026 ' + omitted + ' more console messages omitted' });
    }, 250);
    return;
  }
  sent += 1;
  notify(${JSON.stringify(CONSOLE)}, { level: level, text: text(args) });
};
var wrap = function (name, level) {
  var orig = console[name];
  console[name] = function () {
    emit(level, arguments);
    if (typeof orig === 'function') { try { orig.apply(console, arguments); } catch (e) {} }
  };
};
wrap('log', 'log'); wrap('info', 'info'); wrap('warn', 'warning'); wrap('error', 'error');
window.addEventListener('error', function (ev) {
  emit('error', [ev.error && ev.error.stack ? ev.error.stack : ev.message || 'Uncaught error']);
});
window.addEventListener('unhandledrejection', function (ev) {
  var r = ev.reason;
  emit('error', ['Unhandled rejection: ' + (r && r.stack ? r.stack : String(r))]);
});
`

/** `</script` in embedded JSON would end the element early. */
const jsonForScript = (v: unknown): string => JSON.stringify(v).replace(/</g, '\\u003c')

const RAW = 'script|style|textarea|title|xmp|iframe|noembed|noframes|noscript|plaintext|template'
/** Comments, raw-text elements and `<template>` contents, blanked to the same length so offsets stay valid (§5.1, T3 `:362-388`). */
const INERT = new RegExp(`<!--[\\s\\S]*?(?:-->|$)|<(${RAW})\\b[^>]*>[\\s\\S]*?(?:</\\1\\s*>|$)`, 'gi')

/**
 * The bootstrapped document: `<meta charset>` (unless the first 4 KiB declare one), the viewport meta (unless present),
 * `<style id="theuth-theme">` and the script, at the start of `<head>` — before the page's own styles so first paint is
 * themed and the page's later `:root` rules still win. Position is found on a scan copy where comments, scripts,
 * styles and templates are blanked, spliced into the original. No `<head>`: after `<html>`, wrapped; else after the
 * doctype; else prepended with one.
 */
export function injectBootstrap(html: string, theme: PageTheme): string {
  const scan = html.replace(INERT, (m) => ' '.repeat(m.length))
  let markup = ''
  if (!/<meta[^>]+charset/i.test(scan.slice(0, 4096))) markup += '<meta charset="utf-8">'
  if (!/<meta[^>]+name=["']?viewport/i.test(scan)) markup += '<meta name="viewport" content="width=device-width, initial-scale=1">'
  markup += `<style id="theuth-theme">${DEFAULT_STYLE}</style>`
  markup += `<script>(function(window,document,console,THEME){${BOOTSTRAP_JS}})(window,document,console,${jsonForScript(hostContext(theme))});</script>`

  const head = /<head\b[^>]*>/i.exec(scan)
  if (head) {
    const at = head.index + head[0].length
    return html.slice(0, at) + markup + html.slice(at)
  }
  const open = /<html\b[^>]*>/i.exec(scan) ?? /<!doctype\b[^>]*>/i.exec(scan)
  if (open) {
    const at = open.index + open[0].length
    return `${html.slice(0, at)}<head>${markup}</head>${html.slice(at)}`
  }
  return `<!doctype html><head>${markup}</head>${html}`
}

type Rpc = { jsonrpc?: unknown; method?: unknown; id?: unknown; params?: unknown }
const rpc = (data: unknown, method: string): Rpc | null => {
  const d = data as Rpc | null
  return d && typeof d === 'object' && d.jsonrpc === '2.0' && d.method === method ? d : null
}

/** `size-changed` → the height when finite and `> 0` (T3 `:277-286`), else `null`. */
export function readSizeChanged(data: unknown): number | null {
  const d = rpc(data, SIZE_CHANGED)
  const h = (d?.params as { height?: unknown } | undefined)?.height
  return typeof h === 'number' && Number.isFinite(h) && h > 0 ? h : null
}

/** `ui/open-link` → `{ id, url }` for `http(s)` only; `javascript:`, `file:`, `asset:`, `theuth-page:` never pass (§5.3). */
export function readOpenLink(data: unknown): { id: string | number; url: string } | null {
  const d = rpc(data, OPEN_LINK)
  if (!d || (typeof d.id !== 'string' && typeof d.id !== 'number')) return null
  const url = (d.params as { url?: unknown } | undefined)?.url
  return typeof url === 'string' && /^https?:\/\//i.test(url) ? { id: d.id, url } : null
}

const LEVELS: readonly ConsoleLevel[] = ['log', 'info', 'warning', 'error']

/** `theuth/notifications/console` → the line, text cut to 500 chars; `null` for anything else. */
export function readConsole(data: unknown): ConsoleLine | null {
  const d = rpc(data, CONSOLE)
  const p = d?.params as { level?: unknown; text?: unknown } | undefined
  if (!p || typeof p.text !== 'string' || !LEVELS.includes(p.level as ConsoleLevel)) return null
  return { level: p.level as ConsoleLevel, text: p.text.slice(0, CONSOLE_CHARS) }
}

export type PointDescription = { selector: string; role: string; label: string; rect: [number, number, number, number] }

/** Untrusted page text (design §6.5) → one line: newlines and `[ ] < >` gone, whitespace collapsed, cut to `max`. */
const clean = (v: unknown, max: number): string =>
  typeof v === 'string' ? v.replace(/[[\]<>]/g, '').replace(/\s+/g, ' ').trim().slice(0, max).trim() : ''

/** A `describe-point` result as the host keeps it: label/selector ≤ 120, role ≤ 24; `null` unless the rect is 4 finite numbers. */
export function sanitizeDescription(raw: unknown): PointDescription | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as { selector?: unknown; role?: unknown; label?: unknown; rect?: unknown }
  if (!Array.isArray(r.rect) || r.rect.length !== 4 || !r.rect.every((n) => typeof n === 'number' && Number.isFinite(n))) return null
  const [x = 0, y = 0, w = 0, h = 0] = r.rect as number[]
  return { selector: clean(r.selector, 120), role: clean(r.role, 24), label: clean(r.label, 120), rect: [x, y, w, h] }
}

let describeSeq = 0

/**
 * 218 §8.7: ask `frame`'s page what sits at `(x, y)` (frame CSS px). The reply is matched by id and by source (the
 * frame is an opaque origin, so `'*'` both ways); `null` on timeout, refusal or no `contentWindow`. Never throws.
 */
export function describePoint(frame: HTMLIFrameElement, x: number, y: number, timeoutMs = 300): Promise<PointDescription | null> {
  const { promise, resolve } = Promise.withResolvers<PointDescription | null>()
  let target: Window | null = null
  try {
    target = frame.contentWindow
  } catch {
    target = null
  }
  if (!target || typeof window === 'undefined' || !Number.isFinite(x) || !Number.isFinite(y)) {
    resolve(null)
    return promise
  }
  const id = `theuth-describe-${++describeSeq}`
  const done = (v: PointDescription | null) => {
    window.removeEventListener('message', onMessage)
    clearTimeout(timer)
    resolve(v)
  }
  const onMessage = (ev: MessageEvent) => {
    const d = ev.data as { jsonrpc?: unknown; id?: unknown; result?: unknown } | null
    if (ev.source !== target || !d || typeof d !== 'object' || d.jsonrpc !== '2.0' || d.id !== id) return
    done(sanitizeDescription(d.result))
  }
  window.addEventListener('message', onMessage)
  const timer = setTimeout(() => done(null), timeoutMs)
  try {
    target.postMessage({ jsonrpc: '2.0', id, method: DESCRIBE_POINT, params: { x, y } }, '*')
  } catch {
    done(null)
  }
  return promise
}
