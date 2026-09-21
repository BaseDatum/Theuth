/**
 * Markdown ⇄ blocks ⇄ Y.XmlFragment, on top of BlockNote's server editor (design 59 §3.2,
 * §4.3–§4.5, §5.1). Pure of I/O: `helper.ts` owns stdio, this owns the conversions and the
 * one-transaction import. Everything here is exercised by `test/*.test.ts` without a process.
 */
import { ServerBlockNoteEditor } from "@blocknote/server-util";
import * as Y from "yjs";
import type { ImportMode, ImportResult, OutlineBlock } from "./protocol.ts";
import { type DocBlock, type DocumentSchema, FRAGMENT, schema } from "./schema.ts";
import { emitHeader, get, parseHeader, splitFrontmatter } from "./yaml.ts";

/** Transaction origin of every change this module makes; the helper forwards only these (§3.2 `ev: update`). */
export type Origin = { helper: true; touched: string[]; changed?: number; total?: number };
export const isOurs = (origin: unknown): origin is Origin =>
  typeof origin === "object" && origin !== null && "helper" in origin && origin.helper === true;

/** The one headless editor: schema-aware conversions, no view, no persistence. */
export const editor = ServerBlockNoteEditor.create({ schema });

type Editor = ServerBlockNoteEditor<
  DocumentSchema["blockSchema"],
  DocumentSchema["inlineContentSchema"],
  DocumentSchema["styleSchema"]
>;
const ed = editor as Editor;

/** One open document mirrored from the authority. */
export type Open = { ydoc: Y.Doc; fragment: Y.XmlFragment };

export function open(state: Uint8Array, fragment = FRAGMENT): Open {
  const ydoc = new Y.Doc();
  if (state.length > 0) Y.applyUpdate(ydoc, state, "authority");
  return { ydoc, fragment: ydoc.getXmlFragment(fragment) };
}

/** Current top-level blocks; an empty fragment is an empty document. */
export function blocks(o: Open): DocBlock[] {
  if (o.fragment.length === 0) return [];
  return ed.yXmlFragmentToBlocks(o.fragment);
}

type FmBlock = Extract<DocBlock, { type: "frontmatter" }>;
const isFrontmatter = (b: DocBlock): b is FmBlock => b.type === "frontmatter";

/** The frontmatter block's YAML header body, when the document has one. */
export function frontmatterOf(bs: DocBlock[]): string | null {
  const fm = bs[0];
  return fm && isFrontmatter(fm) ? fm.props.yaml : null;
}

/**
 * Lossy markdown of the document (or of `only` these top-level block ids). A leading
 * frontmatter block becomes the `---` YAML header (§4.5); it is never handed to BlockNote's
 * exporter, which would render it as nothing (verified: no throw, empty output).
 */
export async function markdown(o: Open, only?: string[]): Promise<string> {
  const bs = blocks(o);
  const yaml = frontmatterOf(bs);
  let body = yaml === null ? bs : bs.slice(1);
  if (only) {
    const wanted: Record<string, true> = Object.fromEntries(only.map((id) => [id, true]));
    body = body.filter((b) => wanted[b.id]);
  }
  const md = body.length === 0 ? "" : await ed.blocksToMarkdownLossy(body);
  if (only || yaml === null) return md;
  return `---\n${yaml.endsWith("\n") || yaml === "" ? yaml : `${yaml}\n`}---\n${md}`;
}
/** Whitespace-insensitive markdown keys for LCS alignment (§4.4). */
async function blockKeys(bs: DocBlock[]): Promise<string[]> {
  return Promise.all(bs.map(async (b) => (await ed.blocksToMarkdownLossy([b])).replace(/\s+/g, " ").trim()));
}

/** Longest common subsequence over equal keys → pairs of (old index, new index). */
export function lcs(a: string[], b: string[]): [number, number][] {
  const n = a.length;
  const m = b.length;
  const dp: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i]![j] = a[i] === b[j] ? dp[i + 1]![j + 1]! + 1 : Math.max(dp[i + 1]![j]!, dp[i]![j + 1]!);
    }
  }
  const out: [number, number][] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push([i, j]);
      i++;
      j++;
    } else if (dp[i + 1]![j]! >= dp[i]![j + 1]!) i++;
    else j++;
  }
  return out;
}

function emptyParagraph(): DocBlock {
  return { id: crypto.randomUUID(), type: "paragraph", props: { backgroundColor: "default", textColor: "default", textAlignment: "left" }, content: [], children: [] } as DocBlock;
}

function frontmatterBlock(yaml: string, id: string = crypto.randomUUID()): DocBlock {
  return { id, type: "frontmatter", props: { yaml }, content: undefined, children: [] } as DocBlock;
}

/**
 * Import `text` (§4.4): parse the body to blocks, then in ONE transaction (origin {@link ORIGIN})
 * rewrite the fragment so unchanged blocks keep their ids and formatting (y-prosemirror's
 * structural diff leaves equal nodes untouched). Returns how many body blocks changed.
 */
export async function importMarkdown(o: Open, text: string, mode: ImportMode, frontmatter: boolean): Promise<ImportResult> {
  const old = blocks(o);
  const first = old[0];
  const oldFm = first && isFrontmatter(first) ? first : null;
  const oldBody = oldFm ? old.slice(1) : old;

  const split = frontmatter ? splitFrontmatter(text) : { header: null, body: text };
  const parsed = split.body.trim() === "" ? [] : await ed.tryParseMarkdownToBlocks(split.body);
  // The parser never yields a frontmatter block; a stray one from a hand-built document is dropped.
  const fresh = parsed.filter((b) => !isFrontmatter(b));

  const touched: string[] = [];
  let body: DocBlock[];
  let changed = 0;
  if (mode === "replace" || oldBody.length === 0) {
    body = fresh;
    changed = fresh.length;
  } else if (mode === "append") {
    body = [...oldBody, ...fresh];
    changed = fresh.length;
  } else {
    const [oldKeys, newKeys] = await Promise.all([blockKeys(oldBody), blockKeys(fresh)]);
    const pairs = lcs(oldKeys, newKeys);
    const keep: Record<number, DocBlock> = {};
    for (const [oi, ni] of pairs) keep[ni] = oldBody[oi]!;
    body = fresh.map((b, ni) => keep[ni] ?? b);
    changed = fresh.length - pairs.length;
  }
  for (const b of body) if (!oldBody.some((ob) => ob.id === b.id)) touched.push(b.id);
  if (body.length === 0) {
    const p = emptyParagraph();
    body = [p];
    touched.push(p.id);
  }

  // Frontmatter (§4.5): the header maps onto the one block, keeping its id; no header keeps the block as is.
  let fm: DocBlock | null = oldFm;
  if (frontmatter) {
    const header = split.header ?? "";
    if (oldFm === null) {
      fm = frontmatterBlock(header);
      touched.push(fm.id);
    } else if (normalizeHeader(oldFm.props.yaml) !== normalizeHeader(header)) {
      fm = frontmatterBlock(header, oldFm.id);
      touched.push(oldFm.id);
    }
  }

  const next = fm ? [fm, ...body] : body;
  const origin: Origin = { helper: true, touched, changed, total: fresh.length };
  o.ydoc.transact(() => {
    ed.blocksToYXmlFragment(next, o.fragment);
  }, origin);
  return { changed, total: fresh.length, touched };
}

/** Header equality up to formatting: parsed entries re-emitted. */
function normalizeHeader(yaml: string): string {
  return emitHeader(parseHeader(yaml));
}

/** Plain text of a block's inline content. */
export function textOf(b: DocBlock): string {
  const c: unknown = b.content;
  if (!c) return "";
  if (Array.isArray(c)) return c.map(inlineText).join("");
  if (typeof c === "object" && "type" in c && c.type === "tableContent" && "rows" in c && Array.isArray(c.rows)) {
    const rows = c.rows as { cells: unknown[] }[];
    const first = rows[0]?.cells ?? [];
    const cols = first.length;
    const head = first.map((cell) => cellText(cell)).join(" | ");
    return `[table ${rows.length}×${cols}] ${head}`;
  }
  return "";
}

function cellText(cell: unknown): string {
  if (Array.isArray(cell)) return cell.map(inlineText).join("");
  if (cell && typeof cell === "object" && "content" in cell && Array.isArray(cell.content)) return cell.content.map(inlineText).join("");
  return "";
}

function inlineText(node: unknown): string {
  if (!node || typeof node !== "object") return "";
  if ("text" in node && typeof node.text === "string") return node.text;
  if ("content" in node && Array.isArray(node.content)) return node.content.map(inlineText).join("");
  return "";
}

const TRUNCATE = 120;

/** §5.1 tag for a block type; paragraphs carry none, bullets a `•`. */
function tag(b: DocBlock): string {
  switch (b.type) {
    case "paragraph":
      return "";
    case "heading":
      return `[h${String(b.props.level)}] `;
    case "bulletListItem":
    case "toggleListItem":
      return "• ";
    case "numberedListItem":
      return "1. ";
    case "checkListItem":
      return b.props.checked ? "[x] " : "[ ] ";
    case "codeBlock":
      return `[code${b.props.language ? `:${String(b.props.language)}` : ""}] `;
    case "table":
      return "";
    default:
      return `[${b.type}] `;
  }
}

function outlineLine(b: DocBlock): string {
  if (b.type === "codeBlock") {
    const lines = textOf(b).split("\n");
    return `${tag(b)}${lines[0] ?? ""}${lines.length > 1 ? ` … (${lines.length} lines)` : ""}`;
  }
  if (b.type === "table") return textOf(b);
  const t = textOf(b);
  return `${tag(b)}${t.length > TRUNCATE ? `${t.slice(0, TRUNCATE)}…` : t}`;
}

/** The compact addressable view (§5.1). */
export function outline(o: Open, depth = Infinity): { outline: string; blocks: OutlineBlock[] } {
  const bs = blocks(o);
  const lines: string[] = [];
  const tree: OutlineBlock[] = [];
  const walk = (list: DocBlock[], level: number, into: OutlineBlock[]) => {
    for (const b of list) {
      if (isFrontmatter(b)) {
        const entries = parseHeader(b.props.yaml);
        const parts = ["type", "title", "tags"]
          .map((k) => [k, get(entries, k)] as const)
          .filter((kv): kv is readonly [string, string] => kv[1] !== null && kv[1] !== "")
          .map(([k, v]) => (k === "title" ? `title="${v}"` : `${k}=${v.replace(/, /g, ",")}`));
        lines.push(`[fm] ${parts.join(" ")}`);
        into.push({ id: b.id, type: "frontmatter", text: parts.join(" "), children: [] });
        continue;
      }
      const line = outlineLine(b);
      lines.push(`${"  ".repeat(level)}${b.id} ${line}`);
      const node: OutlineBlock = { id: b.id, type: b.type, text: line, children: [] };
      into.push(node);
      if (level + 1 < depth && b.children.length > 0) walk(b.children as DocBlock[], level + 1, node.children);
    }
  };
  walk(bs, 0, tree);
  return { outline: lines.join("\n"), blocks: tree };
}
