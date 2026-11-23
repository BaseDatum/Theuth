// theuth tile agent (design 209 §4.7). ONE source, run two ways:
// - in the user's web view tile, by the client's op `script` with `world: "isolated"`
//   (WebKit `callAsyncJavaScript` in the content world `theuth-agent`; CDP isolated world on Windows);
// - on obscura, through `browser_evaluate` of `(async (cmd, params) => { <this file> })(…)`.
// This file is the BODY of an async function: `cmd` (a command name) and `params` (an object)
// are in scope, and its `return` is the result. It never throws: a failure returns `{ error }`.
// State (the refs) lives on the world's global as `__theuthAgent` and dies with the document.
// Refs `e<n>` map to elements through a Map of WeakRefs; the page's DOM is never stamped.
// What the user typed never leaves the page: a listing shows `value=` only for fields this agent
// set, `value=[set]` for any other non-empty one, nothing for password and credential fields,
// and the page text skips form controls and editable regions.
// 209 P5: `serialize` and `fill_form` lift one element into the chat card and type the user's card
// values back into it by index (`data-theuth-field`); neither returns a value of any field.
// The engine replaces the VERSION line with the hash of this source (crates/browser/src/tile.rs).

const VERSION = 1;

let agent = globalThis.__theuthAgent;
if (!agent || agent.version !== VERSION) {
  agent = (() => {
    // This document's token: a read after an action that left the page compares it.
    const DOC = Math.random().toString(36).slice(2, 10) + Date.now().toString(36);

    // ---- refs ---------------------------------------------------------------------------
    const refs = new Map(); // "e<n>" -> WeakRef<Element>
    const ids = new WeakMap(); // Element -> "e<n>"
    const dead = new Set(); // refs whose element left the page
    let next = 1;
    const weak = (el) => (typeof WeakRef === 'function' ? new WeakRef(el) : { deref: () => el });

    function refOf(el) {
      let r = ids.get(el);
      if (!r) {
        r = 'e' + next++;
        ids.set(el, r);
        refs.set(r, weak(el));
      }
      return r;
    }

    function prune() {
      for (const [r, w] of refs) {
        const el = w.deref();
        if (!el || !el.isConnected) {
          refs.delete(r);
          dead.add(r);
        }
      }
    }

    // Fields this agent filled, typed into or selected: only their values are listed.
    const SET = new WeakSet();

    // ---- reading the page ---------------------------------------------------------------
    const INTERACTIVE =
      'a[href], button, input:not([type=hidden]), select, textarea, summary, ' +
      '[role=button], [role=link], [role=checkbox], [role=radio], [role=switch], [role=tab], ' +
      '[role=menuitem], [role=option], [role=combobox], [role=textbox], [role=searchbox], ' +
      '[contenteditable=""], [contenteditable=true], [onclick], [tabindex]:not([tabindex="-1"])';
    const MAX_ELEMENTS = 500;
    const MAX_CHARS = 40000;
    const SHORT_BODY = 200;
    const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'SVG', 'HEAD', 'IFRAME', 'OBJECT']);
    const SECRET_AUTOCOMPLETE = /(password|one-time-code|cc-|csc)/i;
    const NO_VALUE = ['checkbox', 'radio', 'submit', 'button', 'reset', 'image', 'file'];

    const clip = (s, n) => {
      const t = String(s == null ? '' : s).replace(/\s+/g, ' ').trim();
      return t.length > n ? t.slice(0, n - 1) + '\u2026' : t;
    };

    // Natives bound when this agent is built (209 §9): page code that runs later (a handler the
    // card's fill fires, a timer) cannot swap them under the snapshot, the field check or the
    // result. On obscura the agent runs in the page's world, so a page that replaced them before
    // the agent's first command is not covered; the tile's isolated world is.
    const apply = Reflect.apply;
    const nat = {
      gcs: typeof getComputedStyle === 'function' ? getComputedStyle : null,
      qs: Document.prototype.querySelector,
      qsa: Element.prototype.querySelectorAll,
      get: Element.prototype.getAttribute,
      set: Element.prototype.setAttribute,
      del: Element.prototype.removeAttribute,
      has: Element.prototype.hasAttribute,
    };
    const attrOf = (el, n) => apply(nat.get, el, [n]);

    function style(el) {
      try {
        return nat.gcs ? apply(nat.gcs, globalThis, [el]) : null;
      } catch (_) {
        return null;
      }
    }

    // One native check where the engine has it; the ancestor walk stops at the first hidden one.
    function visible(el) {
      if (!el || !el.isConnected) return false;
      if (el.closest && el.closest('[hidden], [aria-hidden="true"]')) return false;
      if (typeof el.checkVisibility === 'function') {
        try {
          return el.checkVisibility({ visibilityProperty: true });
        } catch (_) {
          // older engines: fall through to the ancestor walk
        }
      }
      for (let n = el; n && n.nodeType === 1; n = n.parentElement) {
        const s = style(n);
        if (s && (s.display === 'none' || (n === el && s.visibility === 'hidden'))) return false;
      }
      return true;
    }

    // An element whose own box is not drawn; `display: contents` has no box but its children do.
    function shown(el) {
      if (el.hidden) return false;
      if (typeof el.checkVisibility === 'function') {
        try {
          if (el.checkVisibility()) return true;
          const s = style(el);
          return !!s && s.display === 'contents';
        } catch (_) {
          // fall through
        }
      }
      const s = style(el);
      return !(s && s.display === 'none');
    }

    // Inside an editable region (`contenteditable`; the attribute too, for engines without
    // `isContentEditable`): what is there is the user's draft.
    function editableRegion(el) {
      return !!el.isContentEditable || !!(el.closest && el.closest('[contenteditable]:not([contenteditable="false"])'));
    }

    // A form control or an editable region: what the user types lives there.
    function formish(el) {
      const t = el.tagName;
      return t === 'INPUT' || t === 'TEXTAREA' || t === 'SELECT' || editableRegion(el);
    }

    function accessibleName(el) {
      const aria = el.getAttribute('aria-label');
      if (aria && aria.trim()) return clip(aria, 80);
      const by = el.getAttribute('aria-labelledby');
      if (by) {
        const t = by
          .split(/\s+/)
          .map((id) => document.getElementById(id))
          .filter(Boolean)
          .map((n) => n.textContent)
          .join(' ');
        if (t.trim()) return clip(t, 80);
      }
      if (el.labels && el.labels.length) {
        const t = Array.from(el.labels).map((l) => l.textContent).join(' ');
        if (t.trim()) return clip(t, 80);
      } else if (el.id) {
        try {
          const l = document.querySelector('label[for="' + CSS.escape(el.id) + '"]');
          if (l && l.textContent.trim()) return clip(l.textContent, 80);
        } catch (_) {
          // no CSS.escape: skip the label lookup
        }
      }
      const wrap = el.closest && el.closest('label');
      if (wrap && wrap !== el && wrap.textContent.trim()) return clip(wrap.textContent, 80);
      const tag = el.tagName;
      if (tag === 'INPUT') {
        const type = (el.type || '').toLowerCase();
        if (['submit', 'button', 'reset'].includes(type) && el.value) return clip(el.value, 80);
        if (type === 'image' && el.alt) return clip(el.alt, 80);
      }
      if (tag === 'IMG' && el.alt) return clip(el.alt, 80);
      // An editable region's text is what the user wrote: never its name.
      if (!editableRegion(el)) {
        const text = clip(el.innerText !== undefined ? el.innerText : el.textContent, 80);
        if (text) return text;
        const img = el.querySelector && el.querySelector('img[alt]');
        if (img && img.alt.trim()) return clip(img.alt, 80);
      }
      return clip(el.getAttribute('placeholder') || el.getAttribute('title') || '', 80);
    }

    function role(el) {
      const explicit = el.getAttribute('role');
      if (explicit) return explicit.split(/\s+/)[0];
      const tag = el.tagName;
      if (tag === 'A') return 'link';
      if (tag === 'BUTTON' || tag === 'SUMMARY') return 'button';
      if (tag === 'SELECT') return el.multiple ? 'listbox' : 'combobox';
      if (tag === 'TEXTAREA') return 'textbox';
      if (tag === 'INPUT') {
        const type = (el.type || 'text').toLowerCase();
        if (['submit', 'button', 'reset', 'image'].includes(type)) return 'button';
        if (type === 'checkbox' || type === 'radio') return type;
        if (type === 'range') return 'slider';
        if (type === 'search') return 'searchbox';
        return 'textbox';
      }
      if (editableRegion(el)) return 'textbox';
      return 'generic';
    }

    function secret(el) {
      if (el.tagName !== 'INPUT' && el.tagName !== 'TEXTAREA') return false;
      return (el.type || '').toLowerCase() === 'password' || SECRET_AUTOCOMPLETE.test(el.autocomplete || '');
    }

    function describe(el) {
      return el.tagName.toLowerCase() + ' "' + accessibleName(el) + '"';
    }

    // ` value="…"` for a field this agent set, ` value=[set]` for another non-empty one.
    function valueOf(el, value) {
      return SET.has(el) ? ' value=' + JSON.stringify(clip(value, 80)) : ' value=[set]';
    }

    function line(el) {
      const tag = el.tagName.toLowerCase();
      let s = 'ref=' + refOf(el) + ' ' + tag + ' ' + JSON.stringify(accessibleName(el)) + ' role=' + role(el);
      const type = el.getAttribute('type');
      if (type) s += ' type=' + JSON.stringify(clip(type, 40));
      const name = el.getAttribute('name');
      if (name) s += ' name=' + JSON.stringify(clip(name, 80));
      if (tag === 'a' && el.getAttribute('href')) s += ' href=' + JSON.stringify(clip(el.href, 120));
      const kind = (el.type || '').toLowerCase();
      if ((tag === 'input' || tag === 'textarea') && !secret(el) && !NO_VALUE.includes(kind) && el.value) {
        s += valueOf(el, el.value);
      } else if (editableRegion(el) && el.textContent.trim()) {
        s += valueOf(el, el.textContent);
      }
      const chosen = tag === 'select' ? (el.selectedOptions ? el.selectedOptions[0] : el.options[el.selectedIndex]) : null;
      if (chosen) {
        // The page's own default is page content; another choice is the user's unless we made it.
        const defaults = Array.from(el.options).some((o) => o.defaultSelected);
        const pageDefault = chosen.defaultSelected || (!defaults && el.selectedIndex === 0);
        s += pageDefault ? ' value=' + JSON.stringify(clip(chosen.text, 80)) : valueOf(el, chosen.text);
      }
      if (kind === 'checkbox' || kind === 'radio') s += ' checked=' + !!el.checked;
      if (el.disabled) s += ' disabled=true';
      if (el.required) s += ' required=true';
      for (const a of ['expanded', 'selected', 'pressed', 'checked']) {
        const v = el.getAttribute('aria-' + a);
        if (v !== null && !(a === 'checked' && (kind === 'checkbox' || kind === 'radio'))) s += ' ' + a + '=' + v;
      }
      return s;
    }

    function listing() {
      prune();
      const out = [];
      let total = 0;
      for (const el of Array.from(document.querySelectorAll(INTERACTIVE))) {
        // Inside an editable region every element is the user's draft.
        if (el.parentElement && editableRegion(el.parentElement)) continue;
        if (!visible(el)) continue;
        total++;
        if (out.length < MAX_ELEMENTS) out.push(line(el));
      }
      if (total > out.length) out.push('...(' + (total - out.length) + ' more elements not listed)');
      return out;
    }

    const BLOCKS = new Set(['P', 'DIV', 'SECTION', 'ARTICLE', 'MAIN', 'HEADER', 'FOOTER', 'NAV', 'ASIDE', 'UL', 'OL', 'TABLE', 'TBODY', 'THEAD', 'TR', 'FORM', 'BLOCKQUOTE', 'DL', 'DT', 'DD', 'FIGURE', 'BODY', 'LI', 'PRE', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'FIELDSET']);

    // The page's visible text, from its text nodes (no clone, no innerText): form controls and
    // editable regions are skipped, so nothing the user typed is in it.
    function bodyText() {
      const b = document.body;
      if (!b) return '';
      const out = [];
      const walk = (node, pre) => {
        for (let c = node.firstChild; c; c = c.nextSibling) {
          if (c.nodeType === 3) {
            out.push(pre ? c.nodeValue : c.nodeValue.replace(/\s+/g, ' '));
            continue;
          }
          if (c.nodeType !== 1) continue;
          const tag = c.tagName.toUpperCase();
          if (SKIP.has(tag) || formish(c) || !shown(c)) continue;
          if (tag === 'BR') {
            out.push('\n');
            continue;
          }
          const block = BLOCKS.has(tag);
          if (block) out.push('\n');
          else if (tag === 'TD' || tag === 'TH') out.push(' ');
          walk(c, pre || tag === 'PRE');
          if (block) out.push('\n');
        }
      };
      walk(b, false);
      return out
        .join('')
        .split('\n')
        .map((l) => l.replace(/[ \t\u00a0\r]+/g, ' ').trim())
        .join('\n')
        .replace(/\n{3,}/g, '\n\n')
        .trim();
    }

    // What a short read did not cover: frames and (open) shadow roots.
    function unread() {
      const frames = document.querySelectorAll('iframe, frame').length;
      let shadows = 0;
      for (const el of document.querySelectorAll('*')) if (el.shadowRoot) shadows++;
      if (!frames && !shadows) return '';
      return '\n(' + frames + ' frames and ' + shadows + ' shadow roots on this page were not read)';
    }

    function markdown(root) {
      const out = [];
      const flat = (s) => s.replace(/\s+/g, ' ').trim();
      // Inline content of one node as one line (links, code and images kept).
      function one(c) {
        if (c.nodeType === 3) return c.nodeValue;
        if (c.nodeType !== 1 || SKIP.has(c.tagName.toUpperCase()) || formish(c) || !visible(c)) return '';
        const tag = c.tagName.toUpperCase();
        if (tag === 'A' && c.getAttribute('href')) return '[' + (flat(inline(c)) || c.href) + '](' + c.href + ')';
        if (tag === 'IMG') return c.alt ? '![' + clip(c.alt, 80) + ']' : '';
        if (tag === 'BR') return ' ';
        if (tag === 'CODE') return '`' + c.textContent + '`';
        return inline(c);
      }
      function inline(node) {
        return Array.from(node.childNodes).map(one).join('');
      }
      // A block: its inline runs as paragraphs, its block children in document order.
      function block(node) {
        const tag = node.tagName.toUpperCase();
        if (/^H[1-6]$/.test(tag)) {
          const t = flat(inline(node));
          if (t) out.push('#'.repeat(Number(tag[1])) + ' ' + t);
          return;
        }
        if (tag === 'PRE') {
          out.push('```\n' + node.textContent.replace(/\n+$/, '') + '\n```');
          return;
        }
        let run = '';
        const flush = () => {
          const t = flat(run);
          if (t) out.push(tag === 'LI' ? '- ' + t : t);
          run = '';
        };
        for (const c of Array.from(node.childNodes)) {
          if (c.nodeType === 1 && !SKIP.has(c.tagName.toUpperCase()) && !formish(c) && visible(c) && BLOCKS.has(c.tagName.toUpperCase())) {
            flush();
            block(c);
          } else {
            run += one(c);
          }
        }
        flush();
      }
      if (root) block(root);
      return out.join('\n\n').trim();
    }

    function settled(ms) {
      if (document.readyState === 'complete') return Promise.resolve(true);
      return new Promise((resolve) => {
        const done = () => resolve(document.readyState === 'complete');
        const t = setTimeout(done, ms);
        addEventListener('load', () => { clearTimeout(t); done(); }, { once: true });
      });
    }

    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

    // Run `act` and report whether it started leaving this page: the address changed, or the
    // page is unloading (`beforeunload` / `pagehide` within a moment). `from` is the address
    // before; the engine's next read waits for another document or address. The wait ends at the
    // first `beforeunload` / `pagehide`, while the document is still alive: a timer still pending
    // when the document goes away never fires, and the command would never answer (WebKit drops
    // the script's result and the call runs into its timeout).
    async function leaving(act) {
      const from = location.href;
      let left = false;
      let wake = () => {};
      const gone = new Promise((resolve) => { wake = resolve; });
      const mark = () => { left = true; wake(); };
      addEventListener('beforeunload', mark);
      addEventListener('pagehide', mark);
      let out;
      try {
        out = act();
        await Promise.race([sleep(50), gone]);
      } finally {
        removeEventListener('beforeunload', mark);
        removeEventListener('pagehide', mark);
      }
      return Object.assign(out, { navigating: left || location.href !== from, from, url: location.href, doc: DOC });
    }

    // ---- targets ------------------------------------------------------------------------
    function resolve(p) {
      if (p.ref) {
        const w = refs.get(p.ref);
        const el = w && w.deref();
        if (!w && !dead.has(p.ref)) {
          throw new Error('unknown ref ' + p.ref + ': refs come from browser_read or browser_elements of this page and go stale after a navigation; re-run browser_elements');
        }
        if (!el || !el.isConnected) {
          refs.delete(p.ref);
          dead.add(p.ref);
          throw new Error('stale ref ' + p.ref + ': that element is gone from the page; re-run browser_elements');
        }
        return el;
      }
      if (p.selector) {
        let el;
        try {
          el = document.querySelector(p.selector);
        } catch (_) {
          throw new Error('invalid CSS selector ' + JSON.stringify(p.selector));
        }
        if (!el) throw new Error('no element matches ' + JSON.stringify(p.selector));
        return el;
      }
      throw new Error('pass `ref` or `selector`');
    }

    // ---- events -------------------------------------------------------------------------
    function center(el) {
      try {
        const r = el.getBoundingClientRect();
        return { clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 };
      } catch (_) {
        return { clientX: 0, clientY: 0 };
      }
    }

    // What the page shows on top at `at` when it is not `el` (or a part or label of it).
    function coveredBy(el, at) {
      if (typeof document.elementFromPoint !== 'function') return '';
      const top = document.elementFromPoint(at.clientX, at.clientY);
      if (!top || top === el || el.contains(top) || top.contains(el)) return '';
      if (top.tagName === 'LABEL' && top.control === el) return '';
      return describe(top);
    }

    function fire(el, type, init) {
      const opts = Object.assign({ bubbles: true, cancelable: true, composed: true }, init || {});
      let ev;
      if (type.startsWith('pointer') && typeof PointerEvent === 'function') {
        ev = new PointerEvent(type, Object.assign({ pointerId: 1, pointerType: 'mouse', isPrimary: true }, opts));
      } else if (type.startsWith('pointer')) {
        return true;
      } else if (type.startsWith('mouse') || type === 'click') {
        ev = new MouseEvent(type, opts);
      } else if (type.startsWith('key')) {
        ev = new KeyboardEvent(type, opts);
      } else if ((type === 'input' || type === 'beforeinput') && typeof InputEvent === 'function') {
        ev = new InputEvent(type, opts);
      } else {
        ev = new Event(type, opts);
      }
      return el.dispatchEvent(ev);
    }

    function setNativeValue(el, value) {
      const proto =
        el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype
          : el.tagName === 'SELECT' ? HTMLSelectElement.prototype
            : HTMLInputElement.prototype;
      const desc = Object.getOwnPropertyDescriptor(proto, 'value');
      if (desc && desc.set) desc.set.call(el, value);
      else el.value = value;
    }

    function focus(el) {
      try {
        el.focus({ preventScroll: true });
      } catch (_) {
        try { el.focus(); } catch (_e) { /* not focusable */ }
      }
    }

    function scrollTo(el) {
      try {
        el.scrollIntoView({ block: 'center', inline: 'center' });
      } catch (_) {
        try { el.scrollIntoView(); } catch (_e) { /* no layout */ }
      }
    }

    function editable(el) {
      if (el.tagName === 'TEXTAREA') return 'value';
      if (el.tagName === 'INPUT') {
        const t = (el.type || 'text').toLowerCase();
        if (['checkbox', 'radio', 'submit', 'button', 'reset', 'image', 'file', 'range', 'color'].includes(t)) return null;
        return 'value';
      }
      return editableRegion(el) ? 'content' : null;
    }

    const KEYS = {
      Enter: { code: 'Enter', keyCode: 13 }, Tab: { code: 'Tab', keyCode: 9 },
      Escape: { code: 'Escape', keyCode: 27 }, Backspace: { code: 'Backspace', keyCode: 8 },
      Delete: { code: 'Delete', keyCode: 46 }, ' ': { code: 'Space', keyCode: 32 },
      ArrowUp: { code: 'ArrowUp', keyCode: 38 }, ArrowDown: { code: 'ArrowDown', keyCode: 40 },
      ArrowLeft: { code: 'ArrowLeft', keyCode: 37 }, ArrowRight: { code: 'ArrowRight', keyCode: 39 },
      Home: { code: 'Home', keyCode: 36 }, End: { code: 'End', keyCode: 35 },
      PageUp: { code: 'PageUp', keyCode: 33 }, PageDown: { code: 'PageDown', keyCode: 34 },
    };

    function keyInit(key) {
      const k = KEYS[key];
      if (k) return { key, code: k.code, keyCode: k.keyCode, which: k.keyCode };
      if (key.length === 1) {
        const up = key.toUpperCase();
        const code = /[A-Z]/.test(up) ? 'Key' + up : /[0-9]/.test(key) ? 'Digit' + key : '';
        return { key, code, keyCode: up.charCodeAt(0), which: up.charCodeAt(0) };
      }
      return { key, code: key };
    }

    function focusables() {
      return Array.from(document.querySelectorAll('a[href], button, input:not([type=hidden]), select, textarea, [tabindex]:not([tabindex="-1"]), [contenteditable=""], [contenteditable=true]'))
        .filter((el) => !el.disabled && visible(el));
    }

    // ---- 209 P5: one element of the page as a card in the chat ---------------------------
    // `serialize` clones an element into an inert snapshot (computed styles inlined, no script,
    // no address, no value) and stamps its controls `data-theuth-field="<n>"` in the live page;
    // `fill_form` sets the values the user typed into the card by those indices and submits.
    const FORM_ID = /^[0-9a-f-]{36}$/;
    const MAX_SNAPSHOT = 256 * 1024;
    // Dropped with their content: they run, load or embed something.
    const DROPPED = new Set(['SCRIPT', 'NOSCRIPT', 'STYLE', 'LINK', 'META', 'BASE', 'HEAD', 'TITLE', 'TEMPLATE', 'IFRAME', 'FRAME', 'FRAMESET', 'OBJECT', 'EMBED', 'APPLET', 'CANVAS', 'VIDEO', 'AUDIO', 'SOURCE', 'TRACK', 'PORTAL', 'MATH', 'DIALOG']);
    const VOID = new Set(['INPUT', 'BR', 'HR', 'IMG', 'WBR', 'COL', 'AREA']);
    const SUBMIT = 'button:not([type]):not([disabled]), button[type=submit]:not([disabled]), input[type=submit]:not([disabled]), input[type=image]:not([disabled])';
    // Inherited properties are written only where they differ from the parent's.
    const INHERITED = ['color', 'font-family', 'font-size', 'font-weight', 'font-style', 'line-height', 'letter-spacing', 'text-align', 'text-transform', 'text-indent', 'white-space', 'visibility', 'cursor', 'list-style-type'];
    const OWN = ['display', 'position', 'top', 'right', 'bottom', 'left', 'z-index', 'float', 'clear', 'box-sizing',
      'width', 'height', 'min-width', 'min-height', 'max-width', 'max-height',
      'margin-top', 'margin-right', 'margin-bottom', 'margin-left', 'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
      'border-top-width', 'border-right-width', 'border-bottom-width', 'border-left-width',
      'border-top-style', 'border-right-style', 'border-bottom-style', 'border-left-style',
      'border-top-color', 'border-right-color', 'border-bottom-color', 'border-left-color',
      'border-top-left-radius', 'border-top-right-radius', 'border-bottom-right-radius', 'border-bottom-left-radius',
      'background-color', 'background-image', 'background-size', 'background-position', 'background-repeat',
      'text-decoration-line', 'vertical-align', 'opacity', 'box-shadow', 'outline-width', 'outline-style', 'outline-color',
      'flex-direction', 'flex-wrap', 'justify-content', 'align-items', 'align-content', 'align-self', 'flex-grow', 'flex-shrink', 'flex-basis', 'order',
      'row-gap', 'column-gap', 'grid-template-columns', 'grid-template-rows', 'grid-column', 'grid-row',
      'overflow-x', 'overflow-y', 'appearance', 'transform'];

    const escText = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    const escAttr = (s) => escText(s).replace(/"/g, '&quot;');

    // A value that loads nothing: a `url()` other than a `data:image/` one drops the property.
    function inert(v) {
      const all = String(v).toLowerCase().match(/url\(\s*['"]?[^'")\s]*/g) || [];
      return all.every((u) => /url\(\s*['"]?data:image\//.test(u)) && !/image-set\(|image\(|\\/.test(String(v).toLowerCase());
    }

    // Computed styles of fresh elements: inside a closed shadow root under an `all: initial` host
    // the page's own rules do not reach them, so they show the browser's defaults. A form's default
    // submit button (`:default`) is probed as one, inside a form: WebKit gives it its own colours,
    // radius and weight, and its native look is lost once those are written inline (209 P5 proof).
    function defaultsOf() {
      const cache = new Map();
      let host = null;
      let place = null;
      try {
        host = document.createElement('div');
        host.setAttribute('style', 'all: initial; position: absolute; width: 0; height: 0; overflow: hidden; visibility: hidden');
        place = typeof host.attachShadow === 'function' ? host.attachShadow({ mode: 'closed' }) : host;
        (document.body || document.documentElement).appendChild(host);
      } catch (_) {
        host = null;
      }
      const isDefault = (el) => {
        try {
          return isSubmit(el) && el.matches(':default');
        } catch (_) {
          return false;
        }
      };
      return {
        get(el) {
          const tag = el.tagName;
          const type = tag === 'INPUT' ? (el.type || 'text').toLowerCase() : '';
          const dflt = isDefault(el);
          const key = tag + ' ' + type + (dflt ? ' default' : '');
          if (cache.has(key)) return cache.get(key);
          const out = {};
          if (host) {
            let holder = null;
            try {
              const probe = document.createElement(tag.toLowerCase());
              if (type) probe.setAttribute('type', type);
              else if (dflt) probe.setAttribute('type', 'submit');
              if (dflt) {
                holder = document.createElement('form');
                holder.appendChild(probe);
                place.appendChild(holder);
              } else {
                holder = probe;
                place.appendChild(probe);
              }
              const s = getComputedStyle(probe);
              for (const p of OWN) out[p] = s.getPropertyValue(p);
              for (const p of INHERITED) out[p] = s.getPropertyValue(p);
            } catch (_) {
              // no defaults: every property counts as set
            }
            if (holder) holder.remove();
          }
          cache.set(key, out);
          return out;
        },
        done() {
          if (host) host.remove();
        },
      };
    }

    // What makes WebKit paint a control with the author's style instead of its native look: a
    // background or border other than the default (border radii count too, and WebKit reports a
    // native button's adjusted radius, which differs from a fresh one's).
    const PAINT = ['background-color', 'background-image',
      'border-top-width', 'border-right-width', 'border-bottom-width', 'border-left-width',
      'border-top-style', 'border-right-style', 'border-bottom-style', 'border-left-style',
      'border-top-color', 'border-right-color', 'border-bottom-color', 'border-left-color'];
    const RADII = new Set(['border-top-left-radius', 'border-top-right-radius', 'border-bottom-right-radius', 'border-bottom-left-radius']);

    // `style="…"` of `el`: its own properties that differ from the default of its tag, and the
    // inherited ones that differ from what its parent passes down. A control the page leaves with
    // its native look (an `appearance` other than `none`, background and borders the defaults)
    // gets no radius either, so the copy keeps that look (209 P5 proof).
    function styleOf(el, defaults, inheritedFrom) {
      const s = style(el);
      const mine = {};
      if (!s || typeof s.getPropertyValue !== 'function') return { css: '', inherited: inheritedFrom };
      const d = defaults.get(el);
      const look = s.getPropertyValue('appearance') || s.getPropertyValue('-webkit-appearance');
      const native = !!look && look !== 'none' && PAINT.every((p) => s.getPropertyValue(p) === d[p]);
      const parts = [];
      for (const p of OWN) {
        if (native && RADII.has(p)) continue;
        const v = s.getPropertyValue(p);
        if (v && v !== d[p] && inert(v)) parts.push(p + ': ' + v);
      }
      for (const p of INHERITED) {
        const v = s.getPropertyValue(p);
        mine[p] = v;
        const base = inheritedFrom ? inheritedFrom[p] : d[p];
        if (v && v !== base && inert(v)) parts.push(p + ': ' + v);
      }
      return { css: parts.join('; '), inherited: mine };
    }

    function box(el) {
      let w = 0;
      let h = 0;
      try {
        const r = el.getBoundingClientRect();
        w = Math.round(r.width);
        h = Math.round(r.height);
      } catch (_) {
        // no layout
      }
      return { w, h };
    }

    // An empty box the size of `el`: what a cross-origin image or an inline SVG becomes.
    function emptyBox(el) {
      const b = box(el);
      return '<span style="display: inline-block; width: ' + b.w + 'px; height: ' + b.h + 'px"></span>';
    }

    // A same-origin image as a `data:` URI (PNG through a canvas), else '' (a box then).
    function imageData(img) {
      try {
        const src = img.currentSrc || img.src || '';
        if (/^data:image\/(png|jpeg|gif|webp);base64,/i.test(src)) return src;
        if (new URL(src, location.href).origin !== location.origin) return '';
        if (!img.complete || !img.naturalWidth) return '';
        const b = box(img);
        const c = document.createElement('canvas');
        c.width = Math.max(1, Math.min(img.naturalWidth, (b.w || img.naturalWidth) * 2));
        c.height = Math.max(1, Math.min(img.naturalHeight, (b.h || img.naturalHeight) * 2));
        c.getContext('2d').drawImage(img, 0, 0, c.width, c.height);
        return c.toDataURL('image/png');
      } catch (_) {
        return '';
      }
    }

    function isSubmit(el) {
      const t = (el.getAttribute('type') || '').toLowerCase();
      if (el.tagName === 'BUTTON') return t === '' || t === 'submit';
      return el.tagName === 'INPUT' && (t === 'submit' || t === 'image');
    }

    // A control the card fills (`isSubmit` ones are stamped too, to submit a form-less widget).
    function fillable(el) {
      if (el.tagName === 'SELECT' || el.tagName === 'TEXTAREA') return true;
      if (el.tagName !== 'INPUT') return false;
      return !['hidden', 'submit', 'image', 'button', 'reset', 'file'].includes((el.type || 'text').toLowerCase());
    }

    function kindOf(el) {
      if (isSubmit(el)) return 'submit';
      if (el.tagName === 'INPUT') return (el.type || 'text').toLowerCase();
      return el.tagName.toLowerCase();
    }

    // The words the page shows for a field: never anything the user typed into it.
    function fieldLabel(el) {
      if (isSubmit(el)) {
        const t = el.tagName === 'INPUT' ? el.value || el.alt || '' : el.textContent;
        return clip(t || el.getAttribute('aria-label') || 'Submit', 80);
      }
      const aria = el.getAttribute('aria-label');
      if (aria && aria.trim()) return clip(aria, 80);
      if (el.labels && el.labels.length) {
        const t = Array.from(el.labels).map((l) => l.textContent).join(' ');
        if (t.trim()) return clip(t, 80);
      }
      const wrap = el.closest && el.closest('label');
      if (wrap && wrap.textContent.trim()) return clip(wrap.textContent, 80);
      return clip(el.getAttribute('placeholder') || el.getAttribute('title') || el.getAttribute('name') || '', 80);
    }

    function frameOrigin(f) {
      try {
        const o = new URL(f.getAttribute('src') || f.getAttribute('data') || '', location.href).origin;
        return o && o !== 'null' ? o : 'an unknown origin';
      } catch (_) {
        return 'an unknown origin';
      }
    }

    function byteLength(s) {
      try {
        return new TextEncoder().encode(s).length;
      } catch (_) {
        return unescape(encodeURIComponent(s)).length;
      }
    }

    // The snapshot of `root` as an HTML string, plus the live controls it stamps (in order). The
    // walk stops once the snapshot outgrows MAX_SNAPSHOT (`over`), so a huge element costs little.
    function snapshot(root) {
      const defaults = defaultsOf();
      const stamps = [];
      const out = {
        parts: [],
        size: 0,
        push(s) {
          this.parts.push(s);
          this.size += s.length;
        },
      };
      const attrs = (el, extra) => {
        let a = '';
        for (const n of ['id', 'class', 'name', 'placeholder', 'autocomplete', 'for', 'alt']) {
          const v = el.getAttribute(n);
          if (v !== null && v !== '') a += ' ' + n + '="' + escAttr(v) + '"';
        }
        if (el.disabled) a += ' disabled';
        return a + (extra || '');
      };
      const walk = (el, inherited, isRoot) => {
        if (DROPPED.has(el.tagName) || out.size > MAX_SNAPSHOT) return;
        if (el.tagName === 'INPUT' && (el.type || '').toLowerCase() === 'hidden') return;
        if (!isRoot && !shown(el)) return;
        const tag = el.tagName.toLowerCase();
        if (el.namespaceURI && el.namespaceURI !== 'http://www.w3.org/1999/xhtml') {
          out.push(emptyBox(el));
          return;
        }
        const st = styleOf(el, defaults, inherited);
        const css = st.css ? ' style="' + escAttr(st.css) + '"' : '';
        if (tag === 'img') {
          const data = imageData(el);
          out.push(data ? '<img src="' + escAttr(data) + '"' + attrs(el) + css + '>' : emptyBox(el));
          return;
        }
        let stamp = '';
        if ((fillable(el) || isSubmit(el)) && visible(el)) {
          stamp = ' data-theuth-field="' + stamps.length + '"';
          stamps.push(el);
        }
        if (tag === 'input') {
          const type = (el.type || 'text').toLowerCase();
          if (type === 'submit' || type === 'image' || type === 'button' || type === 'reset') {
            const t = type === 'submit' || type === 'image' ? 'submit' : 'button';
            out.push('<button type="' + t + '"' + attrs(el, stamp) + css + '>' + escText(el.value || el.alt || 'Submit') + '</button>');
            return;
          }
          const checked = (type === 'checkbox' || type === 'radio') && el.checked ? ' checked' : '';
          out.push('<input type="' + escAttr(type) + '"' + attrs(el, stamp) + checked + css + '>');
          return;
        }
        if (tag === 'option') {
          out.push('<option' + attrs(el) + (el.selected ? ' selected' : '') + css + '>' + escText(el.text) + '</option>');
          return;
        }
        if (tag === 'textarea') {
          out.push('<textarea' + attrs(el, stamp) + css + '></textarea>');
          return;
        }
        let type = '';
        if (tag === 'button') type = ' type="' + (isSubmit(el) ? 'submit' : 'button') + '"';
        out.push('<' + tag + type + attrs(el, stamp) + css + '>');
        if (VOID.has(el.tagName)) return;
        // An editable region holds the user's draft: its box, never its text.
        if (!editableRegion(el)) {
          for (let c = el.firstChild; c; c = c.nextSibling) {
            if (c.nodeType === 3) out.push(escText(c.nodeValue));
            else if (c.nodeType === 1) walk(c, st.inherited, false);
          }
        }
        out.push('</' + tag + '>');
      };
      try {
        walk(root, null, true);
      } finally {
        defaults.done();
      }
      return { html: out.parts.join(''), stamps, over: out.size > MAX_SNAPSHOT };
    }

    // ---- commands -----------------------------------------------------------------------
    const commands = {
      async location() {
        return { url: location.href, title: document.title, ready: document.readyState, doc: DOC };
      },

      async read(p) {
        await settled(Math.min(p.settle_ms || 5000, 10000));
        const max = Math.min(MAX_CHARS, Math.max(0, p.max_chars == null ? 4000 : p.max_chars));
        const page = { url: location.href, title: document.title, doc: DOC };
        if (max === 0) return Object.assign(page, { text: '' });
        if (p.format === 'markdown') {
          let md = markdown(document.body);
          if (md.length < SHORT_BODY) md += unread();
          return Object.assign(page, { text: md.length > max ? md.slice(0, max) + '\u2026' : md });
        }
        const lines = listing();
        let text = '';
        let remaining = max;
        let shown = 0;
        for (const l of lines) {
          if (l.length + 1 > remaining) break;
          text += l + '\n';
          remaining -= l.length + 1;
          shown++;
        }
        if (shown < lines.length) text += '...(' + (lines.length - shown) + ' controls omitted; use browser_elements)\n';
        let body = bodyText();
        if (body.length < SHORT_BODY) body += unread();
        text += body.length > remaining ? body.slice(0, remaining) + '\u2026' : body;
        return Object.assign(page, { text });
      },

      async elements() {
        await settled(5000);
        return { url: location.href, title: document.title, lines: listing(), doc: DOC };
      },

      async click(p) {
        const el = resolve(p);
        if (el.disabled) throw new Error(describe(el) + ' is disabled');
        scrollTo(el);
        const at = center(el);
        const covered = coveredBy(el, at);
        const what = describe(el);
        return leaving(() => {
          for (const t of ['pointerover', 'pointerenter', 'mouseover', 'pointerdown', 'mousedown']) fire(el, t, Object.assign({ button: 0, buttons: 1 }, at));
          focus(el);
          for (const t of ['pointerup', 'mouseup']) fire(el, t, Object.assign({ button: 0 }, at));
          // The click itself at the element's centre (activation runs as for el.click()); an
          // engine without the MouseEvent constructor falls back to el.click() at (0, 0).
          try {
            fire(el, 'click', Object.assign({ button: 0, detail: 1 }, at));
          } catch (_) {
            el.click();
          }
          return { clicked: what, covered };
        });
      },

      async fill(p) {
        const el = resolve(p);
        if (el.tagName === 'SELECT') return commands.select(p);
        if (el.tagName === 'INPUT' && (el.type || '').toLowerCase() === 'file') {
          throw new Error('a file input cannot be filled with text');
        }
        const how = editable(el);
        if (!how) throw new Error(describe(el) + ' is not a text field');
        if (el.disabled || el.readOnly) throw new Error(describe(el) + ' is not editable');
        scrollTo(el);
        focus(el);
        const value = String(p.value == null ? '' : p.value);
        if (how === 'content') {
          el.textContent = value;
        } else {
          setNativeValue(el, value);
        }
        SET.add(el);
        fire(el, 'input', { inputType: 'insertText', data: value, cancelable: false });
        fire(el, 'change', { cancelable: false });
        return { filled: describe(el) };
      },

      async type(p) {
        const el = resolve(p);
        const how = editable(el);
        if (!how) throw new Error(describe(el) + ' is not a text field');
        if (el.disabled || el.readOnly) throw new Error(describe(el) + ' is not editable');
        scrollTo(el);
        focus(el);
        SET.add(el);
        const text = String(p.text == null ? '' : p.text);
        for (const ch of text) {
          const init = keyInit(ch);
          const go = fire(el, 'keydown', init);
          fire(el, 'keypress', Object.assign({ charCode: ch.charCodeAt(0) }, init));
          if (go && fire(el, 'beforeinput', { inputType: 'insertText', data: ch })) {
            if (how === 'content') el.textContent += ch;
            else setNativeValue(el, el.value + ch);
            fire(el, 'input', { inputType: 'insertText', data: ch, cancelable: false });
          }
          fire(el, 'keyup', init);
        }
        fire(el, 'change', { cancelable: false });
        return { typed: Array.from(text).length, into: describe(el) };
      },

      async press(p) {
        const key = String(p.key || '');
        if (!key) throw new Error('pass a `key`');
        const el = p.selector || p.ref ? resolve(p) : document.activeElement || document.body;
        const init = keyInit(key);
        return leaving(() => {
          const go = fire(el, 'keydown', init);
          if (go && (key.length === 1 || key === 'Enter')) fire(el, 'keypress', Object.assign({ charCode: init.keyCode }, init));
          let submitted = false;
          if (go && key === 'Enter') {
            const form = el.form || (el.closest && el.closest('form'));
            if (form && el.tagName !== 'TEXTAREA' && !editableRegion(el)) {
              if (typeof form.requestSubmit === 'function') form.requestSubmit();
              else {
                const btn = form.querySelector('button:not([type]), button[type=submit], input[type=submit]');
                if (btn) btn.click();
              }
              submitted = true;
            } else if (el.tagName === 'A' || el.tagName === 'BUTTON') {
              el.click();
            }
          }
          if (go && key === 'Tab') {
            const list = focusables();
            const i = list.indexOf(document.activeElement);
            const to = list[(i + (p.shift ? -1 : 1) + list.length) % list.length];
            if (to) focus(to);
          }
          fire(el, 'keyup', init);
          const on = el === document.body ? 'the page' : describe(el);
          return { pressed: key, on, submitted };
        });
      },

      async select(p) {
        const el = resolve(p);
        if (el.tagName !== 'SELECT') throw new Error(describe(el) + ' is not a <select>');
        const want = String(p.value == null ? '' : p.value);
        const opts = Array.from(el.options);
        const opt = opts.find((o) => o.value === want) || opts.find((o) => o.text.trim() === want.trim()) ||
          opts.find((o) => o.text.trim().toLowerCase() === want.trim().toLowerCase());
        if (!opt) {
          const known = opts.slice(0, 20).map((o) => JSON.stringify(o.text.trim())).join(', ');
          throw new Error('no option ' + JSON.stringify(want) + ' in ' + describe(el) + '; options: ' + known);
        }
        focus(el);
        setNativeValue(el, opt.value);
        opt.selected = true;
        SET.add(el);
        fire(el, 'input', { cancelable: false });
        fire(el, 'change', { cancelable: false });
        return { selected: opt.text.trim(), in: describe(el) };
      },

      async scroll(p) {
        const se = document.scrollingElement || document.documentElement;
        if (p.ref || p.selector) {
          scrollTo(resolve(p));
        } else {
          const dir = p.direction || 'down';
          const dy = p.amount || innerHeight || 600;
          const dx = p.amount || innerWidth || 800;
          if (dir === 'top') scrollTo0(0, 0);
          else if (dir === 'bottom') scrollTo0(scrollX, se.scrollHeight);
          else if (dir === 'up') scrollBy(0, -dy);
          else if (dir === 'down') scrollBy(0, dy);
          else if (dir === 'left') scrollBy(-dx, 0);
          else if (dir === 'right') scrollBy(dx, 0);
          else throw new Error('unknown direction ' + JSON.stringify(dir));
        }
        await sleep(50);
        return { x: Math.round(scrollX), y: Math.round(scrollY), height: se.scrollHeight, viewport: innerHeight };
      },

      // Polls every 250 ms; a text is looked for in the cheap `textContent` first and confirmed
      // in the visible text only when it is there.
      async wait(p) {
        const limit = Math.max(0, Math.min(p.timeout_ms == null ? 30000 : p.timeout_ms, 60000));
        const started = Date.now();
        if ((p.selector ? 1 : 0) + (p.text ? 1 : 0) !== 1) throw new Error('pass exactly one of `selector` or `text`');
        for (;;) {
          let hit = false;
          if (p.selector) {
            try {
              hit = !!document.querySelector(p.selector);
            } catch (_) {
              throw new Error('invalid CSS selector ' + JSON.stringify(p.selector));
            }
          } else {
            const b = document.body;
            hit = !!b && String(b.textContent || '').includes(p.text) && bodyText().includes(p.text);
          }
          const waited = Date.now() - started;
          if (hit) return { found: p.selector ? 'selector' : 'text', waited_ms: waited };
          if (waited >= limit) {
            throw new Error('timed out after ' + Math.round(limit / 1000) + ' s waiting for ' + (p.selector ? 'selector ' + JSON.stringify(p.selector) : 'text ' + JSON.stringify(p.text)));
          }
          await sleep(250);
        }
      },

      // 209 P5: `root` (ref or selector) as an inert snapshot for the chat card. Refuses a
      // frame, an empty element and a snapshot over 256 KiB without stamping anything.
      async serialize(p) {
        const root = resolve(p);
        const id = String(p.form || '');
        if (!FORM_ID.test(id)) throw new Error('bad form id');
        const frame = /^(IFRAME|FRAME|OBJECT|EMBED)$/.test(root.tagName) ? root : root.querySelector('iframe, frame, object, embed');
        if (frame) return { refused: 'frame', origin: frameOrigin(frame) };
        const snap = snapshot(root);
        const bytes = byteLength(snap.html);
        if (snap.over || bytes > MAX_SNAPSHOT) {
          const whole = Math.max(bytes, byteLength(root.outerHTML || ''));
          return { refused: 'size', kib: Math.ceil(whole / 1024) };
        }
        if (!snap.stamps.some(fillable)) return { refused: 'empty' };
        // Marks of this card only (`-<id hex>`), so another card of the same page stays valid.
        const hex = id.replace(/-/g, '');
        const fieldAttr = 'data-theuth-field-' + hex;
        snap.stamps.forEach((el, i) => apply(nat.set, el, [fieldAttr, String(i)]));
        apply(nat.set, root, ['data-theuth-form-' + hex, '']);
        const fields = snap.stamps.map((el, i) => ({
          index: i,
          kind: kindOf(el),
          name: clip(attrOf(el, 'name') || '', 120),
          label: fieldLabel(el),
          masked: secret(el),
        }));
        const b = box(root);
        const action = actionOf(root, snap.stamps);
        return { html: snap.html, width: b.w, height: b.h, url: location.href, title: document.title, fields, action, doc: DOC };
      },

      // 209 P5: the user's values (`{ "<index>": string }`) into the controls `serialize`
      // stamped, then submit; `{ stale }` and nothing typed when the page is not the one shown:
      // another address, the element gone, or its controls not the ones of the card (`fields`,
      // each `{ name, kind }` in index order). Only the keys sent are filled.
      async fill_form(p) {
        const id = String(p.form || '');
        if (!FORM_ID.test(id)) throw new Error('bad form id');
        const values = p.values;
        if (!values || typeof values !== 'object' || !Object.keys(values).length) throw new Error('no values to fill');
        const bare = (u) => String(u || '').split('#')[0];
        if (bare(location.href) !== bare(p.url)) return { stale: 'address' };
        const hex = id.replace(/-/g, '');
        const fieldAttr = 'data-theuth-field-' + hex;
        const formAttr = 'data-theuth-form-' + hex;
        const root = apply(nat.qs, document, ['[' + formAttr + ']']);
        if (!root) return { stale: 'form' };
        const stamped = (apply(nat.has, root, [fieldAttr]) ? [root] : []).concat(Array.from(apply(nat.qsa, root, ['[' + fieldAttr + ']'])));
        const want = Array.isArray(p.fields) ? p.fields : [];
        if (stamped.length !== want.length) return { stale: 'fields' };
        const at = new Map(stamped.map((el) => [attrOf(el, fieldAttr), el]));
        for (let i = 0; i < want.length; i++) {
          const el = at.get(String(i));
          const w = want[i] || {};
          if (!el || kindOf(el) !== w.kind || clip(attrOf(el, 'name') || '', 120) !== w.name) return { stale: 'fields' };
        }
        const keys = Object.keys(values).filter((k) => /^\d+$/.test(k) && at.has(k)).sort((a, b) => a - b);
        // Unchecked first, so a radio group ends on the one the user chose.
        const order = keys.filter((k) => values[k] === 'false').concat(keys.filter((k) => values[k] !== 'false'));
        const filled = [];
        for (const k of order) {
          const el = at.get(k);
          const v = String(values[k] == null ? '' : values[k]);
          if (el.disabled || isSubmit(el)) continue;
          const type = (el.type || '').toLowerCase();
          if (el.tagName === 'SELECT') {
            const opts = Array.from(el.options);
            const opt = opts.find((o) => o.value === v) || opts.find((o) => o.text.trim() === v.trim());
            if (!opt) continue;
            setNativeValue(el, opt.value);
            opt.selected = true;
          } else if (el.tagName === 'INPUT' && (type === 'checkbox' || type === 'radio')) {
            const on = v === 'true';
            if (el.checked === on) {
              filled.push(Number(k));
              continue;
            }
            el.checked = on;
          } else if (editable(el) === 'value' && !el.readOnly) {
            focus(el);
            setNativeValue(el, v);
          } else {
            continue;
          }
          fire(el, 'input', { cancelable: false });
          fire(el, 'change', { cancelable: false });
          filled.push(Number(k));
        }
        filled.sort((a, b) => a - b);
        const { form, button } = submitterOf(root, stamped);
        for (const el of stamped) apply(nat.del, el, [fieldAttr]);
        apply(nat.del, root, [formAttr]);
        return leaving(() => {
          let submitted = '';
          if (form && button) {
            button.click();
            submitted = 'button';
          } else if (form && typeof form.requestSubmit === 'function') {
            form.requestSubmit();
            submitted = 'form';
          } else if (button) {
            button.click();
            submitted = 'field';
          }
          return { filled, submitted };
        });
      },
    };

    // The form `root` submits and the button that submits it: the form's first enabled submit
    // button (none: `requestSubmit`), or without a form the element's own stamped one.
    function submitterOf(root, stamped) {
      const form = root.tagName === 'FORM' ? root : root.querySelector('form') || (root.closest && root.closest('form'));
      const button = form ? form.querySelector(SUBMIT) : stamped.find((el) => isSubmit(el) && !el.disabled);
      return { form, button: button || null };
    }

    // Where the form posts, resolved: the submitting button's `formaction`, else the form's
    // `action`, else the page (209 §4.10: the card names it).
    function actionOf(root, stamped) {
      try {
        const { form, button } = submitterOf(root, stamped);
        const raw = (button && attrOf(button, 'formaction')) || (form && attrOf(form, 'action')) || '';
        return raw.trim() ? new URL(raw, document.baseURI).href : location.href;
      } catch (_) {
        return location.href;
      }
    }

    function scrollTo0(x, y) {
      try { window.scrollTo(x, y); } catch (_) { /* no layout */ }
    }

    return { version: VERSION, commands };
  })();
  globalThis.__theuthAgent = agent;
}

try {
  const run = Object.prototype.hasOwnProperty.call(agent.commands, cmd) ? agent.commands[cmd] : null;
  if (!run) return { error: 'unknown agent command ' + JSON.stringify(cmd) };
  return await run(params || {});
} catch (e) {
  return { error: String((e && e.message) || e) };
}
