// Supplement to polyfill.js for the app bundles (crates/js-runtime: `Bundle::web_polyfill`), loaded right after
// it: TextEncoder/TextDecoder, structuredClone, URL and console (from the mermaid spike's polyfill,
// docs/design/apps-mermaid-inproc-spike.md). DOCUMENTS does not load it: its bundle `typeof`-probes
// `TextEncoder` (lib0), so defining it there would change the documents' code paths.
//
// APPS (theuth-apps-inproc.js), measured by packages/apps/test/inproc.test.ts (a node:vm context with only
// ECMAScript built-ins + polyfill.js + this file): `TextEncoder` only (check.ts's UTF-8 byte cap, inproc.ts's
// line cap). Its opt-in telemetry needs `fetch` + `process.env`, which do not exist here, so it stays off.
// MERMAID (theuth-mermaid-inproc.js), measured by the spike's bare-context corpus run and replayed by
// packages/apps/test/inproc-mermaid.test.ts: `TextEncoder`/`TextDecoder` (vscode-jsonrpc in every langium-based
// diagram chunk), `structuredClone` (pie/radar/usecase dbs, mermaid's config), `URL` (only
// @braintree/sanitize-url on `click X "http…"`, behind `URL.canParse`: answering false makes the href
// about:blank, the parse answer is unchanged; constructing one throws), timers (polyfill.js; referenced, not called).
((core) => {
  // UTF-8 only, WHATWG semantics for well-formed input; lone surrogates encode as U+FFFD like the spec.
  class TextEncoder {
    get encoding() {
      return "utf-8";
    }
    encode(input = "") {
      const s = String(input);
      const out = new Uint8Array(s.length * 3);
      let n = 0;
      for (let i = 0; i < s.length; i++) {
        let c = s.charCodeAt(i);
        if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
          const d = s.charCodeAt(i + 1);
          if (d >= 0xdc00 && d <= 0xdfff) {
            c = 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00);
            i++;
          } else c = 0xfffd;
        } else if (c >= 0xd800 && c <= 0xdfff) c = 0xfffd;
        if (c < 0x80) out[n++] = c;
        else if (c < 0x800) {
          out[n++] = 0xc0 | (c >> 6);
          out[n++] = 0x80 | (c & 63);
        } else if (c < 0x10000) {
          out[n++] = 0xe0 | (c >> 12);
          out[n++] = 0x80 | ((c >> 6) & 63);
          out[n++] = 0x80 | (c & 63);
        } else {
          out[n++] = 0xf0 | (c >> 18);
          out[n++] = 0x80 | ((c >> 12) & 63);
          out[n++] = 0x80 | ((c >> 6) & 63);
          out[n++] = 0x80 | (c & 63);
        }
      }
      return out.slice(0, n);
    }
  }
  // UTF-8 only, non-fatal (malformed sequences decode as U+FFFD), no streaming.
  class TextDecoder {
    constructor(label = "utf-8") {
      if (!/^utf-?8$/i.test(String(label))) throw new RangeError(`TextDecoder: only utf-8 is supported, got ${label}`);
    }
    get encoding() {
      return "utf-8";
    }
    decode(input) {
      if (input === undefined) return "";
      const b = ArrayBuffer.isView(input)
        ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
        : new Uint8Array(input);
      let out = "";
      let i = b[0] === 0xef && b[1] === 0xbb && b[2] === 0xbf ? 3 : 0;
      const cont = (k) => k < b.length && (b[k] & 0xc0) === 0x80;
      while (i < b.length) {
        const x = b[i];
        let c = 0xfffd;
        let len = 1;
        if (x < 0x80) c = x;
        else if (x >= 0xc2 && x < 0xe0 && cont(i + 1)) {
          c = ((x & 31) << 6) | (b[i + 1] & 63);
          len = 2;
        } else if (x >= 0xe0 && x < 0xf0 && cont(i + 1) && cont(i + 2)) {
          c = ((x & 15) << 12) | ((b[i + 1] & 63) << 6) | (b[i + 2] & 63);
          len = 3;
          if (c < 0x800 || (c >= 0xd800 && c <= 0xdfff)) c = 0xfffd;
        } else if (x >= 0xf0 && x < 0xf5 && cont(i + 1) && cont(i + 2) && cont(i + 3)) {
          c = ((x & 7) << 18) | ((b[i + 1] & 63) << 12) | ((b[i + 2] & 63) << 6) | (b[i + 3] & 63);
          len = 4;
          if (c < 0x10000 || c > 0x10ffff) c = 0xfffd;
        }
        out += String.fromCodePoint(c);
        i += len;
      }
      return out;
    }
  }
  globalThis.TextEncoder = TextEncoder;
  globalThis.TextDecoder = TextDecoder;

  // Structured clone of plain data (primitives, plain objects, arrays, Date, RegExp, Map, Set, typed arrays,
  // ArrayBuffer), cycles preserved; functions and symbols throw like the spec's DataCloneError.
  // deno_core's `core.structuredClone` (V8's serializer, this realm) when present; the JS fallback (plain data)
  // serves the bun node:vm tests, whose host clone would hand back host-realm Maps.
  globalThis.structuredClone = core.structuredClone ? (value) => core.structuredClone(value) : (value) => {
    const seen = new Map();
    const clone = (v) => {
      if (typeof v === "function" || typeof v === "symbol") throw new TypeError(`structuredClone: ${typeof v} cannot be cloned`);
      if (v === null || typeof v !== "object") return v;
      if (seen.has(v)) return seen.get(v);
      let out;
      if (v instanceof Date) out = new Date(v.getTime());
      else if (v instanceof RegExp) out = new RegExp(v.source, v.flags);
      else if (v instanceof ArrayBuffer) out = v.slice(0);
      else if (ArrayBuffer.isView(v)) out = new v.constructor(v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength));
      else if (v instanceof Map) {
        out = new Map();
        seen.set(v, out);
        for (const [k, x] of v) out.set(clone(k), clone(x));
        return out;
      } else if (v instanceof Set) {
        out = new Set();
        seen.set(v, out);
        for (const x of v) out.add(clone(x));
        return out;
      } else if (Array.isArray(v)) {
        out = new Array(v.length);
        seen.set(v, out);
        for (let i = 0; i < v.length; i++) if (i in v) out[i] = clone(v[i]);
        return out;
      } else {
        out = {};
        seen.set(v, out);
        for (const k of Object.keys(v)) out[k] = clone(v[k]);
        return out;
      }
      seen.set(v, out);
      return out;
    };
    return clone(value);
  };

  class URL {
    constructor(url) {
      throw new TypeError(`URL is not available in the in-process runtime: ${String(url).slice(0, 80)}`);
    }
    static canParse() {
      return false;
    }
  }
  globalThis.URL = URL;

  // deno_core's bare runtime may or may not install a console; keep one that writes to stderr.
  if (typeof globalThis.console === "undefined") {
    const line = (args) => `${args.map((a) => (typeof a === "string" ? a : (() => { try { return JSON.stringify(a); } catch { return String(a); } })())).join(" ")}\n`;
    const err = (...args) => core.print(line(args), true);
    globalThis.console = { log: err, info: err, debug: err, warn: err, error: err };
  }
})(Deno.core);
