/**
 * Wire contract shared by the headless helper (`wth-app-parser`, JSON lines over stdio),
 * the Rust app plugin and the desktop adapter. Field names are snake_case on the wire.
 *
 * Never carries a render tree: only source, diagnostics and statement/tool metadata.
 */

/** Visible tool snapshot the harness passes for binding validation. */
export interface ToolSnapshot {
  /** Registry tool name as referenced by `Query("name", …)` / `Mutation("name", …)`. */
  name: string;
  /** JSON Schema of the tool input (MCP `inputSchema` shape); `null`/`{}` when unknown. */
  schema: unknown;
  /** Harness risk classification, transported opaquely (e.g. "read_only", "mutating", "destructive"). */
  risk: string;
  /** Identity fingerprint (native identity or MCP server identity + tool schema hash). */
  identity: string;
}

export type ParserOp = "check" | "merge_check";

export interface ParserRequest {
  id: string;
  op: ParserOp;
  /** `check`: complete source. `merge_check`: the patch (statements; `name = null` deletes). */
  source: string;
  /** `merge_check` only: committed base source. */
  base?: string;
  tools: ToolSnapshot[];
}

export type StatementKind = "value" | "state" | "query" | "mutation";

export interface StatementInfo {
  id: string;
  kind: StatementKind;
}

export interface Binding {
  statement_id: string;
  tool: string;
  kind: "query" | "mutation";
  /** Copied from the matching `ToolSnapshot` so the artifact metadata pins what was validated. */
  risk: string;
  identity: string;
}

export interface Diagnostic {
  /** Statement id the problem is located at; "" when it applies to the whole program. */
  at: string;
  message: string;
  /** Machine-readable class; upstream parser codes plus the adapter codes listed in `DiagnosticCode`. */
  code: DiagnosticCode;
}

export type DiagnosticCode =
  // upstream ValidationErrorCode
  | "missing-required"
  | "null-required"
  | "unknown-component"
  | "inline-reserved"
  | "excess-args"
  | "type-mismatch"
  | "budget-exceeded"
  // adapter
  | "incomplete"
  | "no-root"
  | "duplicate-statement"
  | "unresolved"
  | "parse-exception"
  | "invalid-action"
  | "tool-unknown"
  | "tool-dynamic"
  | "tool-args"
  | "state-undeclared";

/** Result of `check`/`mergeCheck`; also the helper response body (plus `id`). */
export interface CheckResult {
  /** Canonical source that was validated: trimmed input for `check`, upstream merge output for `merge_check`. */
  source: string;
  library_hash: string;
  statements: StatementInfo[];
  bindings: Binding[];
  /** Empty means the program is publishable against this library and tool snapshot. */
  diagnostics: Diagnostic[];
  /** Declared `$state` defaults (materialized literals); used to retain user state across compatible generations. */
  state: Record<string, unknown>;
  /** Value statements not reachable from root. Informational: they never render, and `merge_check` garbage-collects them. */
  orphaned: string[];
}

export interface ParserResponse extends CheckResult {
  id: string;
}

/** Emitted instead of `ParserResponse` when the request line itself is malformed. */
export interface ParserErrorResponse {
  id: string | null;
  error: string;
}
