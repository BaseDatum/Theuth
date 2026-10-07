/**
 * Design 123 §6: zod mirrors of the `crates/surface` diagram props the apps library registers, with the structural
 * rules of `crates/surface/src/validate.rs` as `superRefine`s at the same JSON-Pointer paths and messages (the
 * parity test in `test/diagrams.test.tsx` runs the crate's fixtures through them). Wire shapes only — `library.ts`
 * adds the apps-only reactive `highlight` and `onSelect`.
 *
 * `BarChart`, `LineChart`, `ScatterChart` and `RadarChart` are NOT here: upstream OpenUI already defines components
 * of those names (recharts) and the library is keyed by name (`buildCuratedLibrary` keeps upstream first).
 */
import { z } from "zod/v4";

type Ctx = z.RefinementCtx;
const issue = (ctx: Ctx, path: (string | number)[], message: string) =>
  ctx.addIssue({ code: "custom", path, message });

const str = z.string();
const num = z.number();
const uint = z.number().int().min(0);
const state = z
  .enum(["default", "active", "done", "muted", "danger"])
  .optional();
const lineStyle = z.enum(["solid", "dashed"]).optional();
const direction = z.enum(["LR", "TB"]).optional();
const title = str.optional();
const ref = str.optional();

/** `crates/surface` caps (lib.rs). */
export const CAPS = {
  label: 80,
  sub: 40,
  tag: 12,
  graphNodes: 120,
  graphEdges: 240,
  graphGroups: 12,
  graphDepth: 3,
  states: 40,
  transitions: 80,
  entities: 24,
  fields: 32,
  relations: 48,
  lanes: 8,
  laneSteps: 40,
  laneEdges: 80,
  gantt: 60,
  layers: 12,
  quadrant: 40,
  waterfall: 20,
  funnel: 10,
  treemap: 60,
  treemapGroups: 12,
  sankeyNodes: 24,
  sankeyFlows: 48,
  journeyStages: 8,
  journeyRows: 4,
  fishbone: 8,
  fishboneItems: 6,
} as const;

const cap = (
  ctx: Ctx,
  path: (string | number)[],
  n: number,
  max: number,
  unit: string,
) => {
  if (n > max) issue(ctx, path, `${n} ${unit}, limit ${max}`);
};
const within = (
  ctx: Ctx,
  path: (string | number)[],
  n: number,
  lo: number,
  hi: number,
  unit: string,
) => {
  if (n < lo || n > hi)
    issue(ctx, path, `${n} ${unit}, expected ${lo}..=${hi}`);
};
const label = (ctx: Ctx, path: (string | number)[], text: string) => {
  const n = [...text].length;
  if (n > CAPS.label) issue(ctx, path, `${n} characters, limit ${CAPS.label}`);
};
const short = (
  ctx: Ctx,
  path: (string | number)[],
  text: string | undefined,
  max: number,
) => {
  if (text === undefined) return;
  const n = [...text].length;
  if (n > max) issue(ctx, path, `${n} characters, limit ${max}`);
};
/** Collect ids; a repeat is `duplicate <what> id` at `<path>/<i>/id`. */
const unique = (
  ctx: Ctx,
  path: (string | number)[],
  ids: readonly string[],
  what: string,
): Set<string> => {
  const set = new Set<string>();
  ids.forEach((id, i) => {
    if (set.has(id))
      issue(ctx, [...path, i, "id"], `duplicate ${what} id ${JSON.stringify(id)}`);
    set.add(id);
  });
  return set;
};
const resolves = (
  ctx: Ctx,
  path: (string | number)[],
  id: string,
  set: Set<string>,
  what: string,
) => {
  if (!set.has(id)) issue(ctx, path, `unknown ${what} ${JSON.stringify(id)}`);
};

/** `crates/surface::validate::iso_datetime`: an ISO date or RFC 3339 datetime as a comparable UTC millisecond count. */
export function isoDatetime(s: string): number | null {
  const m =
    /^(\d{4})-(\d{2})-(\d{2})(?:[Tt ](\d{2}):(\d{2})(?::(\d{2})(?:\.(\d+))?)?(Z|z|[+-]\d{2}:\d{2})?)?$/.exec(
      s,
    );
  if (!m) return null;
  const [, y, mo, d, h = "0", mi = "0", se = "0"] = m;
  const t = Date.UTC(+y!, +mo! - 1, +d!, +h, +mi, +se);
  const date = new Date(t);
  if (
    date.getUTCMonth() !== +mo! - 1 ||
    date.getUTCDate() !== +d! ||
    +h > 23 ||
    +mi > 59 ||
    +se > 59
  )
    return null;
  return t;
}
const iso = (ctx: Ctx, path: (string | number)[], s: string): number | null => {
  const t = isoDatetime(s);
  if (t === null)
    issue(
      ctx,
      path,
      `${JSON.stringify(s)} is not an ISO date (YYYY-MM-DD) or datetime (RFC 3339)`,
    );
  return t;
};

// ── Graph pieces (shared with `library.ts`'s Graph and with GraphDelta) ──

export const graphNode = z.object({
  id: str.describe("unique node id"),
  label: str.describe("text in the shape, ≤ 80 chars"),
  kind: z
    .enum([
      "box",
      "rounded",
      "pill",
      "diamond",
      "cylinder",
      "actor",
      "hexagon",
      "circle",
      "note",
    ])
    .optional(),
  group: str.optional().describe("id of a group"),
  state,
  sub: str.optional().describe("mono second line, ≤ 40 chars"),
  tag: str.optional().describe("small tag, ≤ 12 chars"),
  icon: z
    .enum([
      "server",
      "database",
      "cloud",
      "user",
      "users",
      "globe",
      "lock",
      "shield",
      "key",
      "queue",
      "cpu",
      "box",
      "package",
      "file",
      "folder",
      "terminal",
      "laptop",
      "phone",
      "mail",
      "clock",
      "settings",
      "code",
      "git-branch",
      "bot",
      "browser",
    ])
    .optional(),
});
export const graphEdge = z.object({
  from: str,
  to: str,
  label: str.optional(),
  style: lineStyle,
  state,
  arrow: z.enum(["forward", "both", "none"]).optional(),
});
export const graphGroup = z.object({
  id: str,
  label: str,
  parent: str.optional().describe("enclosing group id, depth ≤ 3"),
  kind: z.enum(["zone", "boundary", "cluster"]).optional(),
});
export const graphSide = z.object({
  nodes: z.array(graphNode),
  edges: z.array(graphEdge).describe("from/to name node ids"),
  groups: z.array(graphGroup).optional(),
});
type Side = z.infer<typeof graphSide>;

/** `crates/surface::validate::graph_side`: caps, unique ids, resolving ends/groups/parents, depth, label lengths. */
export function graphRules(g: Side, ctx: Ctx, at: (string | number)[] = []) {
  const p = (...rest: (string | number)[]) => [...at, ...rest];
  cap(ctx, p("nodes"), g.nodes.length, CAPS.graphNodes, "nodes");
  cap(ctx, p("edges"), g.edges.length, CAPS.graphEdges, "edges");
  const groups = g.groups ?? [];
  cap(ctx, p("groups"), groups.length, CAPS.graphGroups, "groups");
  const groupIds = unique(
    ctx,
    p("groups"),
    groups.map((x) => x.id),
    "group",
  );
  const parentOf = new Map(
    groups.filter((x) => x.parent !== undefined).map((x) => [x.id, x.parent!]),
  );
  groups.forEach((grp, i) => {
    label(ctx, p("groups", i, "label"), grp.label);
    if (grp.parent === undefined) return;
    const path = p("groups", i, "parent");
    if (!groupIds.has(grp.parent)) {
      issue(ctx, path, `unknown group ${JSON.stringify(grp.parent)}`);
      return;
    }
    let depth = 1;
    let cur: string | undefined = grp.parent;
    while (cur !== undefined) {
      depth += 1;
      if (cur === grp.id) {
        issue(ctx, path, `group ${JSON.stringify(grp.id)} contains itself`);
        break;
      }
      if (depth > CAPS.graphDepth) {
        issue(ctx, path, `nesting depth ${depth}, limit ${CAPS.graphDepth}`);
        break;
      }
      cur = parentOf.get(cur);
    }
  });
  const nodeIds = unique(
    ctx,
    p("nodes"),
    g.nodes.map((n) => n.id),
    "node",
  );
  g.nodes.forEach((n, i) => {
    if (n.group !== undefined)
      resolves(ctx, p("nodes", i, "group"), n.group, groupIds, "group");
    label(ctx, p("nodes", i, "label"), n.label);
    short(ctx, p("nodes", i, "sub"), n.sub, CAPS.sub);
    short(ctx, p("nodes", i, "tag"), n.tag, CAPS.tag);
  });
  g.edges.forEach((e, i) => {
    resolves(ctx, p("edges", i, "from"), e.from, nodeIds, "node");
    resolves(ctx, p("edges", i, "to"), e.to, nodeIds, "node");
    if (e.label !== undefined) label(ctx, p("edges", i, "label"), e.label);
  });
}

// ── The fifteen diagram components the apps library registers (catalog order) ──

export const stateMachineProps = z
  .object({
    states: z.array(
      z.object({
        id: str,
        label: str,
        kind: z.enum(["initial", "final", "state", "choice"]).optional(),
        sub: str.optional(),
        state,
        ref,
      }),
    ),
    transitions: z.array(
      z.object({
        from: str,
        to: str,
        label: str.optional(),
        guard: str.optional(),
        style: lineStyle,
        state,
      }),
    ),
    direction,
    title,
  })
  .superRefine((m, ctx) => {
    cap(ctx, ["states"], m.states.length, CAPS.states, "states");
    cap(ctx, ["transitions"], m.transitions.length, CAPS.transitions, "transitions");
    const ids = unique(ctx, ["states"], m.states.map((s) => s.id), "state");
    let initial: number | null = null;
    m.states.forEach((s, i) => {
      label(ctx, ["states", i, "label"], s.label);
      short(ctx, ["states", i, "sub"], s.sub, CAPS.sub);
      if (s.kind === "initial") {
        if (initial === null) initial = i;
        else
          issue(ctx, ["states", i, "kind"], `second initial state; the first is states/${initial}`);
      }
    });
    m.transitions.forEach((t, i) => {
      resolves(ctx, ["transitions", i, "from"], t.from, ids, "state");
      resolves(ctx, ["transitions", i, "to"], t.to, ids, "state");
      if (t.label !== undefined) label(ctx, ["transitions", i, "label"], t.label);
      short(ctx, ["transitions", i, "guard"], t.guard, CAPS.sub);
    });
  });

export const entitiesProps = z
  .object({
    entities: z.array(
      z.object({
        id: str,
        label: str,
        kind: z.enum(["entity", "table", "class", "interface", "enum"]).optional(),
        fields: z.array(
          z.object({
            name: str,
            type: str.optional(),
            key: z.enum(["pk", "fk", "uk"]).optional(),
            nullable: z.boolean().optional(),
            note: str.optional(),
          }),
        ),
        methods: z.array(z.object({ name: str, sig: str.optional() })).optional(),
        state,
        ref,
      }),
    ),
    relations: z.array(
      z.object({
        from: str,
        to: str,
        kind: z.enum([
          "one-one",
          "one-many",
          "many-many",
          "inherits",
          "implements",
          "composes",
          "aggregates",
          "depends",
          "references",
        ]),
        label: str.optional(),
        from_field: str.optional(),
        to_field: str.optional(),
      }),
    ),
    detail: z.enum(["model", "schema", "class"]).optional(),
    title,
  })
  .superRefine((e, ctx) => {
    cap(ctx, ["entities"], e.entities.length, CAPS.entities, "entities");
    cap(ctx, ["relations"], e.relations.length, CAPS.relations, "relations");
    const ids = unique(ctx, ["entities"], e.entities.map((x) => x.id), "entity");
    const fieldsOf = new Map<string, Set<string>>();
    e.entities.forEach((en, i) => {
      label(ctx, ["entities", i, "label"], en.label);
      cap(ctx, ["entities", i, "fields"], en.fields.length, CAPS.fields, "fields");
      const names = new Set<string>();
      en.fields.forEach((f, j) => {
        if (names.has(f.name))
          issue(ctx, ["entities", i, "fields", j, "name"], `duplicate field name ${JSON.stringify(f.name)}`);
        names.add(f.name);
      });
      if (en.methods !== undefined && en.kind !== "class" && en.kind !== "interface")
        issue(ctx, ["entities", i, "methods"], 'methods need kind "class" or "interface"');
      if (!fieldsOf.has(en.id)) fieldsOf.set(en.id, names);
    });
    e.relations.forEach((r, i) => {
      for (const [end, id, field] of [
        ["from", r.from, r.from_field],
        ["to", r.to, r.to_field],
      ] as const) {
        resolves(ctx, ["relations", i, end], id, ids, "entity");
        const names = fieldsOf.get(id);
        if (field !== undefined && names && !names.has(field))
          issue(ctx, ["relations", i, `${end}_field`], `unknown field ${JSON.stringify(field)} on ${JSON.stringify(id)}`);
      }
      if (r.label !== undefined) label(ctx, ["relations", i, "label"], r.label);
    });
  });

export const swimlaneProps = z
  .object({
    lanes: z.array(z.object({ id: str, label: str })),
    steps: z.array(
      z.object({
        id: str,
        lane: str,
        label: str,
        kind: z.enum(["step", "decision", "start", "end", "data"]).optional(),
        sub: str.optional(),
        state,
        ref,
      }),
    ),
    edges: z.array(z.object({ from: str, to: str, label: str.optional(), style: lineStyle })),
    direction,
    title,
  })
  .superRefine((s, ctx) => {
    cap(ctx, ["lanes"], s.lanes.length, CAPS.lanes, "lanes");
    cap(ctx, ["steps"], s.steps.length, CAPS.laneSteps, "steps");
    cap(ctx, ["edges"], s.edges.length, CAPS.laneEdges, "edges");
    const lanes = unique(ctx, ["lanes"], s.lanes.map((l) => l.id), "lane");
    s.lanes.forEach((l, i) => label(ctx, ["lanes", i, "label"], l.label));
    const steps = unique(ctx, ["steps"], s.steps.map((st) => st.id), "step");
    s.steps.forEach((st, i) => {
      resolves(ctx, ["steps", i, "lane"], st.lane, lanes, "lane");
      label(ctx, ["steps", i, "label"], st.label);
      short(ctx, ["steps", i, "sub"], st.sub, CAPS.sub);
    });
    s.edges.forEach((e, i) => {
      resolves(ctx, ["edges", i, "from"], e.from, steps, "step");
      resolves(ctx, ["edges", i, "to"], e.to, steps, "step");
      if (e.label !== undefined) label(ctx, ["edges", i, "label"], e.label);
    });
  });

export const graphDeltaProps = z
  .object({ before: graphSide, after: graphSide, direction, title })
  .superRefine((d, ctx) => {
    graphRules(d.before, ctx, ["before"]);
    graphRules(d.after, ctx, ["after"]);
  });

export const waterfallProps = z
  .object({
    steps: z.array(
      z.object({
        label: str,
        value: num,
        kind: z.enum(["delta", "total", "subtotal"]).optional(),
        state,
      }),
    ),
    unit: str.optional(),
    title,
  })
  .superRefine((w, ctx) => {
    cap(ctx, ["steps"], w.steps.length, CAPS.waterfall, "steps");
    w.steps.forEach((s, i) => label(ctx, ["steps", i, "label"], s.label));
  });

export const funnelProps = z
  .object({
    stages: z.array(
      z.object({ label: str, value: num.optional(), note: str.optional(), state }),
    ),
    variant: z.enum(["funnel", "pyramid"]).optional(),
    unit: str.optional(),
    title,
  })
  .superRefine((f, ctx) => {
    cap(ctx, ["stages"], f.stages.length, CAPS.funnel, "stages");
    const withValue = f.stages.filter((s) => s.value !== undefined).length;
    if (withValue !== 0 && withValue !== f.stages.length)
      f.stages.forEach((s, i) => {
        if (s.value === undefined)
          issue(ctx, ["stages", i, "value"], "missing; values go on every stage or none");
      });
    f.stages.forEach((s, i) => label(ctx, ["stages", i, "label"], s.label));
  });

export const ganttProps = z
  .object({
    tasks: z.array(
      z.object({
        id: str,
        label: str,
        start: str.describe("ISO date or datetime"),
        end: str.describe("ISO date or datetime, ≥ start"),
        group: str.optional(),
        state,
        progress: num.min(0).max(1).optional(),
        depends: z.array(str).optional(),
        ref,
      }),
    ),
    today: str.optional(),
    title,
  })
  .superRefine((g, ctx) => {
    cap(ctx, ["tasks"], g.tasks.length, CAPS.gantt, "tasks");
    if (g.today !== undefined) iso(ctx, ["today"], g.today);
    const ids = unique(ctx, ["tasks"], g.tasks.map((t) => t.id), "task");
    g.tasks.forEach((t, i) => {
      label(ctx, ["tasks", i, "label"], t.label);
      const start = iso(ctx, ["tasks", i, "start"], t.start);
      const end = iso(ctx, ["tasks", i, "end"], t.end);
      if (start !== null && end !== null && end < start)
        issue(ctx, ["tasks", i, "end"], `end ${JSON.stringify(t.end)} is before start ${JSON.stringify(t.start)}`);
      (t.depends ?? []).forEach((d, j) => {
        if (d === t.id)
          issue(ctx, ["tasks", i, "depends", j], `task ${JSON.stringify(d)} depends on itself`);
        else resolves(ctx, ["tasks", i, "depends", j], d, ids, "task");
      });
    });
  });

export const quadrantProps = z
  .object({
    xLabel: str,
    yLabel: str,
    items: z.array(
      z.object({
        id: str.optional(),
        label: str,
        x: num.min(0).max(1),
        y: num.min(0).max(1),
        size: num.optional(),
        state,
        ref,
      }),
    ),
    quadrants: z.array(str).optional().describe("exactly 4: tl, tr, bl, br"),
    xTicks: z.array(str).optional().describe("exactly 2: low, high"),
    yTicks: z.array(str).optional().describe("exactly 2: low, high"),
    title,
  })
  .superRefine((q, ctx) => {
    cap(ctx, ["items"], q.items.length, CAPS.quadrant, "items");
    for (const [name, want, list] of [
      ["quadrants", 4, q.quadrants],
      ["xTicks", 2, q.xTicks],
      ["yTicks", 2, q.yTicks],
    ] as const)
      if (list !== undefined && list.length !== want)
        issue(ctx, [name], `${list.length} ${name}, expected exactly ${want}`);
    unique(ctx, ["items"], q.items.map((i) => i.id).filter((i): i is string => i !== undefined), "item");
    q.items.forEach((it, i) => label(ctx, ["items", i, "label"], it.label));
  });

export const treemapProps = z
  .object({
    items: z.array(
      z.object({ label: str, value: num, group: str.optional(), state, ref }),
    ),
    title,
  })
  .superRefine((t, ctx) => {
    cap(ctx, ["items"], t.items.length, CAPS.treemap, "items");
    const groups = new Set(t.items.map((c) => c.group).filter((g) => g !== undefined));
    cap(ctx, ["items"], groups.size, CAPS.treemapGroups, "groups");
    t.items.forEach((c, i) => {
      if (!(c.value > 0)) issue(ctx, ["items", i, "value"], `${c.value} is not > 0`);
      label(ctx, ["items", i, "label"], c.label);
    });
  });

export const sankeyProps = z
  .object({
    nodes: z.array(z.object({ id: str, label: str, stage: uint.optional(), state })),
    flows: z.array(
      z.object({ from: str, to: str, value: num, label: str.optional(), state }),
    ),
    unit: str.optional(),
    title,
  })
  .superRefine((s, ctx) => {
    cap(ctx, ["nodes"], s.nodes.length, CAPS.sankeyNodes, "nodes");
    cap(ctx, ["flows"], s.flows.length, CAPS.sankeyFlows, "flows");
    const ids = unique(ctx, ["nodes"], s.nodes.map((n) => n.id), "node");
    s.nodes.forEach((n, i) => label(ctx, ["nodes", i, "label"], n.label));
    const out = new Map<string, [number, string][]>();
    s.flows.forEach((f, i) => {
      resolves(ctx, ["flows", i, "from"], f.from, ids, "node");
      resolves(ctx, ["flows", i, "to"], f.to, ids, "node");
      if (f.from === f.to)
        issue(ctx, ["flows", i, "to"], `flow from ${JSON.stringify(f.from)} to itself`);
      else {
        if (!out.has(f.from)) out.set(f.from, []);
        out.get(f.from)!.push([i, f.to]);
      }
      if (!(f.value > 0)) issue(ctx, ["flows", i, "value"], `${f.value} is not > 0`);
      if (f.label !== undefined) label(ctx, ["flows", i, "label"], f.label);
    });
    // Iterative DFS in authored order; the first back edge names the cycle (as `validate.rs`).
    const done = new Set<string>();
    for (const root of s.nodes.map((n) => n.id)) {
      if (done.has(root)) continue;
      const stack: [string, number][] = [[root, 0]];
      const on: string[] = [root];
      while (stack.length) {
        const top = stack[stack.length - 1]!;
        const edges = out.get(top[0]) ?? [];
        const next = edges[top[1]];
        if (!next) {
          done.add(top[0]);
          stack.pop();
          on.pop();
          continue;
        }
        top[1] += 1;
        const [fi, to] = next;
        const start = on.indexOf(to);
        if (start >= 0) {
          issue(ctx, ["flows", fi], `circular flow ${[...on.slice(start), to].join(" → ")}`);
          for (const n of on) done.add(n);
          break;
        }
        if (!done.has(to)) {
          stack.push([to, 0]);
          on.push(to);
        }
      }
    }
  });

export const vennProps = z
  .object({
    sets: z.array(z.object({ id: str, label: str, size: num.optional() })),
    overlaps: z.array(z.object({ sets: z.array(str), label: str })).optional(),
    title,
  })
  .superRefine((v, ctx) => {
    within(ctx, ["sets"], v.sets.length, 2, 3, "sets");
    const ids = unique(ctx, ["sets"], v.sets.map((s) => s.id), "set");
    v.sets.forEach((s, i) => label(ctx, ["sets", i, "label"], s.label));
    const seen = new Set<string>();
    (v.overlaps ?? []).forEach((o, i) => {
      label(ctx, ["overlaps", i, "label"], o.label);
      const key = [...new Set(o.sets)].sort();
      if (key.length !== o.sets.length || key.length < 2 || key.length > 3)
        issue(ctx, ["overlaps", i, "sets"], `${o.sets.length} sets, expected 2 or 3 distinct`);
      o.sets.forEach((id, j) => resolves(ctx, ["overlaps", i, "sets", j], id, ids, "set"));
      const k = key.join("\u0001");
      if (seen.has(k)) issue(ctx, ["overlaps", i, "sets"], "duplicate overlap");
      seen.add(k);
    });
  });

export const loopProps = z
  .object({
    steps: z.array(z.object({ label: str, note: str.optional(), state, ref })),
    hub: str.optional(),
    title,
  })
  .superRefine((l, ctx) => {
    within(ctx, ["steps"], l.steps.length, 2, 8, "steps");
    l.steps.forEach((s, i) => label(ctx, ["steps", i, "label"], s.label));
  });

export const layersProps = z
  .object({
    layers: z.array(
      z.object({
        label: str,
        items: z.array(str).optional(),
        note: str.optional(),
        state,
        ref,
      }),
    ),
    direction: z.enum(["top-down", "bottom-up"]).optional(),
    title,
  })
  .superRefine((l, ctx) => {
    cap(ctx, ["layers"], l.layers.length, CAPS.layers, "layers");
    l.layers.forEach((layer, i) => {
      label(ctx, ["layers", i, "label"], layer.label);
      cap(ctx, ["layers", i, "items"], layer.items?.length ?? 0, CAPS.layers, "items");
    });
  });

export const journeyProps = z
  .object({
    stages: z.array(str),
    rows: z.array(z.object({ label: str, cells: z.array(str) })),
    mood: z.array(num).optional(),
    pains: z.array(z.object({ stage: uint, text: str })).optional(),
    title,
  })
  .superRefine((j, ctx) => {
    const n = j.stages.length;
    cap(ctx, ["stages"], n, CAPS.journeyStages, "stages");
    cap(ctx, ["rows"], j.rows.length, CAPS.journeyRows, "rows");
    j.rows.forEach((r, i) => {
      if (r.cells.length !== n)
        issue(ctx, ["rows", i, "cells"], `${r.cells.length} cells, ${n} stages`);
    });
    if (j.mood !== undefined) {
      if (j.mood.length !== n) issue(ctx, ["mood"], `${j.mood.length} moods, ${n} stages`);
      j.mood.forEach((v, i) => {
        if (v < -1 || v > 1) issue(ctx, ["mood", i], `${v} is outside -1..=1`);
      });
    }
    (j.pains ?? []).forEach((p, i) => {
      if (p.stage >= n)
        issue(ctx, ["pains", i, "stage"], `stage ${p.stage} does not exist (${n} stages)`);
      label(ctx, ["pains", i, "text"], p.text);
    });
  });

export const fishboneProps = z
  .object({
    effect: str,
    causes: z.array(z.object({ category: str, items: z.array(str) })),
    title,
  })
  .superRefine((f, ctx) => {
    label(ctx, ["effect"], f.effect);
    cap(ctx, ["causes"], f.causes.length, CAPS.fishbone, "causes");
    f.causes.forEach((c, i) => {
      label(ctx, ["causes", i, "category"], c.category);
      cap(ctx, ["causes", i, "items"], c.items.length, CAPS.fishboneItems, "items");
    });
  });

/** One registration: the wire schema, the skill-facing description and the text fallback (`<Name>: n <units>`). */
export interface DiagramDef {
  props: z.ZodObject;
  description: string;
  /** `[count, unit]` for the text fallback, e.g. `[6, "tasks"]`. */
  count: (props: Record<string, unknown>) => [number, string];
}

const len = (v: unknown): number => (Array.isArray(v) ? v.length : 0);

/** The fifteen registrations in catalog order (the names the hash appends after `OptionCard`). */
export const DIAGRAMS: Record<string, DiagramDef> = {
  StateMachine: {
    props: stateMachineProps,
    description:
      "States and transitions of a lifecycle: states (id, label, kind? initial|final|state|choice, sub?, state?), transitions (from, to, label? = event, guard?, style?, state?). `highlight` lights state ids; `onSelect` runs with the pressed state id as $value.",
    count: (p) => [len(p.states), "states"],
  },
  Entities: {
    props: entitiesProps,
    description:
      "Entities, tables or classes with fields and typed relations (one-one|one-many|many-many|inherits|implements|composes|aggregates|depends|references). `highlight` lights entity ids; `onSelect` runs with the pressed entity id as $value.",
    count: (p) => [len(p.entities), "entities"],
  },
  Swimlane: {
    props: swimlaneProps,
    description:
      "Steps in lanes (teams, systems) with edges across lanes: lanes (id, label), steps (id, lane, label, kind?, sub?, state?), edges (from, to, label?, style?). `highlight` lights step ids; `onSelect` runs with the pressed step id as $value.",
    count: (p) => [len(p.steps), "steps"],
  },
  GraphDelta: {
    props: graphDeltaProps,
    description:
      "Two graphs, before and after, over one layout; the added/removed/changed ledger is derived by node id. `highlight` lights node ids; `onSelect` runs with the pressed node id as $value.",
    count: (p) => [len((p.after as { nodes?: unknown[] } | undefined)?.nodes), "nodes"],
  },
  Waterfall: {
    props: waterfallProps,
    description:
      "Deltas carrying a running total between absolute levels: steps (label, value, kind? delta|total|subtotal, state?), unit?. `highlight` lights step indices as strings; `onSelect` runs with the pressed index as $value.",
    count: (p) => [len(p.steps), "steps"],
  },
  Funnel: {
    props: funnelProps,
    description:
      "Stages narrowing (funnel) or widening (pyramid) with values on all stages or none. `highlight` lights stage indices as strings; `onSelect` runs with the pressed index as $value.",
    count: (p) => [len(p.stages), "stages"],
  },
  Gantt: {
    props: ganttProps,
    description:
      "Tasks on a date scale: tasks (id, label, start, end as ISO dates, group?, state?, progress? 0..1, depends? ids), today?. `highlight` lights task ids; `onSelect` runs with the pressed task id as $value.",
    count: (p) => [len(p.tasks), "tasks"],
  },
  Quadrant: {
    props: quadrantProps,
    description:
      "Items in a 2×2 by two 0..1 coordinates (xLabel, yLabel, items (id?, label, x, y, size?, state?), quadrants? exactly 4, xTicks?/yTicks? exactly 2). `highlight` lights item ids; `onSelect` runs with the pressed item id as $value.",
    count: (p) => [len(p.items), "items"],
  },
  Treemap: {
    props: treemapProps,
    description:
      "Part-of-whole as nested rectangles sized by value: items (label, value > 0, group?, state?). `highlight` lights item labels; `onSelect` runs with the pressed label as $value.",
    count: (p) => [len(p.items), "items"],
  },
  Sankey: {
    props: sankeyProps,
    description:
      "Flows between staged nodes, ribbons sized by value, no cycles: nodes (id, label, stage?, state?), flows (from, to, value > 0, label?, state?), unit?. `highlight` lights node ids; `onSelect` runs with the pressed node id as $value.",
    count: (p) => [len(p.flows), "flows"],
  },
  Venn: {
    props: vennProps,
    description:
      "Two or three overlapping sets (id, label, size?) with labelled overlaps (sets, label). `highlight` lights set ids; `onSelect` runs with the pressed set id as $value.",
    count: (p) => [len(p.sets), "sets"],
  },
  Loop: {
    props: loopProps,
    description:
      "A cycle of 2–8 steps (label, note?, state?) drawn clockwise around an optional hub. `highlight` lights step indices as strings; `onSelect` runs with the pressed index as $value.",
    count: (p) => [len(p.steps), "steps"],
  },
  Layers: {
    props: layersProps,
    description:
      "Stacked horizontal layers (label, items?, note?, state?), top-down or bottom-up. `highlight` lights layer indices as strings; `onSelect` runs with the pressed index as $value.",
    count: (p) => [len(p.layers), "layers"],
  },
  Journey: {
    props: journeyProps,
    description:
      "Stages across, rows down (label, cells one per stage), a mood line (−1..1 per stage) and pain points (stage index, text). `highlight` lights stage indices as strings; `onSelect` runs with the pressed index as $value.",
    count: (p) => [len(p.stages), "stages"],
  },
  Fishbone: {
    props: fishboneProps,
    description:
      "Cause and effect: an effect with categorised causes (category, items) on the bones. `highlight` lights category indices as strings; `onSelect` runs with the pressed index as $value.",
    count: (p) => [len(p.causes), "causes"],
  },
};

/** The text a diagram shows without a host renderer: `Gantt: 6 tasks` (what the headless helper and tests see). */
export function diagramText(name: string, props: Record<string, unknown>): string {
  const def = DIAGRAMS[name];
  if (!def) return name;
  const [n, unit] = def.count(props);
  const t = typeof props.title === "string" && props.title ? ` ${props.title} —` : "";
  return `${name}:${t} ${n} ${n === 1 ? unit.replace(/ies$/, "y").replace(/s$/, "") : unit}`;
}
