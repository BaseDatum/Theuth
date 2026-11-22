// The web globals every in-process bundle needs that a bare deno_core runtime does not have (crates/js-runtime;
// the app bundles also load polyfill-web.js after it). Measured for DOCUMENTS
// (packages/documents: vm probe + parity run): `crypto.getRandomValues`/`randomUUID` (Y.Doc client
// ids, block ids), `atob` (htmlparser2's entity tables, at load), and `setTimeout`/`clearTimeout`/
// `setInterval`/`clearInterval` (referenced at load by lib0's eventloop; never called on the helper's paths).
// Everything else the bundle touches is ECMAScript or `typeof`-probed with a fallback.
((core) => {
  const ops = core.ops;

  function getRandomValues(view) {
    ops.op_jsrt_random_fill(new Uint8Array(view.buffer, view.byteOffset, view.byteLength));
    return view;
  }
  const HEX = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, "0"));
  function randomUUID() {
    const b = getRandomValues(new Uint8Array(16));
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    const h = Array.from(b, (x) => HEX[x]);
    return `${h.slice(0, 4).join("")}-${h.slice(4, 6).join("")}-${h.slice(6, 8).join("")}-${h.slice(8, 10).join("")}-${h.slice(10).join("")}`;
  }
  globalThis.crypto = { getRandomValues, randomUUID };

  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const IDX = new Map(Array.from(B64, (c, i) => [c, i]));
  globalThis.atob = (input) => {
    const s = String(input).replace(/[\t\n\f\r ]+/g, "").replace(/={1,2}$/, "");
    let out = "";
    let bits = 0;
    let acc = 0;
    for (const c of s) {
      const v = IDX.get(c);
      if (v === undefined) throw new Error("atob: invalid character");
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out += String.fromCharCode((acc >> bits) & 0xff);
      }
    }
    return out;
  };
  globalThis.btoa = (input) => {
    const s = String(input);
    let out = "";
    for (let i = 0; i < s.length; i += 3) {
      const a = s.charCodeAt(i);
      const b = s.charCodeAt(i + 1);
      const c = s.charCodeAt(i + 2);
      if (a > 255 || b > 255 || c > 255) throw new Error("btoa: character out of range");
      const n = (a << 16) | ((b || 0) << 8) | (c || 0);
      out += B64[n >> 18] + B64[(n >> 12) & 63] + (i + 1 < s.length ? B64[(n >> 6) & 63] : "=") + (i + 2 < s.length ? B64[n & 63] : "=");
    }
    return out;
  };

  const timers = new Map();
  let nextId = 1;
  const arm = (repeat) => (cb, ms, ...args) => {
    const id = nextId++;
    const run = () => {
      if (!repeat) timers.delete(id);
      cb(...args);
    };
    timers.set(id, core.createTimer(run, ms, undefined, repeat, true));
    return id;
  };
  const disarm = (id) => {
    const t = timers.get(id);
    if (t) {
      core.cancelTimer(t);
      timers.delete(id);
    }
  };
  globalThis.setTimeout = arm(false);
  globalThis.setInterval = arm(true);
  globalThis.clearTimeout = disarm;
  globalThis.clearInterval = disarm;
})(Deno.core);
