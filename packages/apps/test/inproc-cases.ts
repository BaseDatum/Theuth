/**
 * Request cases for the in-process bundle: ported from check.test.ts (same sources, tools and bases) plus the
 * scripts/apps/fixtures.json program and its two edit patches, plus protocol-error lines. Used by
 * inproc.test.ts (bundle in a bare node:vm context vs direct `call()`) and by scripts/record-inproc-golden.ts
 * (crates/js-runtime/tests/apps_golden.json, replayed in deno_core by the Rust tests).
 */
import { LIMITS } from "@openuidev/lang-core";
import fixtures from "../../../scripts/apps/fixtures.json";
import type { ToolSnapshot } from "../src/protocol.ts";

export const tools: ToolSnapshot[] = [
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

export const prototype = `root = Stack([title, status, picture, controls])
$stage = "idle"
title = TextContent("## DR cutover drill")
status = TextContent("Stage: " + $stage)
picture = TextContent("![Architecture](https://www.rust-lang.org/logos/rust-logo-128x128.png)")
controls = Stack([begin, reset, docs], "row")
begin = Button("Begin drill", Action([@Set($stage, "cutover")]))
reset = Button("Reset", Action([@Reset($stage)]))
docs = Button("Open runbook", Action([@OpenUrl("https://argo-cd.readthedocs.io/")]))`;

export const livePanel = `root = Stack([title, pick, healthText, controls, resultText])
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

/** The fixture MCP server's tool contract (same as check.test.ts). */
function fixtureTools(): ToolSnapshot[] {
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
    argo_slow: { ...named, properties: { name: { type: "string" }, ms: { type: "integer" } } },
    argo_echo: {
      type: "object",
      properties: { secret: { type: "string" } },
      required: ["secret"],
      additionalProperties: false,
    },
  };
  const risks: Record<string, string> = {
    argo_app_health: "read_only",
    argo_sync: "mutating",
    argo_destroy: "destructive",
    argo_slow: "read_only",
    argo_echo: "read_only",
  };
  return fixtures.mcp.tools.map((name) => ({
    name,
    schema: schemas[name],
    risk: risks[name]!,
    identity: `mcp:${fixtures.mcp.name}/${name}#1`,
  }));
}

export interface Case {
  name: string;
  /** The request line as the Rust side sends it. */
  line: string;
}

const req = (id: string, op: string, source: string, ts: ToolSnapshot[], base?: string) =>
  JSON.stringify(base === undefined ? { id, op, source, tools: ts } : { id, op, source, base, tools: ts });

let budget = "root = Stack([a11, a11])\n";
for (let i = 11; i > 0; i--) budget += `a${i} = Stack([a${i - 1}, a${i - 1}])\n`;
budget += 'a0 = TextContent("x")';

const fixtureSource = fixtures.app.source.join("\n");

export const CASES: Case[] = [
  { name: "prototype", line: req("c1", "check", prototype, []) },
  { name: "live panel with tools", line: req("c2", "check", livePanel, tools) },
  { name: "live panel without tools", line: req("c3", "check", livePanel, []) },
  { name: "fenced source", line: req("c4", "check", `\`\`\`openui\n${prototype}\n\`\`\``, []) },
  { name: "fixtures.json app", line: req("c5", "check", fixtureSource, fixtureTools()) },
  { name: "no root", line: req("c6", "check", 'main = Stack([TextContent("x")])', []) },
  { name: "incomplete", line: req("c7", "check", 'root = Stack([TextContent("x")', []) },
  {
    name: "unresolved and cyclic",
    line: req("c8", "check", "root = Stack([a, b])\na = TextContent(missing)\nb = Stack([c])\nc = Stack([b])", []),
  },
  { name: "orphans", line: req("c9", "check", 'root = Stack([a])\na = TextContent("x")\nb = TextContent("dead")', []) },
  { name: "schema error", line: req("c10", "check", 'root = Stack([a])\na = TextContent(42, "huge")', []) },
  {
    name: "invalid actions",
    line: req(
      "c11",
      "check",
      [
        "root = Stack([a, b, c, d])",
        '$x = ""',
        'a = Button("bare", @Set($x, "y"))',
        'b = Button("run", Action([@Run(nothing)]))',
        'c = Button("url", Action([@OpenUrl("javascript:alert(1)")]))',
        'd = Button("reset", Action([@Reset("literal")]))',
      ].join("\n"),
      [],
    ),
  },
  { name: "undeclared state", line: req("c12", "check", 'root = Stack([a])\na = TextContent("v: " + $missing)', []) },
  {
    name: "dynamic tool",
    line: req("c13", "check", 'root = Stack([t])\n$tool = "argo_sync"\nq = Query($tool, {})\nt = TextContent("" + q)', tools),
  },
  {
    name: "argument keys",
    line: req("c14", "check", 'root = Stack([t])\nq = Query("argo_app_health", {app: "x"})\nt = TextContent(q.status)', tools),
  },
  {
    name: "extra key allowed",
    line: req("c15", "check", 'root = Stack([t])\nq = Query("argo_sync", {name: "x", extra: 1})\nt = TextContent(q.status)', tools),
  },
  { name: "args not object", line: req("c16", "check", 'root = Stack([t])\nq = Query("argo_sync", [1])\nt = TextContent(q.status)', tools) },
  {
    name: "refresh not literal",
    line: req("c17", "check", 'root = Stack([t])\n$r = 30\nq = Query("argo_sync", {name: "x"}, null, $r)\nt = TextContent(q.status)', tools),
  },
  {
    name: "oversized source",
    line: req("c18", "check", `root = TextContent("${"é".repeat(LIMITS.sourceBytes / 2 + 10)}")`, []),
  },
  { name: "render budget", line: req("c19", "check", budget, []) },
  { name: "trailing tokens", line: req("c20", "check", "root = Stack([]) unexpected", []) },
  { name: "dropped line", line: req("c21", "check", 'root = Stack([])\nq Query("argo_sync", {})', tools) },
  { name: "reference in mutation args", line: req("c22", "check", 'root = Stack([])\nm = Mutation("argo_sync", {name: missing})', tools) },
  { name: "resolved mutation args", line: req("c23", "check", 'root = Stack([])\nname = "prod"\nm = Mutation("argo_sync", {name: name})', tools) },
  {
    name: "merge: replace, delete, gc",
    line: req(
      "m1",
      "merge_check",
      'status = TextContent("S: " + $stage)\ndocs = null\ncontrols = Stack([begin, reset], "row")',
      [],
      prototype,
    ),
  },
  { name: "merge: removes referenced", line: req("m2", "merge_check", "title = null", [], prototype) },
  {
    name: "merge: patch without root",
    line: req("m3", "merge_check", 'health = Query("argo_app_health", {name: $app}, {status: "Loading", synced: false}, 60)', tools, livePanel),
  },
  { name: "merge: invisible tool", line: req("m4", "merge_check", 'syncResult = Mutation("argo_delete", {name: $app})', tools, livePanel) },
  {
    name: "merge: oversized patch",
    line: req("m5", "merge_check", `x = TextContent("${"a".repeat(LIMITS.sourceBytes + 1)}")`, [], prototype),
  },
  { name: "merge: fixtures edit A", line: req("m6", "merge_check", fixtures.patches.a, fixtureTools(), fixtureSource) },
  { name: "merge: fixtures edit B", line: req("m7", "merge_check", fixtures.patches.b, fixtureTools(), fixtureSource) },
  { name: "protocol: invalid JSON", line: "{not json" },
  { name: "protocol: missing id", line: JSON.stringify({ op: "check", source: "x", tools: [] }) },
  { name: "protocol: unknown op", line: JSON.stringify({ id: "p3", op: "render", source: "x", tools: [] }) },
  { name: "protocol: merge without base", line: JSON.stringify({ id: "p4", op: "merge_check", source: "x", tools: [] }) },
  { name: "protocol: bad tools", line: JSON.stringify({ id: "p5", op: "check", source: "x", tools: [{ name: 1 }] }) },
  { name: "protocol: mermaid not served", line: JSON.stringify({ id: "p6", op: "check_mermaid", source: "graph TD" }) },
];
