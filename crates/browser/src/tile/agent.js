// theuth tile agent (design 209 §4.7). ONE source, run two ways:
// - in the user's web view tile, by the client's op `script` with `world: "isolated"`
//   (WebKit `callAsyncJavaScript` in the content world `theuth-agent`; CDP isolated world on Windows);
// - on obscura, through `browser_evaluate` of `(async (cmd, params) => { <this file> })(…)`.
// This file is the BODY of an async function: `cmd` (a command name) and `params` (an object)
// are in scope, and its `return` is the result. It never throws: a failure returns `{ error }`.
// State (the refs) lives on the world's global as `__theuthAgent` and dies with the document.
// Refs `e<n>` map to elements through a Map of WeakRefs; the page's DOM is never stamped.
// Values typed into fields are never returned; password and credential fields show no value.

const VERSION = 1;

let agent = globalThis.__theuthAgent;
if (!agent || agent.version !== VERSION) {
  agent = (() => {
    // ---- refs ---------------------------------------------------------------------------
    const refs = new Map(); // "e<n>" -> WeakRef<Element>
    const ids = new WeakMap(); // Element -> "e<n>"
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
        if (!el || !el.isConnected) refs.delete(r);
      }
    }

    // ---- reading the page ---------------------------------------------------------------
    const INTERACTIVE =
      'a[href], button, input:not([type=hidden]), select, textarea, summary, ' +
      '[role=button], [role=link], [role=checkbox], [role=radio], [role=switch], [role=tab], ' +
      '[role=menuitem], [role=option], [role=combobox], [role=textbox], [role=searchbox], ' +
      '[contenteditable=""], [contenteditable=true], [onclick], [tabindex]:not([tabindex="-1"])';
    const MAX_ELEMENTS = 500;
    const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'SVG', 'HEAD', 'IFRAME', 'OBJECT']);
    const SECRET_AUTOCOMPLETE = /(password|one-time-code|cc-|csc)/i;

    const clip = (s, n) => {
      const t = String(s == null ? '' : s).replace(/\s+/g, ' ').trim();
      return t.length > n ? t.slice(0, n - 1) + '\u2026' : t;
    };

    function style(el) {
      try {
        return typeof getComputedStyle === 'function' ? getComputedStyle(el) : null;
      } catch (_) {
        return null;
      }
    }

    function visible(el) {
      if (!el || !el.isConnected) return false;
      if (typeof el.checkVisibility === 'function') {
        try {
          if (!el.checkVisibility({ visibilityProperty: true })) return false;
        } catch (_) {
          // older engines: fall through to the ancestor walk
        }
      }
      for (let n = el; n && n.nodeType === 1; n = n.parentElement) {
        if (n.hidden || n.getAttribute('aria-hidden') === 'true') return false;
        const s = style(n);
        if (s && (s.display === 'none' || (n === el && s.visibility === 'hidden'))) return false;
      }
      return true;
    }

    function textOf(el) {
      return el ? clip(el.innerText !== undefined ? el.innerText : el.textContent, 200) : '';
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
      const text = clip(el.innerText !== undefined ? el.innerText : el.textContent, 80);
      if (text) return text;
      const img = el.querySelector && el.querySelector('img[alt]');
      if (img && img.alt.trim()) return clip(img.alt, 80);
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
      if (el.isContentEditable) return 'textbox';
      return 'generic';
    }

    function secret(el) {
      if (el.tagName !== 'INPUT' && el.tagName !== 'TEXTAREA') return false;
      return (el.type || '').toLowerCase() === 'password' || SECRET_AUTOCOMPLETE.test(el.autocomplete || '');
    }

    function describe(el) {
      return el.tagName.toLowerCase() + ' "' + accessibleName(el) + '"';
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
      if ((tag === 'input' || tag === 'textarea') && !secret(el) && !['checkbox', 'radio', 'submit', 'button', 'reset', 'image', 'file'].includes(kind) && el.value) {
        s += ' value=' + JSON.stringify(clip(el.value, 80));
      }
      const chosen = tag === 'select' ? (el.selectedOptions ? el.selectedOptions[0] : el.options[el.selectedIndex]) : null;
      if (chosen) {
        s += ' value=' + JSON.stringify(clip(chosen.text, 80));
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
        if (!visible(el)) continue;
        total++;
        if (out.length < MAX_ELEMENTS) out.push(line(el));
      }
      if (total > out.length) out.push('...(' + (total - out.length) + ' more elements not listed)');
      return out;
    }

    function bodyText() {
      const b = document.body;
      if (!b) return '';
      const t = b.innerText !== undefined ? b.innerText : b.textContent;
      return String(t || '')
        .split('\n')
        .map((l) => l.replace(/[ \t\u00a0]+/g, ' ').trim())
        .join('\n')
        .replace(/\n{3,}/g, '\n\n')
        .trim();
    }

    const BLOCKS = new Set(['P', 'DIV', 'SECTION', 'ARTICLE', 'MAIN', 'HEADER', 'FOOTER', 'NAV', 'ASIDE', 'UL', 'OL', 'TABLE', 'TBODY', 'THEAD', 'TR', 'FORM', 'BLOCKQUOTE', 'DL', 'DT', 'DD', 'FIGURE', 'BODY', 'LI', 'PRE', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'FIELDSET']);

    function markdown(root) {
      const out = [];
      const flat = (s) => s.replace(/\s+/g, ' ').trim();
      // Inline content of one node as one line (links, code and images kept).
      function one(c) {
        if (c.nodeType === 3) return c.nodeValue;
        if (c.nodeType !== 1 || SKIP.has(c.tagName.toUpperCase()) || !visible(c)) return '';
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
          if (c.nodeType === 1 && !SKIP.has(c.tagName.toUpperCase()) && visible(c) && BLOCKS.has(c.tagName.toUpperCase())) {
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

    // ---- targets ------------------------------------------------------------------------
    function resolve(p) {
      if (p.ref) {
        const w = refs.get(p.ref);
        if (!w) {
          throw new Error('unknown ref ' + p.ref + ': refs come from browser_read or browser_elements of this page and go stale after a navigation; re-run browser_elements');
        }
        const el = w.deref();
        if (!el || !el.isConnected) {
          refs.delete(p.ref);
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
      if (el.isContentEditable) return 'content';
      if (el.tagName === 'TEXTAREA') return 'value';
      if (el.tagName === 'INPUT') {
        const t = (el.type || 'text').toLowerCase();
        if (['checkbox', 'radio', 'submit', 'button', 'reset', 'image', 'file', 'range', 'color'].includes(t)) return null;
        return 'value';
      }
      return null;
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

    // ---- commands -----------------------------------------------------------------------
    const commands = {
      async location() {
        return { url: location.href, title: document.title, ready: document.readyState };
      },

      async read(p) {
        await settled(Math.min(p.settle_ms || 5000, 10000));
        const max = Math.max(0, p.max_chars == null ? 4000 : p.max_chars);
        const page = { url: location.href, title: document.title };
        if (max === 0) return Object.assign(page, { text: '' });
        if (p.format === 'markdown') {
          const md = markdown(document.body);
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
        const body = bodyText();
        text += body.length > remaining ? body.slice(0, remaining) + '\u2026' : body;
        return Object.assign(page, { text });
      },

      async elements() {
        await settled(5000);
        return { url: location.href, title: document.title, lines: listing() };
      },

      async click(p) {
        const el = resolve(p);
        if (el.disabled) throw new Error(describe(el) + ' is disabled');
        scrollTo(el);
        const at = center(el);
        for (const t of ['pointerover', 'pointerenter', 'mouseover', 'pointerdown', 'mousedown']) fire(el, t, Object.assign({ button: 0, buttons: 1 }, at));
        focus(el);
        for (const t of ['pointerup', 'mouseup']) fire(el, t, Object.assign({ button: 0 }, at));
        const what = describe(el);
        el.click();
        return { clicked: what, url: location.href };
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
        const go = fire(el, 'keydown', init);
        if (go && (key.length === 1 || key === 'Enter')) fire(el, 'keypress', Object.assign({ charCode: init.keyCode }, init));
        let submitted = false;
        if (go && key === 'Enter') {
          const form = el.form || (el.closest && el.closest('form'));
          if (form && el.tagName !== 'TEXTAREA' && !el.isContentEditable) {
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
            hit = bodyText().includes(p.text);
          }
          const waited = Date.now() - started;
          if (hit) return { found: p.selector ? 'selector' : 'text', waited_ms: waited };
          if (waited >= limit) {
            throw new Error('timed out after ' + Math.round(limit / 1000) + ' s waiting for ' + (p.selector ? 'selector ' + JSON.stringify(p.selector) : 'text ' + JSON.stringify(p.text)));
          }
          await sleep(100);
        }
      },
    };

    function scrollTo0(x, y) {
      try { window.scrollTo(x, y); } catch (_) { /* no layout */ }
    }

    // Helpers for later commands (209 P5: serialise a form, fill fields by index).
    const helpers = { resolve, describe, accessibleName, role, visible, secret, refOf, fire, setNativeValue, focus, scrollTo, editable, settled, markdown, bodyText, listing };

    return { version: VERSION, commands, helpers };
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
