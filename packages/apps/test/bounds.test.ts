import { describe, expect, test } from "bun:test";
import {
  LIMITS,
  LimitError,
  createEvalBudget,
  createParser,
  createStreamingParser,
  evaluate,
  evaluateElementProps,
  mergeStatements,
} from "@openuidev/lang-core";
import type { ASTNode, EvaluationContext } from "@openuidev/lang-core";
import { generatedLibrary } from "../src/check.ts";

const parser = createParser(generatedLibrary.schema, generatedLibrary.root);

/** `root = Stack([aN, aN])` … `a0 = TextContent("x")`: 2^(levels+1)-1 expansions via shared references. */
function sharedRefBlowup(levels: number): string {
  let src = `root = Stack([a${levels}, a${levels}])\n`;
  for (let i = levels; i > 0; i--)
    src += `a${i} = Stack([a${i - 1}, a${i - 1}])\n`;
  return `${src}a0 = TextContent("x")\n`;
}

describe("parse-time bounds", () => {
  test("shared-reference blowup fails with a located render-budget error before producing a tree", () => {
    const r = parser.parse(sharedRefBlowup(12));
    expect(r.root).toBeNull();
    expect(r.meta.errors).toHaveLength(1);
    expect(r.meta.errors[0]).toMatchObject({ code: "budget-exceeded" });
    expect(r.meta.errors[0]!.message).toContain(`${LIMITS.expandedNodes}`);
    expect(r.meta.errors[0]!.statementId).toBeString();
  });

  test("counts reference occurrences, not distinct declarations", () => {
    // 10 levels → 2047 element expansions: within budget. 11 levels → 4095: over.
    expect(parser.parse(sharedRefBlowup(9)).root).not.toBeNull();
    expect(parser.parse(sharedRefBlowup(11)).root).toBeNull();
  });

  test("streaming parser applies the same budget to partial input", () => {
    const sp = createStreamingParser(
      generatedLibrary.schema,
      generatedLibrary.root,
    );
    const src = sharedRefBlowup(12);
    const r = sp.set(src.slice(0, src.length - 5));
    expect(r.root).toBeNull();
    expect(r.meta.errors[0]?.code).toBe("budget-exceeded");
  });

  test("expression nesting deeper than the depth limit is rejected", () => {
    const deep = `root = ${"Stack([".repeat(LIMITS.depth)}TextContent("x")${"])".repeat(LIMITS.depth)}`;
    const r = parser.parse(deep);
    expect(r.root).toBeNull();
    expect(r.meta.errors[0]?.message).toContain("nesting");
    const ok = `root = ${"Stack([".repeat(8)}TextContent("x")${"])".repeat(8)}`;
    expect(parser.parse(ok).root?.typeName).toBe("Stack");
  });

  test("component tree deeper than the depth limit through references is rejected", () => {
    let src = "root = Stack([n1])\n";
    for (let i = 1; i <= LIMITS.depth; i++)
      src += `n${i} = Stack([n${i + 1}])\n`;
    src += `n${LIMITS.depth + 1} = TextContent("leaf")\n`;
    const r = parser.parse(src);
    expect(r.root).toBeNull();
    expect(r.meta.errors[0]?.code).toBe("budget-exceeded");
  });

  test("statement cap", () => {
    let src = "root = Stack([])\n";
    for (let i = 0; i < LIMITS.statements; i++)
      src += `s${i} = TextContent("x")\n`;
    const r = parser.parse(src);
    expect(r.root).toBeNull();
    expect(r.meta.errors[0]?.message).toContain(`${LIMITS.statements}`);
  });

  test("source cap is measured in UTF-8 bytes", () => {
    // 70 000 two-byte characters = 140 000 bytes > 131 072 while only ~70 000 chars.
    const r = parser.parse(`root = TextContent("${"é".repeat(70_000)}")`);
    expect(r.root).toBeNull();
    expect(r.meta.errors[0]?.message).toContain("bytes");
    expect(
      parser.parse(`root = TextContent("${"e".repeat(70_000)}")`).root,
    ).not.toBeNull();
  });

  test("mergeStatements propagates LimitError for over-deep patches", () => {
    const deep = `x = ${"[".repeat(LIMITS.depth + 1)}1${"]".repeat(LIMITS.depth + 1)}`;
    expect(() =>
      mergeStatements('root = Stack([x])\nx = TextContent("a")', deep),
    ).toThrow(LimitError);
  });
});

describe("evaluation budget", () => {
  const rows = Array.from({ length: LIMITS.evalNodes + 1 }, (_, i) => ({
    id: i,
  }));
  const baseCtx = (budget = createEvalBudget()): EvaluationContext => ({
    getState: () => null,
    resolveRef: (n) => (n === "rows" ? rows : null),
    budget,
  });
  const each = (template: ASTNode): ASTNode => ({
    k: "Comp",
    name: "Each",
    args: [{ k: "Ref", n: "rows" }, { k: "Ref", n: "r" }, template],
  });

  test("@Each charges produced nodes before allocating the result array", () => {
    const budget = createEvalBudget();
    expect(() =>
      evaluate(
        each({ k: "Member", obj: { k: "Ref", n: "r" }, field: "id" }),
        baseCtx(budget),
      ),
    ).toThrow(LimitError);
    expect(budget.nodes).toBe(LIMITS.evalNodes);
  });

  test("nested @Each shares one budget", () => {
    const small = Array.from({ length: 64 }, (_, i) => i);
    const ctx: EvaluationContext = {
      getState: () => null,
      resolveRef: (n) => (n === "rows" ? small : null),
      budget: createEvalBudget(),
    };
    const inner: ASTNode = {
      k: "Comp",
      name: "Each",
      args: [
        { k: "Ref", n: "rows" },
        { k: "Ref", n: "j" },
        { k: "Ref", n: "j" },
      ],
    };
    // 64 outer × (1 + 64 inner) = 4160 > 2048
    expect(() =>
      evaluate(
        {
          k: "Comp",
          name: "Each",
          args: [{ k: "Ref", n: "rows" }, { k: "Ref", n: "i" }, inner],
        },
        ctx,
      ),
    ).toThrow(LimitError);
  });

  test("operation budget stops runaway expression evaluation", () => {
    const budget = { ops: 10, nodes: 10 };
    let expr: ASTNode = { k: "Num", v: 1 };
    for (let i = 0; i < 20; i++)
      expr = { k: "BinOp", op: "+", left: expr, right: { k: "Num", v: 1 } };
    expect(() => evaluate(expr, baseCtx(budget))).toThrow(LimitError);
  });

  test("evaluateElementProps rethrows LimitError instead of leaking raw AST into props", () => {
    const parsed = parser.parse(
      'root = Stack(@Each(rows, "r", TextContent("" + r.id)))\nrows = Query("list", {}, [])',
    );
    expect(parsed.root).not.toBeNull();
    const evalCtx = {
      ctx: baseCtx(),
      library: { components: {} } as never,
      store: null,
      errors: [],
    };
    expect(() => evaluateElementProps(parsed.root!, evalCtx)).toThrow(
      LimitError,
    );
  });

  test("without a budget, evaluation is unbounded (upstream behavior preserved)", () => {
    const ctx: EvaluationContext = {
      getState: () => null,
      resolveRef: (n) => (n === "rows" ? rows : null),
    };
    expect(
      evaluate(
        each({ k: "Member", obj: { k: "Ref", n: "r" }, field: "id" }),
        ctx,
      ),
    ).toHaveLength(rows.length);
  });
});
