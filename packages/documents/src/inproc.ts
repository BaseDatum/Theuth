/**
 * The documents helper (design 59 §3.2) as an embeddable, runtime-neutral module (no Node, Bun or Deno APIs),
 * run by the deno_core runtime inside `t` (crates/document-js). Wire: protocol.ts. `call(line)` takes one
 * request line and resolves to the helper's output for it — any `{"ev":"update"}` lines first, then the
 * reply — joined by "\n".
 */
import * as Y from "yjs";
import { EditFailure, edit, importMarkdown, isOurs, markdown, open, outline, type Open } from "./convert.ts";
import { PROTOCOL_VERSION, type Edit, type Req, type Res } from "./protocol.ts";

const docs: Record<string, Open> = {};
let sink: string[] = [];
const write = (r: Res) => sink.push(JSON.stringify(r));

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64_INDEX = new Uint8Array(128);
for (let i = 0; i < B64.length; i++) B64_INDEX[B64.charCodeAt(i)] = i;

function b64(u: Uint8Array): string {
  const parts: string[] = [];
  let chunk = "";
  let i = 0;
  for (; i + 2 < u.length; i += 3) {
    const n = (u[i]! << 16) | (u[i + 1]! << 8) | u[i + 2]!;
    chunk += B64[n >> 18]! + B64[(n >> 12) & 63]! + B64[(n >> 6) & 63]! + B64[n & 63]!;
    if (chunk.length > 8192) {
      parts.push(chunk);
      chunk = "";
    }
  }
  if (i < u.length) {
    const n = (u[i]! << 16) | ((u[i + 1] ?? 0) << 8);
    chunk += B64[n >> 18]! + B64[(n >> 12) & 63]! + (i + 1 < u.length ? B64[(n >> 6) & 63]! : "=") + "=";
  }
  parts.push(chunk);
  return parts.join("");
}

function unb64(s: string): Uint8Array {
  const clean = s.replace(/[^A-Za-z0-9+/]/g, "");
  const out = new Uint8Array(Math.floor((clean.length * 3) / 4));
  let o = 0;
  for (let i = 0; i < clean.length; i += 4) {
    const n =
      (B64_INDEX[clean.charCodeAt(i)]! << 18) |
      (B64_INDEX[clean.charCodeAt(i + 1)]! << 12) |
      ((i + 2 < clean.length ? B64_INDEX[clean.charCodeAt(i + 2)]! : 0) << 6) |
      (i + 3 < clean.length ? B64_INDEX[clean.charCodeAt(i + 3)]! : 0);
    out[o++] = n >> 16;
    if (i + 2 < clean.length) out[o++] = (n >> 8) & 255;
    if (i + 3 < clean.length) out[o++] = n & 255;
  }
  return out.subarray(0, o);
}

type Bad = { id: number; ok: false; error: string };

function parseRequest(line: string): Req | Bad {
  let raw: unknown;
  try {
    raw = JSON.parse(line);
  } catch (e) {
    return { id: 0, ok: false, error: `invalid JSON: ${e instanceof Error ? e.message : String(e)}` };
  }
  if (typeof raw !== "object" || raw === null) return { id: 0, ok: false, error: "request must be a JSON object" };
  const id = "id" in raw && typeof raw.id === "number" ? raw.id : null;
  if (id === null) return { id: 0, ok: false, error: "missing numeric field id" };
  const op = "op" in raw ? raw.op : undefined;
  const doc = "doc" in raw && typeof raw.doc === "string" ? raw.doc : null;
  if (doc === null) return { id, ok: false, error: "missing string field doc" };
  const fields = raw as Record<string, unknown>;
  const str = (k: string): string | null => (typeof fields[k] === "string" ? (fields[k] as string) : null);
  switch (op) {
    case "open": {
      const state = str("state");
      if (state === null) return { id, ok: false, error: "open requires string field state" };
      return { id, op, doc, state, fragment: str("fragment") ?? undefined };
    }
    case "update": {
      const update = str("update");
      if (update === null) return { id, ok: false, error: "update requires string field update" };
      return { id, op, doc, update };
    }
    case "close":
      return { id, op, doc };
    case "outline": {
      const depth = "depth" in raw && typeof raw.depth === "number" ? raw.depth : undefined;
      return { id, op, doc, depth };
    }
    case "markdown": {
      const blocks = "blocks" in raw ? raw.blocks : undefined;
      if (blocks !== undefined && !(Array.isArray(blocks) && blocks.every((b) => typeof b === "string")))
        return { id, ok: false, error: "blocks must be an array of block ids" };
      const annotate = "annotate" in raw && typeof raw.annotate === "boolean" ? raw.annotate : undefined;
      const authors = "authors" in raw && typeof raw.authors === "object" && raw.authors !== null ? (raw.authors as Record<string, string>) : undefined;
      return { id, op, doc, blocks: blocks as string[] | undefined, annotate, authors };
    }
    case "import": {
      const md = str("markdown");
      const mode = str("mode");
      if (md === null) return { id, ok: false, error: "import requires string field markdown" };
      if (mode !== "diff" && mode !== "replace" && mode !== "append") return { id, ok: false, error: 'mode must be "diff" | "replace" | "append"' };
      const frontmatter = "frontmatter" in raw && typeof raw.frontmatter === "boolean" ? raw.frontmatter : undefined;
      return { id, op, doc, markdown: md, mode, frontmatter };
    }
    case "edit": {
      const edits = "edits" in raw ? raw.edits : undefined;
      if (!Array.isArray(edits) || !edits.every((e) => typeof e === "object" && e !== null && "op" in e && typeof e.op === "string"))
        return { id, ok: false, error: "edits must be an array of {op, …} objects" };
      return { id, op, doc, edits: edits as Edit[] };
    }
    default:
      return { id, ok: false, error: `unknown op ${JSON.stringify(op)}` };
  }
}

let current = 0;

async function handle(req: Req): Promise<Res> {
  const { id, doc } = req;
  current = id;
  try {
    if (req.op === "open") {
      docs[doc]?.ydoc.destroy();
      const o = open(unb64(req.state), req.fragment);
      o.ydoc.on("update", (update: Uint8Array, origin: unknown) => {
        if (isOurs(origin)) write({ ev: "update", doc, update: b64(update), touched: origin.touched, req: current, changed: origin.changed, total: origin.total });
      });
      docs[doc] = o;
      return { id, ok: true };
    }
    const o = docs[doc];
    if (!o) return { id, ok: false, error: `document ${doc} is not open` };
    switch (req.op) {
      case "update":
        Y.applyUpdate(o.ydoc, unb64(req.update), "authority");
        return { id, ok: true };
      case "close":
        o.ydoc.destroy();
        delete docs[doc];
        return { id, ok: true };
      case "outline":
        return { id, ok: true, result: outline(o, req.depth) };
      case "markdown":
        return { id, ok: true, result: { markdown: await markdown(o, req.blocks, req.annotate ?? false, req.authors ?? {}) } };
      case "import":
        return { id, ok: true, result: await importMarkdown(o, req.markdown, req.mode, req.frontmatter ?? false) };
      case "edit":
        return { id, ok: true, result: await edit(o, req.edits) };
    }
  } catch (e) {
    if (e instanceof EditFailure) return { id, ok: false, error: e.message };
    return { id, ok: false, error: e instanceof Error ? (e.stack ?? e.message) : String(e) };
  }
}

/** One request line in, the helper's output lines for it out (events first, reply last), "\n"-joined. */
export async function call(line: string): Promise<string> {
  sink = [];
  const req = parseRequest(line);
  write("ok" in req ? req : await handle(req));
  const out = sink.join("\n");
  sink = [];
  return out;
}

export const version = `theuth-documents ${PROTOCOL_VERSION}`;

// The embedder (crates/document-js) reaches the module through this one global; a classic script has no exports.
const host = globalThis as { theuthDocuments?: { call: typeof call; version: string } };
host.theuthDocuments = { call, version };
