/**
 * SPIKE: serve the embedded bundle over stdio from a bare `node:vm` context (ECMAScript built-ins plus only timers,
 * `crypto` and `atob`), so the parity driver can tell DOM differences (linkedom vs jsdom, same JS engine)
 * from engine differences (the deno_core peer).
 *   bun run scripts/parity-inproc.ts dist/theuth-documents "bun test-inproc/serve-bundle.ts"
 */
import vm from "node:vm";
import { join } from "node:path";

const src = await Bun.file(join(import.meta.dir, "..", "dist", "theuth-documents-inproc.js")).text();
const ctx = vm.createContext({ crypto, atob, setTimeout, clearTimeout, setInterval, clearInterval });
vm.runInContext(src, ctx);
const api = vm.runInContext("globalThis.theuthDocuments", ctx) as { call(line: string): Promise<string> };
const dec = new TextDecoder();
let pending = "";
for await (const chunk of Bun.stdin.stream()) {
  pending += dec.decode(chunk, { stream: true });
  let nl: number;
  while ((nl = pending.indexOf("\n")) !== -1) {
    const line = pending.slice(0, nl).trim();
    pending = pending.slice(nl + 1);
    if (line) process.stdout.write(`${await api.call(line)}\n`);
  }
}
