import { describe, expect, test } from "bun:test";
import { join } from "node:path";
import { check } from "../src/check.ts";
import type { ToolSnapshot } from "../src/protocol.ts";

/** The tools the skill's examples name, with the argument keys they pass (123 §6 recipes included). */
const tools: ToolSnapshot[] = Object.entries({
  argo_app_health: ["name"],
  argo_sync: ["name"],
  list_incidents: ["status"],
  ack_incident: ["id"],
  plan_status: [],
  schedule_fire: ["name"],
  task_state: ["id"],
  session_cost: [],
  task_board: [],
  task_move: ["task", "state"],
}).map(([name, keys]) => ({
  name,
  schema: {
    type: "object",
    properties: Object.fromEntries(keys.map((k) => [k, { type: "string" }])),
    required: keys,
    additionalProperties: false,
  },
  risk: "read_only",
  identity: `test:${name}#1`,
}));

const skill = await Bun.file(
  join(import.meta.dir, "../../../crates/app/apps.md"),
).text();
/** Every ```openui fence that is a whole program: has `root =` and no `# comment` (the strict checker rejects them). */
const programs = [...skill.matchAll(/```openui\n([\s\S]*?)```/g)]
  .map((m) => m[1]!)
  .filter((src) => /^root = /m.test(src) && !/(^|\s)#\s/m.test(src));

describe("apps.md", () => {
  test("has the prototype, live panel, table, step-through Graph and the four 123 diagram recipes", () => {
    expect(programs).toHaveLength(8);
    for (const name of ["Gantt(", "StateMachine(", "Sankey(", "Swimlane("])
      expect(programs.some((p) => p.includes(name))).toBe(true);
  });

  test.each(programs.map((p, i) => [i, p.split("\n")[0]!, p] as const))(
    "program %i (%s) passes check()",
    (_i, _head, src) => {
      const r = check(src, tools);
      expect(r.diagnostics).toEqual([]);
    },
  );
});
