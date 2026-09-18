import { describe, expect, test } from "bun:test";
import { LIMITS } from "@openuidev/lang-core";
import fixtures from "../../../scripts/apps/fixtures.json";
import { check, libraryHash, mergeCheck } from "../src/check.ts";
import type { ToolSnapshot } from "../src/protocol.ts";

const tools: ToolSnapshot[] = [
  {
    name: "argo_app_health",
    schema: {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
      additionalProperties: false,
    },
    risk: "read_only",
    identity: "mcp:argo/argo_app_health#1",
  },
  {
    name: "argo_sync",
    schema: {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
    },
    risk: "mutating",
    identity: "mcp:argo/argo_sync#1",
  },
];

/** Design 46 §4.1, verbatim. */
const prototype = `root = Stack([title, status, picture, controls])
$stage = "idle"
title = TextContent("## DR cutover drill")
status = TextContent("Stage: " + $stage)
picture = TextContent("![Architecture](https://www.rust-lang.org/logos/rust-logo-128x128.png)")
controls = Stack([begin, reset, docs], "row")
begin = Button("Begin drill", Action([@Set($stage, "cutover")]))
reset = Button("Reset", Action([@Reset($stage)]))
docs = Button("Open runbook", Action([@OpenUrl("https://argo-cd.readthedocs.io/")]))`;

/** Design 46 §4.2, verbatim. */
const livePanel = `root = Stack([title, pick, healthText, controls, resultText])
$app = "portal-web"
title = TextContent("## ArgoCD health")
pick = Select("app", [SelectItem("portal-web", "Portal web"), SelectItem("portal-api", "Portal API")], "Choose application", null, $app)
health = Query("argo_app_health", {name: $app}, {status: "Loading", synced: false}, 30)
healthText = TextContent($app + ": " + health.status + (health.synced ? " — synced" : " — not synced"))
syncResult = Mutation("argo_sync", {name: $app})
controls = Stack([syncBtn, explain], "row")
syncBtn = Button("Sync " + $app, Action([@Run(syncResult), @Run(health)]))
explain = Button("Explain health", Action([@ToAssistant("Explain this application's health", health.status)]))
resultText = TextContent(syncResult.status == "success" ? syncResult.data.message : (syncResult.status == "error" ? syncResult.error : ""))`;

const codes = (r: { diagnostics: { code: string }[] }) =>
  r.diagnostics.map((d) => d.code);

describe("check: design examples", () => {
  test("prototype publishes with 9 statements, one state default and no bindings", () => {
    const r = check(prototype, []);
    expect(r.diagnostics).toEqual([]);
    expect(r.statements).toHaveLength(9);
    expect(r.statements.find((s) => s.id === "$stage")?.kind).toBe("state");
    expect(r.state).toEqual({ $stage: "idle" });
    expect(r.bindings).toEqual([]);
    expect(r.library_hash).toBe(libraryHash);
  });

  test("live panel publishes with pinned query/mutation bindings carrying risk and identity", () => {
    const r = check(livePanel, tools);
    expect(r.diagnostics).toEqual([]);
    expect(r.statements).toHaveLength(11);
    expect(r.bindings).toEqual([
      {
        statement_id: "health",
        tool: "argo_app_health",
        kind: "query",
        risk: "read_only",
        identity: "mcp:argo/argo_app_health#1",
      },
      {
        statement_id: "syncResult",
        tool: "argo_sync",
        kind: "mutation",
        risk: "mutating",
        identity: "mcp:argo/argo_sync#1",
      },
    ]);
  });

  test("the same source against a session without those tools is rejected per statement", () => {
    const r = check(livePanel, []);
    expect(r.diagnostics.map((d) => [d.code, d.at])).toEqual([
      ["tool-unknown", "health"],
      ["tool-unknown", "syncResult"],
    ]);
    expect(r.bindings).toEqual([]);
  });

  test("fenced source is accepted and canonicalized", () => {
    const r = check(`\`\`\`openui\n${prototype}\n\`\`\``, []);
    expect(r.diagnostics).toEqual([]);
    expect(r.source.startsWith("root = Stack(")).toBeTrue();
  });
});

describe("check: acceptance fixture", () => {
  test("scripts/apps/fixtures.json publishes and its bindings match the declared statements", () => {
    // Same tool contract the fixture MCP server (scripts/apps/fixture-server.ts) advertises.
    const named = {
      type: "object",
      properties: { name: { type: "string" } },
      required: ["name"],
      additionalProperties: false,
    };
    const schemas: Record<string, unknown> = {
      argo_app_health: named,
      argo_sync: named,
      argo_destroy: named,
      argo_slow: {
        ...named,
        properties: { name: { type: "string" }, ms: { type: "integer" } },
      },
      argo_echo: {
        type: "object",
        properties: { secret: { type: "string" } },
        required: ["secret"],
        additionalProperties: false,
      },
    };
    const expected = fixtures.app.statements as Record<
      string,
      { tool: string; kind: string; risk: string }
    >;
    // Risk the fixture server's MCP annotations map to (readOnlyHint / destructiveHint / neither).
    const risks: Record<string, ToolSnapshot["risk"]> = {
      argo_app_health: "read_only",
      argo_sync: "mutating",
      argo_destroy: "destructive",
      argo_slow: "read_only",
      argo_echo: "read_only",
    };
    const fixtureTools: ToolSnapshot[] = fixtures.mcp.tools.map((name) => ({
      name,
      schema: schemas[name] as ToolSnapshot["schema"],
      risk: risks[name]!,
      identity: `mcp:${fixtures.mcp.name}/${name}#1`,
    }));
    const r = check(fixtures.app.source.join("\n"), fixtureTools);
    expect(r.diagnostics).toEqual([]);
    const got: Record<string, { tool: string; kind: string; risk: string }> =
      Object.fromEntries(
        r.bindings.map((b) => [
          b.statement_id,
          { tool: b.tool, kind: b.kind, risk: b.risk },
        ]),
      );
    expect(got).toEqual(expected);
    expect(r.source).toContain(fixtures.secret.placeholder);
  });
});

describe("check: strict final rules", () => {
  test("requires a root statement even when another component could be the entry", () => {
    expect(codes(check('main = Stack([TextContent("x")])', []))).toEqual([
      "no-root",
    ]);
  });

  test("incomplete source (auto-closed bracket) is not publishable", () => {
    expect(codes(check('root = Stack([TextContent("x")', []))).toContain(
      "incomplete",
    );
  });

  test("unresolved and cyclic references are located at the referencing statement", () => {
    const r = check(
      "root = Stack([a, b])\na = TextContent(missing)\nb = Stack([c])\nc = Stack([b])",
      [],
    );
    expect(r.diagnostics).toContainEqual(
      expect.objectContaining({ code: "unresolved", at: "a" }),
    );
    expect(
      r.diagnostics.some(
        (d) => d.code === "unresolved" && d.message.includes("cyclic"),
      ),
    ).toBeTrue();
  });

  test("orphans are reported informationally, not as diagnostics", () => {
    const r = check(
      'root = Stack([a])\na = TextContent("x")\nb = TextContent("dead")',
      [],
    );
    expect(r.diagnostics).toEqual([]);
    expect(r.orphaned).toEqual(["b"]);
  });

  test("upstream schema errors keep their statement location and message", () => {
    const r = check('root = Stack([a])\na = TextContent(42, "huge")', []);
    expect(r.diagnostics.map((d) => d.at)).toEqual(["a", "a"]);
    expect(codes(r)).toEqual(["type-mismatch", "type-mismatch"]);
  });

  test("action steps must be well-formed and inside Action([...])", () => {
    const r = check(
      [
        "root = Stack([a, b, c, d])",
        '$x = ""',
        'a = Button("bare", @Set($x, "y"))',
        'b = Button("run", Action([@Run(nothing)]))',
        'c = Button("url", Action([@OpenUrl("javascript:alert(1)")]))',
        'd = Button("reset", Action([@Reset("literal")]))',
      ].join("\n"),
      [],
    );
    expect(
      r.diagnostics.filter((d) => d.code === "invalid-action").map((d) => d.at),
    ).toEqual(["a", "b", "c", "d"]);
    // @Run(nothing) is also an unresolved reference for the upstream materializer.
    expect(r.diagnostics.filter((d) => d.code !== "invalid-action")).toEqual([
      expect.objectContaining({ code: "unresolved", at: "b" }),
    ]);
  });

  test("undeclared $state is reported at the using statement", () => {
    const r = check('root = Stack([a])\na = TextContent("v: " + $missing)', []);
    expect(r.diagnostics).toEqual([
      expect.objectContaining({ code: "state-undeclared", at: "a" }),
    ]);
  });

  test("dynamic tool names cannot be pinned", () => {
    const r = check(
      'root = Stack([t])\n$tool = "argo_sync"\nq = Query($tool, {})\nt = TextContent("" + q)',
      tools,
    );
    expect(codes(r)).toEqual(["tool-dynamic"]);
    expect(r.bindings).toEqual([]);
  });

  test("argument keys are checked against the tool schema statically", () => {
    const r = check(
      'root = Stack([t])\nq = Query("argo_app_health", {app: "x"})\nt = TextContent(q.status)',
      tools,
    );
    expect(r.diagnostics.map((d) => d.message)).toEqual([
      'tool "argo_app_health" requires argument "name"',
      'tool "argo_app_health" does not accept argument "app"; accepted: name',
    ]);
    expect(
      codes(
        check(
          'root = Stack([t])\nq = Query("argo_sync", {name: "x", extra: 1})\nt = TextContent(q.status)',
          tools,
        ),
      ),
    ).toEqual([]);
  });

  test("tool arguments must be an object literal; refresh must be a number literal", () => {
    expect(
      codes(
        check(
          'root = Stack([t])\nq = Query("argo_sync", [1])\nt = TextContent(q.status)',
          tools,
        ),
      ),
    ).toEqual(["tool-args"]);
    expect(
      codes(
        check(
          'root = Stack([t])\n$r = 30\nq = Query("argo_sync", {name: "x"}, null, $r)\nt = TextContent(q.status)',
          tools,
        ),
      ),
    ).toEqual(["tool-args"]);
  });

  test("oversized input is rejected before tokenizing", () => {
    const r = check(
      `root = TextContent("${"é".repeat(LIMITS.sourceBytes / 2 + 10)}")`,
      [],
    );
    expect(codes(r)).toEqual(["budget-exceeded"]);
    expect(r.statements).toEqual([]);
  });

  test("render-budget blowups surface as a located budget diagnostic", () => {
    let src = "root = Stack([a11, a11])\n";
    for (let i = 11; i > 0; i--)
      src += `a${i} = Stack([a${i - 1}, a${i - 1}])\n`;
    src += 'a0 = TextContent("x")';
    const r = check(src, []);
    expect(r.diagnostics).toEqual([
      expect.objectContaining({ code: "budget-exceeded" }),
    ]);
    expect(r.diagnostics[0]!.at).not.toBe("");
  });

  test("trailing tokens and lines the streaming splitter would drop are syntax errors", () => {
    const trailing = check("root = Stack([]) unexpected", []);
    expect(trailing.diagnostics).toEqual([
      expect.objectContaining({ code: "syntax", at: "root" }),
    ]);
    expect(trailing.statements).toEqual([]);
    const dropped = check('root = Stack([])\nq Query("argo_sync", {})', tools);
    expect(dropped.diagnostics).toEqual([
      expect.objectContaining({ code: "syntax", at: "q" }),
    ]);
    expect(
      codes(check('root = Stack([a b])\na = TextContent("x")\nb = a', [])),
    ).toEqual(["syntax"]);
    expect(codes(check("root = Stack([]) ; x", []))).toEqual(["syntax"]);
    // incomplete input still reports the auto-close alongside the strict failure
    expect(codes(check('root = Stack([TextContent("x")', []))).toEqual([
      "incomplete",
      "syntax",
    ]);
  });

  test("references inside Query/Mutation arguments are validated and ordinary ones resolved", () => {
    const r = check(
      'root = Stack([])\nm = Mutation("argo_sync", {name: missing})',
      tools,
    );
    expect(r.diagnostics).toEqual([
      expect.objectContaining({ code: "unresolved", at: "m" }),
    ]);
    const cyclic = check(
      'root = Stack([])\na = b\nb = a\nm = Mutation("argo_sync", {name: a})',
      tools,
    );
    expect(
      cyclic.diagnostics.some(
        (d) => d.code === "unresolved" && d.message.includes("cyclic"),
      ),
    ).toBeTrue();
    const ok = check(
      'root = Stack([])\nname = "prod"\nm = Mutation("argo_sync", {name: name})',
      tools,
    );
    expect(ok.diagnostics).toEqual([]);
    expect(ok.bindings.map((b) => b.statement_id)).toEqual(["m"]);
  });
});

describe("mergeCheck: upstream merge semantics", () => {
  test("replaces by id, deletes with null, garbage-collects unreachable, retains state", () => {
    const r = mergeCheck(
      prototype,
      'status = TextContent("S: " + $stage)\ndocs = null\ncontrols = Stack([begin, reset], "row")',
      [],
    );
    expect(r.diagnostics).toEqual([]);
    const lines = r.source.split("\n");
    expect(lines).toContain('status = TextContent("S: " + $stage)');
    expect(lines.some((l) => l.startsWith("docs ="))).toBeFalse();
    expect(lines).toContain('$stage = "idle"');
    expect(r.statements.map((s) => s.id)).toEqual([
      "root",
      "$stage",
      "title",
      "status",
      "picture",
      "controls",
      "begin",
      "reset",
    ]);
  });

  test("a patch that removes a still-referenced statement fails validation of the merged program", () => {
    const r = mergeCheck(prototype, "title = null", []);
    expect(r.diagnostics).toEqual([
      expect.objectContaining({ code: "unresolved", at: "root" }),
    ]);
  });

  test("a patch need not contain root; the merged program is what is validated", () => {
    const r = mergeCheck(
      livePanel,
      'health = Query("argo_app_health", {name: $app}, {status: "Loading", synced: false}, 60)',
      tools,
    );
    expect(r.diagnostics).toEqual([]);
    expect(r.bindings.map((b) => b.statement_id)).toEqual([
      "health",
      "syncResult",
    ]);
  });

  test("patch rebinding to an invisible tool is refused with the merged source still returned", () => {
    const r = mergeCheck(
      livePanel,
      'syncResult = Mutation("argo_delete", {name: $app})',
      tools,
    );
    expect(r.diagnostics).toEqual([
      expect.objectContaining({ code: "tool-unknown", at: "syncResult" }),
    ]);
    expect(r.source).toContain('Mutation("argo_delete"');
  });

  test("oversized base or patch is rejected before merging", () => {
    const big = `x = TextContent("${"a".repeat(LIMITS.sourceBytes + 1)}")`;
    expect(codes(mergeCheck(prototype, big, []))).toEqual(["budget-exceeded"]);
    expect(codes(mergeCheck(big, "root = Stack([])", []))).toEqual([
      "budget-exceeded",
    ]);
  });
});
