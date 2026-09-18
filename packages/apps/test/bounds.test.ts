import { describe, expect, test } from "bun:test";
import {
  LIMITS,
  LimitError,
  createEvalBudget,
  createParser,
  createStore,
  createStreamingParser,
  evaluate,
  evaluateElementProps,
  mergeStatements,
  resolveStateField,
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

  test("non-component reference blowup through arrays is charged before allocating", () => {
    // 2^27 array cells if materialized; the node budget trips first.
    let src = 'root = TextContent("" + a26)\na0 = [0]\n';
    for (let i = 1; i <= 26; i++) src += `a${i} = [a${i - 1}, a${i - 1}]\n`;
    const r = parser.parse(src);
    expect(r.root).toBeNull();
    expect(r.meta.errors[0]?.message).toContain(`${LIMITS.materializedNodes}`);
  });

  test("left-associative operator chains are bounded by AST depth, not parser recursion", () => {
    const r = parser.parse(`root = TextContent("" ${"+ 1 ".repeat(5000)})`);
    expect(r.root).toBeNull();
    expect(r.meta.errors[0]).toMatchObject({
      code: "budget-exceeded",
      statementId: "root",
    });
    expect(r.meta.errors[0]!.message).toContain("nesting");
    expect(
      parser.parse(`root = TextContent("" ${"+ 1 ".repeat(20)})`).root,
    ).not.toBeNull();
  });

  test("component depth through an @Each template is bounded", () => {
    let src = 'root = Stack(@Each([0], "i", n0))\n';
    for (let i = 0; i < LIMITS.depth + 8; i++)
      src += `n${i} = Stack([n${i + 1}])\n`;
    src += `n${LIMITS.depth + 8} = TextContent("leaf")`;
    const r = parser.parse(src);
    expect(r.root).toBeNull();
    expect(r.meta.errors[0]?.code).toBe("budget-exceeded");
  });

  test("Query/Mutation arguments are normalized: ordinary refs inlined, unknown refs unresolved", () => {
    const r = parser.parse(
      'root = Stack([])\nname = "prod"\n$app = "x"\nm = Mutation("save", {name: name, other: missing})\nq = Query("list", {who: name, app: $app})',
    );
    expect(r.mutationStatements[0]!.argsAST).toEqual({
      k: "Obj",
      entries: [
        ["name", { k: "Str", v: "prod" }],
        ["other", { k: "Ph", n: "missing" }],
      ],
    });
    expect(r.meta.unresolved).toEqual(["missing"]);
    expect(r.queryStatements[0]!.deps).toEqual(["$app"]);
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

  test("array-wide builtins and member projection charge per element", () => {
    const big = Array.from({ length: LIMITS.evalOps + 1 }, () => 0);
    const ctx = (): EvaluationContext => ({
      getState: () => null,
      resolveRef: (n) => (n === "q" ? big : null),
      budget: createEvalBudget(),
    });
    const call = (name: string): ASTNode => ({
      k: "Comp",
      name,
      args: [{ k: "Ref", n: "q" }],
    });
    for (const name of ["Sum", "Avg", "Min", "Max", "Sort", "Filter"])
      expect(() => evaluate(call(name), ctx())).toThrow(LimitError);
    expect(() =>
      evaluate({ k: "Member", obj: { k: "Ref", n: "q" }, field: "x" }, ctx()),
    ).toThrow(LimitError);
    // O(1) builtins and .length stay cheap
    expect(evaluate(call("Count"), ctx())).toBe(big.length);
    expect(
      evaluate(
        { k: "Member", obj: { k: "Ref", n: "q" }, field: "length" },
        ctx(),
      ),
    ).toBe(big.length);
    expect(evaluate(call("Sum"), { ...ctx(), budget: undefined })).toBe(0);
  });

  test("@Each projects the used field without literalizing unused row payload", () => {
    const payload = Array.from({ length: LIMITS.evalOps }, () => 0);
    const items = Array.from({ length: 8 }, (_, i) => ({ id: i, payload }));
    const ctx = (): EvaluationContext => ({
      getState: () => null,
      resolveRef: (n) => (n === "items" ? items : null),
      budget: createEvalBudget(),
    });
    const each = (template: ASTNode): ASTNode => ({
      k: "Comp",
      name: "Each",
      args: [{ k: "Ref", n: "items" }, { k: "Ref", n: "r" }, template],
    });
    expect(
      evaluate(
        each({ k: "Member", obj: { k: "Ref", n: "r" }, field: "id" }),
        ctx(),
      ),
    ).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    // capturing the whole row into a deferred @Set is charged per literal node
    expect(() =>
      evaluate(
        each({
          k: "Comp",
          name: "Set",
          args: [
            { k: "StateRef", n: "$row" },
            { k: "Ref", n: "r" },
          ],
        }),
        ctx(),
      ),
    ).toThrow(LimitError);
    // deeply nested payload cannot be literalized past the depth bound
    let deep: unknown = 0;
    for (let i = 0; i < LIMITS.depth + 1; i++) deep = [deep];
    expect(() =>
      evaluate(
        each({
          k: "Comp",
          name: "Set",
          args: [
            { k: "StateRef", n: "$row" },
            { k: "Ref", n: "r" },
          ],
        }),
        { ...ctx(), resolveRef: () => [deep] },
      ),
    ).toThrow(LimitError);
  });

  test("reactive input assignments get a fresh budget and do not commit on LimitError", () => {
    const store = createStore();
    store.initialize({ $x: "old" }, {});
    const reported: LimitError[] = [];
    const expr: ASTNode = {
      k: "Comp",
      name: "Each",
      args: [
        { k: "Ref", n: "rows" },
        { k: "Ref", n: "r" },
        { k: "Ref", n: "r" },
      ],
    };
    const field = resolveStateField(
      "f",
      { __reactive: "assign", target: "$x", expr },
      store,
      {
        getState: () => null,
        resolveRef: () => rows,
        onLimitError: (e) => reported.push(e),
      },
      () => null,
      () => {},
    );
    field.setValue("typed");
    expect(store.get("$x")).toBe("old");
    expect(reported.map((e) => e.limit)).toEqual(["evalNodes"]);
    // a second change is not starved by the first: fresh budget per change
    const small = resolveStateField(
      "g",
      {
        __reactive: "assign",
        target: "$x",
        expr: { k: "StateRef", n: "$value" },
      },
      store,
      { getState: () => null, resolveRef: () => null },
      () => null,
      () => {},
    );
    small.setValue("new");
    expect(store.get("$x")).toBe("new");
    // without a handler the LimitError propagates and still leaves the store untouched
    const unhandled = resolveStateField(
      "h",
      { __reactive: "assign", target: "$x", expr },
      store,
      { getState: () => null, resolveRef: () => rows },
      () => null,
      () => {},
    );
    expect(() => unhandled.setValue("again")).toThrow(LimitError);
    expect(store.get("$x")).toBe("new");
  });
});
