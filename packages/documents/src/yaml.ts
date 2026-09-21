/**
 * The YAML subset a knowledge header uses (design 10 §2.2, 59 §4.5): one flat mapping of
 * scalars and string lists. Mirrors `okf`'s emitter (plain-or-double-quoted scalars, block
 * sequences) so a header the store wrote and one this code wrote read the same. Anything
 * outside the subset (a nested mapping, a multi-line scalar) is kept as raw lines under its
 * key and written back verbatim: unknown keys survive, as the store guarantees (10 §2.2).
 *
 * No dependencies: imported by the helper and the desktop UI alike.
 */

/** One header entry. `raw` carries lines the subset does not model, verbatim. */
export type Entry =
  | { key: string; value: string }
  | { key: string; list: string[] }
  | { key: string; raw: string[] };

/** Parse a header body (the text between the `---` fences). Order preserved, later duplicates win. */
export function parseHeader(text: string): Entry[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const out: Entry[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    i++;
    if (line.trim() === "" || line.trimStart().startsWith("#")) continue;
    const m = /^([^\s:#][^:]*?)\s*:(?:\s+(.*)|\s*)$/.exec(line);
    if (!m || line.startsWith(" ")) {
      // Not a top-level `key:` line: attach to the previous entry as raw, or drop.
      const prev = out[out.length - 1];
      if (prev && "raw" in prev) prev.raw.push(line);
      continue;
    }
    const key = m[1]!.trim();
    const rest = (m[2] ?? "").trim();
    // Collect indented continuation lines (block sequence or nested mapping).
    const block: string[] = [];
    while (i < lines.length && (lines[i]!.startsWith(" ") || lines[i]!.startsWith("-") || lines[i]!.trim() === "")) {
      if (lines[i]!.trim() === "" && !(i + 1 < lines.length && lines[i + 1]!.startsWith(" "))) break;
      if (lines[i]!.startsWith("-") && !lines[i]!.startsWith("- ")) break;
      block.push(lines[i]!);
      i++;
    }
    const entry = interpret(key, rest, block);
    const at = out.findIndex((e) => e.key === key);
    if (at === -1) out.push(entry);
    else out[at] = entry;
  }
  return out;
}

function interpret(key: string, rest: string, block: string[]): Entry {
  const body = block.filter((l) => l.trim() !== "");
  if (rest === "" && body.length > 0) {
    const items: string[] = [];
    for (const l of body) {
      const m = /^\s*-\s*(.*)$/.exec(l);
      if (!m) return { key, raw: [`${key}:`, ...block] };
      items.push(scalar(m[1]!));
    }
    return { key, list: items };
  }
  if (body.length > 0) return { key, raw: [`${key}: ${rest}`, ...block] };
  if (rest.startsWith("[")) {
    const inner = rest.slice(1, rest.lastIndexOf("]"));
    if (rest.endsWith("]")) return { key, list: splitFlow(inner).map(scalar) };
  }
  return { key, value: scalar(rest) };
}

/** Split `a, "b, c", d` on top-level commas. */
function splitFlow(s: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quote: string | null = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i]!;
    if (quote) {
      cur += c;
      if (c === "\\" && quote === '"' && i + 1 < s.length) cur += s[++i]!;
      else if (c === quote) quote = null;
    } else if (c === '"' || c === "'") {
      quote = c;
      cur += c;
    } else if (c === ",") {
      out.push(cur);
      cur = "";
    } else cur += c;
  }
  if (cur.trim() !== "" || out.length > 0) out.push(cur);
  return out.map((t) => t.trim()).filter((t) => t !== "");
}

/** Unquote one scalar token. Plain scalars are strings here (`null`/`true`/`3` stay as typed text). */
export function scalar(token: string): string {
  const t = token.trim();
  if (t.startsWith('"') && t.endsWith('"') && t.length >= 2) {
    return t
      .slice(1, -1)
      .replace(/\\(u[0-9a-fA-F]{4}|.)/g, (_, e: string) =>
        e === "n" ? "\n" : e === "t" ? "\t" : e === "r" ? "\r" : e === "0" ? "\0" : e.startsWith("u") ? String.fromCharCode(parseInt(e.slice(1), 16)) : e,
      );
  }
  if (t.startsWith("'") && t.endsWith("'") && t.length >= 2) return t.slice(1, -1).replace(/''/g, "'");
  const hash = / #/.exec(t);
  return (hash ? t.slice(0, hash.index) : t).trim();
}

const INDICATORS: Record<string, true> = Object.fromEntries("-?:,[]{}#&*!|>'\"%@` ".split("").map((c) => [c, true]));
const RESERVED = /^(~|null|Null|NULL|true|True|TRUE|false|False|FALSE|\.inf|-\.inf|\.nan|-?(0|[1-9][0-9]*)(\.[0-9]+)?)$/;

/** Emit one scalar the way okf does: plain when it re-reads as the same string, else double-quoted. */
export function emitScalar(s: string): string {
  if (s === "" || RESERVED.test(s) || s.startsWith(" ") || s.endsWith(" ") || INDICATORS[s[0]!]) return quote(s);
  for (let i = 0; i < s.length; i++) {
    const c = s[i]!;
    if (c === "\n" || c === "\t" || c === "\r") return quote(s);
    if (c === ":" && (i + 1 >= s.length || s[i + 1] === " ")) return quote(s);
    if (c === "#" && i > 0 && s[i - 1] === " ") return quote(s);
  }
  return s;
}

function quote(s: string): string {
  let out = '"';
  for (const c of s) {
    const code = c.codePointAt(0)!;
    if (c === "\\") out += "\\\\";
    else if (c === '"') out += '\\"';
    else if (c === "\n") out += "\\n";
    else if (c === "\t") out += "\\t";
    else if (c === "\r") out += "\\r";
    else if (code < 0x20) out += `\\u${code.toString(16).padStart(4, "0")}`;
    else out += c;
  }
  return `${out}"`;
}

/** Serialise entries as a header body (no fences; ends with a newline when non-empty). */
export function emitHeader(entries: Entry[]): string {
  let out = "";
  for (const e of entries) {
    if ("raw" in e) out += `${e.raw.join("\n")}\n`;
    else if ("list" in e) out += e.list.length === 0 ? `${e.key}: []\n` : `${e.key}:\n${e.list.map((v) => `  - ${emitScalar(v)}`).join("\n")}\n`;
    else out += `${e.key}: ${emitScalar(e.value)}\n`;
  }
  return out;
}

/** Split a concept file at its leading `---` block: header body (without fences) and the rest. */
export function splitFrontmatter(text: string): { header: string | null; body: string } {
  const m = /^---\r?\n([\s\S]*?)(?:\r?\n)?---(?:\r?\n|$)/.exec(text);
  if (!m) return { header: null, body: text };
  return { header: m[1] ?? "", body: text.slice(m[0].length) };
}

/** The value of `key` as text: a list joins with `, `; raw entries are `null`. */
export function get(entries: Entry[], key: string): string | null {
  const e = entries.find((x) => x.key === key);
  if (!e || "raw" in e) return null;
  return "list" in e ? e.list.join(", ") : e.value;
}

/** Return entries with `key` set (appended when new). `undefined` removes the key. */
export function set(entries: Entry[], key: string, value: string | string[] | undefined): Entry[] {
  const next: Entry = Array.isArray(value) ? { key, list: value } : { key, value: value ?? "" };
  const at = entries.findIndex((e) => e.key === key);
  if (value === undefined) return at === -1 ? entries : entries.filter((_, i) => i !== at);
  if (at === -1) return [...entries, next];
  return entries.map((e, i) => (i === at ? next : e));
}
