/**
 * Wire types of the documents helper (design 59 §3.2): newline-delimited JSON over stdio,
 * binary fields base64. The Rust side (`crates/document/src/helper.rs`) mirrors these
 * with serde and checks {@link PROTOCOL_VERSION} against the helper's `--version` line.
 */

/** Bumped whenever a request/response shape or an op's semantics change. */
export const PROTOCOL_VERSION = "59.1";

export type ImportMode = "diff" | "replace" | "append";

export type Req =
  /** Load a document from its full state (authority → helper). `fragment` defaults to `document-store`. */
  | { id: number; op: "open"; doc: string; state: string; fragment?: string }
  /** One accepted v1 update (authority → helper). */
  | { id: number; op: "update"; doc: string; update: string }
  | { id: number; op: "close"; doc: string }
  /** §5.1 outline: one line per block, id first, children indented. */
  | { id: number; op: "outline"; doc: string; depth?: number }
  /** Lossy markdown of the whole doc (or of `blocks`); a leading frontmatter block becomes the YAML header. */
  | { id: number; op: "markdown"; doc: string; blocks?: string[] }
  /** Markdown → blocks → one Y transaction; `frontmatter: true` maps a leading YAML header to the frontmatter block. */
  | { id: number; op: "import"; doc: string; markdown: string; mode: ImportMode; frontmatter?: boolean }
  /** §5.2 block edits, one Y transaction. Step 2: the helper answers `unimplemented` for now. */
  | { id: number; op: "edit"; doc: string; edits: Edit[] };

export type Res =
  | { id: number; ok: true; result?: unknown }
  | { id: number; ok: false; error: string }
  /**
   * Helper → authority after `import`/`edit`, written before that request's reply: the transaction's
   * update, the block ids it touched, the request it belongs to, and for an import how many body blocks
   * changed of how many — so the authority can snapshot *before* applying a large diff (§4.6 `import-file`).
   */
  | { ev: "update"; doc: string; update: string; touched: string[]; req: number; changed?: number; total?: number };

export type Edit =
  | { op: "insert"; markdown: string; after?: string; before?: string; at?: "start" | "end" }
  | { op: "replace"; blocks: string[]; markdown: string }
  | { op: "delete"; blocks: string[] };

/** `import` result. */
export type ImportResult = { changed: number; total: number; touched: string[] };

/** `markdown` result. */
export type MarkdownResult = { markdown: string };

/** `outline` result. */
export type OutlineResult = { outline: string; blocks: OutlineBlock[] };

export type OutlineBlock = { id: string; type: string; text: string; children: OutlineBlock[] };
