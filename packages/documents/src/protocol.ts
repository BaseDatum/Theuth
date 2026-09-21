/**
 * Wire types of the documents helper (design 59 §3.2): newline-delimited JSON over stdio,
 * binary fields base64. The Rust side (`crates/document/src/helper.rs`) mirrors these
 * with serde and checks {@link PROTOCOL_VERSION} against the helper's `--version` line.
 */

/** Bumped whenever a request/response shape or an op's semantics change. */
export const PROTOCOL_VERSION = "59.5";

export type ImportMode = "diff" | "replace" | "append";

export type Req =
  /** Load a document from its full state (authority → helper). `fragment` defaults to `document-store`. */
  | { id: number; op: "open"; doc: string; state: string; fragment?: string }
  /** One accepted v1 update (authority → helper). */
  | { id: number; op: "update"; doc: string; update: string }
  | { id: number; op: "close"; doc: string }
  /** §5.1 outline: one line per block, id first, children indented. */
  | { id: number; op: "outline"; doc: string; depth?: number }
  /**
   * Lossy markdown of the whole doc (or of `blocks`, any depth); a leading frontmatter block becomes the
   * YAML header. `annotate` prefixes each top-level body block with `<!-- <id> -->` (§5.1), or
   * `<!-- <id> by <name> -->` for ids in `authors` (§11: member-authored text is labelled).
   */
  | { id: number; op: "markdown"; doc: string; blocks?: string[]; annotate?: boolean; authors?: Record<string, string> }
  /** Markdown → blocks → one Y transaction; `frontmatter: true` maps a leading YAML header to the frontmatter block. */
  | { id: number; op: "import"; doc: string; markdown: string; mode: ImportMode; frontmatter?: boolean }
  /** §5.2 block edits applied in ONE Y transaction; the update comes back as `ev: update` before the reply. */
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
  /** Parse `markdown` (several top-level blocks allowed) and place the blocks; exactly one of after/before/at (default `end`). */
  | { op: "insert"; markdown: string; after?: string; before?: string; at?: "start" | "end" }
  /** Remove `blocks` and put the parsed blocks where the first removed one was. */
  | { op: "replace"; blocks: string[]; markdown: string }
  | { op: "delete"; blocks: string[] }
  /**
   * Move `blocks` (ids kept, so attribution survives — 59 §7.4 regroups a person's notes under headings)
   * after/before a block or to the start/end of the top level; exactly one of after/before/at. The moved
   * ids are not `touched`: nothing was authored.
   */
  | { op: "move"; blocks: string[]; after?: string; before?: string; at?: "start" | "end" }
  /**
   * The inverse the helper computes for §5.5 "Undo last agent edit": remove `remove` (the ids an edit
   * inserted) and put `blocks` (the exact pre-edit block JSON) back at `anchor`. Never built by the model.
   */
  | Restore;

export type Restore = { op: "restore"; remove: string[]; blocks: unknown[]; anchor: Anchor };

/** Where blocks go: after/before a sibling, at the start/end of the top level, or as the first children of `childOf`. */
export type Anchor = { after?: string; before?: string; at?: "start" | "end"; childOf?: string };

/**
 * `edit` result. `touched` = every inserted id (the reveal keys on it); `undo` restores the document
 * exactly (ids included) when passed back as `edits`, one `restore` per applied edit in reverse order.
 */
export type EditResult = { inserted: string[]; removed: string[]; touched: string[]; undo: Restore[] };

/** `edit` failure, serialised as the `error` string (JSON) so the Rust side can name the nearest ids. */
export type EditError =
  | { code: "unknown_block"; id: string; nearest: string[] }
  | { code: "frontmatter_immutable"; id: string }
  | { code: "bad_edit"; message: string };

/** `import` result. */
export type ImportResult = { changed: number; total: number; touched: string[] };

/** `markdown` result. */
export type MarkdownResult = { markdown: string };

/** `outline` result. */
export type OutlineResult = { outline: string; blocks: OutlineBlock[] };

export type OutlineBlock = { id: string; type: string; text: string; children: OutlineBlock[] };
