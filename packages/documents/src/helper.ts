/**
 * `theuth-documents`: the headless BlockNote helper (design 59 §3.2).
 *
 * JSON lines on stdin → one JSON line per request on stdout, in order; `{"ev":"update"}`
 * lines are pushed whenever an `import`/`edit` transaction lands. Logs go to stderr only.
 * One `Y.Doc` per open document, a mirror of the harness authority: every update the
 * authority accepts arrives as `update`; our own transactions go back as `ev: update` and
 * the authority applies them like any client's. Nothing here touches disk.
 *
 *   theuth-documents --version      → "theuth-documents <protocol version>"
 *   printf '' | theuth-documents    → exit 0 on EOF
 */
import * as Y from "yjs";
import { EditFailure, edit, importMarkdown, isOurs, markdown, open, outline, type Open } from "./convert.ts";
import { PROTOCOL_VERSION, type Edit, type Req, type Res } from "./protocol.ts";

/** Largest request line accepted; the Rust side refuses longer ones before sending. */
export const MAX_LINE_BYTES = 16 * 1024 * 1024;

const docs: Record<string, Open> = {};
const write = (r: Res) => process.stdout.write(`${JSON.stringify(r)}\n`);
const b64 = (u: Uint8Array) => Buffer.from(u).toString("base64");

function parseRequest(line: string): Req | { id: number; ok: false; error: string } {
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
  const str = (k: string): string | null => (k in raw && typeof (raw as Record<string, unknown>)[k] === "string" ? ((raw as Record<string, unknown>)[k] as string) : null);
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
      // Shapes past `op` are checked by `edit` itself, which reports a structured error.
      return { id, op, doc, edits: edits as Edit[] };
    }
    default:
      return { id, ok: false, error: `unknown op ${JSON.stringify(op)}` };
  }
}

/** Id of the request being served; stamped on the update events it causes. */
let current = 0;

/** Serve one request. Throws never: every failure is an error line. */
export async function handle(req: Req): Promise<Res> {
  const { id, doc } = req;
  current = id;
  try {
    if (req.op === "open") {
      docs[doc]?.ydoc.destroy();
      const o = open(Buffer.from(req.state, "base64"), req.fragment);
      // Our own transactions travel back to the authority; theirs (origin "authority") do not.
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
        Y.applyUpdate(o.ydoc, Buffer.from(req.update, "base64"), "authority");
        return { id, ok: true };
      case "close":
        o.ydoc.destroy();
        delete docs[doc];
        return { id, ok: true };
      case "outline":
        return { id, ok: true, result: outline(o, req.depth) };
      case "markdown":
        return { id, ok: true, result: { markdown: await markdown(o, req.blocks, req.annotate ?? false, req.authors ?? {}) } };
      case "import": {
        const r = await importMarkdown(o, req.markdown, req.mode, req.frontmatter ?? false);
        return { id, ok: true, result: r };
      }
      case "edit":
        return { id, ok: true, result: await edit(o, req.edits) };
    }
  } catch (e) {
    // A structured edit failure travels as JSON in `error`; the Rust side decodes it for the model.
    if (e instanceof EditFailure) return { id, ok: false, error: e.message };
    return { id, ok: false, error: e instanceof Error ? (e.stack ?? e.message) : String(e) };
  }
}

// `import.meta.main` is false on Windows in an executable compiled through the `Bun.build()` API (Bun 1.4.2;
// the CLI `bun build --compile` says true) — the binary then read stdin forever and `--version` printed nothing.
// A compiled binary is always the entrypoint: Bun.main equals import.meta.path up to slash direction
// (`B:/~BUN/root/x` vs `B:\\~BUN\\root\\x`), and argv[1] carries the virtual root (`/$bunfs/` or `~BUN`).
const slashes = (p: string) => p.replaceAll("\\", "/");
const isMain = import.meta.main || slashes(Bun.main) === slashes(import.meta.path) || /\$bunfs|~BUN/.test(process.argv[1] ?? "");
if (isMain) {
  const argv = process.argv.slice(2);
  if (argv.includes("--version") || argv.includes("-V")) {
    process.stdout.write(`theuth-documents ${PROTOCOL_VERSION}\n`);
    process.exit(0);
  }
  if (argv.length) {
    process.stderr.write(`theuth-documents: unknown arguments ${argv.join(" ")} (stdin JSON lines only; --version)\n`);
    process.exit(2);
  }

  const decoder = new TextDecoder();
  let pending = "";
  let pendingBytes = 0;
  let discarding = false;
  for await (const chunk of Bun.stdin.stream()) {
    if (discarding) {
      const nl = chunk.indexOf(10);
      if (nl === -1) continue;
      discarding = false;
      pending = decoder.decode(chunk.subarray(nl + 1), { stream: true });
      pendingBytes = chunk.byteLength - nl - 1;
    } else {
      pending += decoder.decode(chunk, { stream: true });
      pendingBytes += chunk.byteLength;
    }
    let nl: number;
    while ((nl = pending.indexOf("\n")) !== -1) {
      const line = pending.slice(0, nl).trim();
      pending = pending.slice(nl + 1);
      pendingBytes = new TextEncoder().encode(pending).byteLength;
      if (!line) continue;
      const req = parseRequest(line);
      write("ok" in req ? req : await handle(req));
    }
    if (pendingBytes > MAX_LINE_BYTES) {
      write({ id: 0, ok: false, error: `request line exceeds ${MAX_LINE_BYTES} bytes` });
      pending = "";
      pendingBytes = 0;
      discarding = true;
    }
  }
  const tail = pending.trim();
  if (!discarding && tail) {
    const req = parseRequest(tail);
    write("ok" in req ? req : await handle(req));
  }
}
