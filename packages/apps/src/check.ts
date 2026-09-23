/**
 * Final-document validation shared by the headless helper and the desktop adapter (design 46 §4).
 *
 * Upstream parsing is permissive for streaming; this module layers the strict publish rules on
 * top using upstream AST/schema utilities only: complete source, strict syntax (no skipped lines
 * or trailing tokens), a `root` statement, no unresolved/cyclic references (including inside
 * Query/Mutation arguments), no parser errors, supported action targets, literal tool bindings
 * that exist in the visible tool snapshot, and object-literal tool arguments whose keys satisfy
 * the tool schema. Dynamic argument values are schema-checked again at invocation.
 *
 * Imports only `@openuidev/lang-core` and the generated JSON: no React, no component renderers.
 */
import {
  ACTION_STEPS,
  LIMITS,
  LimitError,
  StrictParseError,
  createParser,
  mergeStatements,
  parseExpression,
  split,
  tokenize,
  type ASTNode,
  type ParseResult,
} from "@openuidev/lang-core";
import generated from "../generated/library.json";
import type { GeneratedLibrary } from "./generated-types.ts";
import type {
  Binding,
  CheckResult,
  Diagnostic,
  DiagnosticCode,
  StatementInfo,
  StatementKind,
  ToolSnapshot,
} from "./protocol.ts";

export const generatedLibrary = generated as GeneratedLibrary;
export const libraryHash: string = generatedLibrary.hash;

const ROOT_ID = "root";
/** Compiled once per process; the helper's startup cost, not a per-request cost. */
const parser = createParser(generatedLibrary.schema, generatedLibrary.root);

interface TypedStatement {
  id: string;
  kind: StatementKind;
  expr: ASTNode;
}

/** UTF-8 size guard applied before any tokenizing/merging can expand input; mirrors the parser's cap. */
function oversized(text: string): LimitError | undefined {
  const max = LIMITS.sourceBytes;
  if (text.length * 3 <= max) return;
  const bytes =
    text.length > max ? text.length : new TextEncoder().encode(text).byteLength;
  if (bytes <= max) return;
  return new LimitError(
    "sourceBytes",
    `source is at least ${bytes} bytes; limit is ${max}`,
  );
}

/** Upstream fence stripping without a second grammar: `mergeStatements("", src)` re-emits the patch statements verbatim. */
function canonicalize(source: string): string {
  const trimmed = source.trim();
  return trimmed.startsWith("```") ? mergeStatements("", trimmed) : trimmed;
}

/**
 * Strict re-parse of the final document (`strict` upstream option): trailing tokens, missing
 * closers/separators, unrecognized characters and lines the streaming splitter would drop all
 * throw `StrictParseError`, located at the statement when the splitter knows it.
 */
function typedStatements(source: string): TypedStatement[] {
  return split(tokenize(source, true), true).map((raw) => {
    let expr: ASTNode;
    try {
      expr = parseExpression(raw.tokens, true);
    } catch (e) {
      if (e instanceof StrictParseError || e instanceof LimitError)
        e.statementId ??= raw.id;
      throw e;
    }
    const kind: StatementKind =
      expr.k === "Comp" && expr.name === "Query"
        ? "query"
        : expr.k === "Comp" && expr.name === "Mutation"
          ? "mutation"
          : raw.id.startsWith("$")
            ? "state"
            : "value";
    return { id: raw.id, kind, expr };
  });
}

/** Depth-first visit with parent access (walkAST has no parent pointer). */
function visit(
  node: ASTNode,
  parent: ASTNode | null,
  fn: (node: ASTNode, parent: ASTNode | null) => void,
): void {
  fn(node, parent);
  switch (node.k) {
    case "Comp":
      for (const a of node.args) visit(a, node, fn);
      break;
    case "Arr":
      for (const e of node.els) visit(e, node, fn);
      break;
    case "Obj":
      for (const [, v] of node.entries) visit(v, node, fn);
      break;
    case "BinOp":
      visit(node.left, node, fn);
      visit(node.right, node, fn);
      break;
    case "UnaryOp":
      visit(node.operand, node, fn);
      break;
    case "Ternary":
      visit(node.cond, node, fn);
      visit(node.then, node, fn);
      visit(node.else, node, fn);
      break;
    case "Member":
      visit(node.obj, node, fn);
      break;
    case "Index":
      visit(node.obj, node, fn);
      visit(node.index, node, fn);
      break;
    case "Assign":
      visit(node.value, node, fn);
      break;
    default:
      break;
  }
}

function failure(
  source: string,
  code: DiagnosticCode,
  message: string,
  at = "",
): CheckResult {
  return {
    source,
    library_hash: libraryHash,
    statements: [],
    bindings: [],
    diagnostics: [{ at, message, code }],
    state: {},
    orphaned: [],
  };
}

function exceptionResult(source: string, e: unknown): CheckResult {
  if (e instanceof LimitError)
    return failure(source, "budget-exceeded", e.message, e.statementId ?? "");
  if (e instanceof StrictParseError)
    return failure(source, "syntax", e.message, e.statementId ?? "");
  return failure(
    source,
    "parse-exception",
    `parser crashed: ${e instanceof Error ? e.message : String(e)}`,
  );
}

/** Validate a complete program against the curated library and the visible tool snapshot. */
export function check(source: string, tools: ToolSnapshot[]): CheckResult {
  const tooBig = oversized(source);
  if (tooBig) return exceptionResult("", tooBig);
  let canonical: string;
  try {
    canonical = canonicalize(source);
  } catch (e) {
    return exceptionResult(source.trim(), e);
  }
  return analyze(canonical, tools);
}

/** Merge `patch` into `base` with upstream semantics (replace by id, `x = null` deletes, GC unreachable) and validate the result. */
export function mergeCheck(
  base: string,
  patch: string,
  tools: ToolSnapshot[],
): CheckResult {
  const tooBig = oversized(base) ?? oversized(patch);
  if (tooBig) return exceptionResult("", tooBig);
  let merged: string;
  try {
    merged = mergeStatements(canonicalize(base), patch, ROOT_ID);
  } catch (e) {
    return exceptionResult(base.trim(), e);
  }
  return analyze(merged, tools);
}

function analyze(source: string, tools: ToolSnapshot[]): CheckResult {
  const diagnostics: Diagnostic[] = [];
  const push = (code: DiagnosticCode, message: string, at = "") =>
    diagnostics.push({ at, message, code });

  let result: ParseResult;
  try {
    result = parser.parse(source);
  } catch (e) {
    return exceptionResult(source, e);
  }

  for (const err of result.meta.errors)
    push(err.code, err.message, err.statementId ?? "");
  if (result.meta.incomplete)
    push(
      "incomplete",
      "source is incomplete: an unclosed string or bracket was auto-closed",
    );

  let statements: TypedStatement[];
  try {
    statements = typedStatements(source);
  } catch (e) {
    const rejected = exceptionResult(source, e);
    rejected.diagnostics.unshift(...diagnostics);
    return rejected;
  }

  const byId = new Map<string, TypedStatement>();
  for (const s of statements) {
    if (byId.has(s.id))
      push(
        "duplicate-statement",
        `statement "${s.id}" is defined more than once; the last definition wins`,
        s.id,
      );
    byId.set(s.id, s);
  }
  if (!byId.has(ROOT_ID))
    push(
      "no-root",
      `no root statement: define root = ${generatedLibrary.root}([...]) as the first line`,
    );
  else if (!result.root && !result.meta.errors.length)
    push("no-root", "root did not produce a renderable component", ROOT_ID);

  const locate = (pred: (n: ASTNode) => boolean): string => {
    for (const s of statements) {
      let hit = false;
      visit(s.expr, null, (n) => {
        if (!hit && pred(n)) hit = true;
      });
      if (hit) return s.id;
    }
    return "";
  };
  for (const name of new Set(result.meta.unresolved)) {
    const at = locate((n) => n.k === "Ref" && n.n === name);
    push(
      "unresolved",
      byId.has(name)
        ? `cyclic reference through "${name}"`
        : `unresolved reference "${name}": no statement defines it`,
      at,
    );
  }

  const stateIds = new Set(
    statements.filter((s) => s.kind === "state").map((s) => s.id),
  );
  // `$value` is the pressed node id inside a Graph's `onSelect` action (65 §3.5; upstream's binding-scope name).
  const valueScoped = new Set<ASTNode>();
  for (const s of statements)
    visit(s.expr, null, (n) => {
      if (n.k !== "Comp" || n.name !== "Graph") return;
      const onSelect = n.mappedProps?.onSelect ?? n.args[5];
      if (onSelect) visit(onSelect, null, (m) => void valueScoped.add(m));
    });
  const callable = new Map<string, "query" | "mutation">();
  for (const s of statements)
    if (s.kind === "query" || s.kind === "mutation") callable.set(s.id, s.kind);

  const stepNames = new Set(Object.keys(ACTION_STEPS));
  for (const s of statements) {
    visit(s.expr, null, (n, parent) => {
      if (
        n.k === "StateRef" &&
        !stateIds.has(n.n) &&
        !(n.n === "$value" && valueScoped.has(n))
      )
        push(
          "state-undeclared",
          `"${n.n}" is used but never declared; add ${n.n} = <default> as a statement`,
          s.id,
        );
      if (n.k !== "Comp") return;
      if (n.name === "Action") {
        const steps = n.args[0];
        if (n.args.length !== 1 || !steps || steps.k !== "Arr")
          push(
            "invalid-action",
            "Action takes exactly one array of steps: Action([@Set(...), @Run(...)])",
            s.id,
          );
        else
          for (const step of steps.els)
            if (step.k !== "Comp" || !stepNames.has(step.name))
              push(
                "invalid-action",
                `Action steps must be @${[...stepNames].join("/@")}`,
                s.id,
              );
        return;
      }
      if (!stepNames.has(n.name)) return;
      if (!(parent && parent.k === "Arr"))
        push(
          "invalid-action",
          `@${n.name} must appear inside Action([...])`,
          s.id,
        );
      switch (n.name) {
        case "Run": {
          const target = n.args[0];
          if (
            n.args.length !== 1 ||
            !target ||
            target.k !== "Ref" ||
            !callable.has(target.n)
          )
            push(
              "invalid-action",
              "@Run target must be a declared Query or Mutation statement",
              s.id,
            );
          break;
        }
        case "Set": {
          const target = n.args[0];
          if (n.args.length !== 2 || !target || target.k !== "StateRef")
            push(
              "invalid-action",
              "@Set takes a $state variable and a value: @Set($var, value)",
              s.id,
            );
          break;
        }
        case "Reset":
          if (!n.args.length || n.args.some((a) => a.k !== "StateRef"))
            push(
              "invalid-action",
              "@Reset takes one or more $state variables",
              s.id,
            );
          break;
        case "OpenUrl": {
          const url = n.args[0];
          if (n.args.length !== 1 || !url)
            push("invalid-action", "@OpenUrl takes exactly one url", s.id);
          else if (url.k === "Str" && !/^https?:\/\//i.test(url.v))
            push(
              "invalid-action",
              `@OpenUrl requires an http(s) url, got "${url.v}"`,
              s.id,
            );
          break;
        }
        case "ToAssistant":
          if (n.args.length < 1 || n.args.length > 2)
            push(
              "invalid-action",
              "@ToAssistant takes a message and an optional context",
              s.id,
            );
          break;
        default:
          break;
      }
    });
  }

  const toolsByName = new Map(tools.map((t) => [t.name, t]));
  const bindings: Binding[] = [];
  for (const s of statements) {
    if (s.kind !== "query" && s.kind !== "mutation") continue;
    if (s.expr.k !== "Comp") continue;
    const [toolAst, argsAst, , refreshAst] = s.expr.args;
    const maxArgs = s.kind === "query" ? 4 : 2;
    if (s.expr.args.length > maxArgs)
      push(
        "tool-args",
        `${s.expr.name} takes at most ${maxArgs} arguments`,
        s.id,
      );
    if (!toolAst || toolAst.k !== "Str") {
      push(
        "tool-dynamic",
        `${s.expr.name} tool name must be a string literal so the binding can be pinned`,
        s.id,
      );
      continue;
    }
    const tool = toolsByName.get(toolAst.v);
    if (!tool) {
      const visible = [...toolsByName.keys()];
      const hint = visible.length
        ? ` Visible tools: ${visible.slice(0, 20).join(", ")}${visible.length > 20 ? ", …" : ""}`
        : " No tools are visible in this session.";
      push(
        "tool-unknown",
        `tool "${toolAst.v}" is not available.${hint}`,
        s.id,
      );
      continue;
    }
    if (argsAst && argsAst.k !== "Obj" && argsAst.k !== "Null")
      push(
        "tool-args",
        `${s.expr.name} arguments must be an object literal {key: value}`,
        s.id,
      );
    else if (argsAst?.k === "Obj") checkArgKeys(argsAst, tool, s.id, push);
    else checkArgKeys({ k: "Obj", entries: [] }, tool, s.id, push);
    if (
      s.kind === "query" &&
      refreshAst &&
      refreshAst.k !== "Num" &&
      refreshAst.k !== "Null"
    )
      push(
        "tool-args",
        "Query refresh interval must be a number literal (seconds)",
        s.id,
      );
    bindings.push({
      statement_id: s.id,
      tool: tool.name,
      kind: s.kind,
      risk: tool.risk,
      identity: tool.identity,
    });
  }

  return {
    source,
    library_hash: libraryHash,
    statements: statements.map<StatementInfo>((s) => ({
      id: s.id,
      kind: s.kind,
    })),
    bindings,
    diagnostics,
    state: result.stateDeclarations,
    orphaned: result.meta.orphaned,
  };
}

/** Static key check against the tool's JSON input schema: required keys present, no unknown keys when the schema forbids them. */
function checkArgKeys(
  args: Extract<ASTNode, { k: "Obj" }>,
  tool: ToolSnapshot,
  at: string,
  push: (code: DiagnosticCode, message: string, at?: string) => void,
): void {
  const schema = tool.schema;
  if (!schema || typeof schema !== "object" || Array.isArray(schema)) return;
  const keys = new Set(args.entries.map(([k]) => k));
  const properties =
    "properties" in schema &&
    schema.properties &&
    typeof schema.properties === "object"
      ? Object.keys(schema.properties)
      : null;
  const required =
    "required" in schema && Array.isArray(schema.required)
      ? schema.required.filter((r): r is string => typeof r === "string")
      : [];
  const closed =
    "additionalProperties" in schema && schema.additionalProperties === false;
  for (const r of required)
    if (!keys.has(r))
      push("tool-args", `tool "${tool.name}" requires argument "${r}"`, at);
  if (properties && closed)
    for (const k of keys)
      if (!properties.includes(k))
        push(
          "tool-args",
          `tool "${tool.name}" does not accept argument "${k}"; accepted: ${properties.join(", ")}`,
          at,
        );
}
