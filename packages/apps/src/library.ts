/**
 * Curated, dependency-closed subset of the upstream `openuiLibrary` (design 46 §4).
 *
 * Every component keeps its upstream renderer, description and field schemas. Only fields that
 * reference other components (child unions such as `Card.children`) are rebuilt so they mention
 * exclusively components in this subset; everything else keeps object identity, so upstream
 * `reactive()` markers, `tagSchemaId` tags and `.describe()` metadata survive untouched.
 *
 * This module pulls the whole react-ui graph and is for the frontend and the generation script.
 * The headless helper never imports it; it reads `generated/library.json`.
 */
import { createLibrary } from "@openuidev/react-lang";
import type {
  ComponentGroup,
  DefinedComponent,
  Library,
} from "@openuidev/react-lang";
import {
  defineComponent as defineReactComponent,
  reactive,
  useStateField,
  useTriggerAction,
} from "@openuidev/react-lang";
import {
  defineComponent,
  isReactiveSchema,
  type ASTNode,
} from "@openuidev/lang-core";
import {
  openuiComponentGroups,
  openuiLibrary,
} from "@openuidev/react-ui/genui-lib";
import { type ComponentType, createElement } from "react";
import { z } from "zod/v4";
import type * as core from "zod/v4/core";

/** Ordered component names; order is part of the compatibility hash. */
export const CURATED_COMPONENTS = [
  // layout
  "Stack",
  "Card",
  "CardHeader",
  "Separator",
  "Tabs",
  "TabItem",
  "Accordion",
  "AccordionItem",
  "Carousel",
  // content
  "TextContent",
  "MarkDownRenderer",
  "CodeBlock",
  "Callout",
  "TextCallout",
  "Steps",
  "StepsItem",
  "MetricIndicatorInline",
  "ListBlock",
  "ListItem",
  "Tag",
  "TagBlock",
  "Icon",
  "Label",
  "Image",
  "ImageGallery",
  // data
  "Table",
  "Col",
  "BarChart",
  "HorizontalBarChart",
  "LineChart",
  "AreaChart",
  "RadarChart",
  "PieChart",
  "SingleStackedBarChart",
  "Series",
  "ScatterChart",
  "ScatterSeries",
  "Point",
  // input
  "Input",
  "TextArea",
  "Select",
  "SelectItem",
  "SwitchGroup",
  "SwitchItem",
  "RadioGroup",
  "RadioItem",
  "CheckBoxGroup",
  "CheckBoxItem",
  "Chips",
  "ChipItem",
  "Slider",
  // actions
  "Button",
  "Buttons",
  // theuth (65 §3.5): the one display component no upstream component expresses.
  "Graph",
  // forms (appended 2026-09-25; order is part of the hash)
  "OptionCards",
  "OptionCard",
] as const;

export type CuratedComponentName = (typeof CURATED_COMPONENTS)[number];

export const LIBRARY_ROOT: CuratedComponentName = "Stack";

type AnyZod = core.$ZodType;
type UpstreamComponent = (typeof openuiLibrary.components)[string];

interface Def {
  type: string;
  innerType?: AnyZod;
  element?: AnyZod;
  options?: AnyZod[];
  shape?: Record<string, AnyZod>;
  defaultValue?: unknown;
}

const def = (s: AnyZod): Def => s._zod.def as unknown as Def;

/** Copy `.describe()`/`.meta()` metadata from an upstream schema onto a rebuilt one. */
function withMeta<T extends AnyZod>(from: AnyZod, to: T): T {
  const meta = z.globalRegistry.get(from);
  return meta ? ((to as unknown as z.ZodType).meta(meta) as unknown as T) : to;
}

// ── Graph (design 65 §3.5): zod mirror of `crates/surface` `GraphProps`, plus reactive `highlight` and `onSelect` ──

const graphStr = z.string();
const graphNode = z.object({
  id: graphStr.describe("unique node id"),
  label: graphStr.describe("text in the shape, ≤ 80 chars"),
  kind: z
    .enum(["box", "rounded", "pill", "diamond", "cylinder", "actor"])
    .optional(),
  group: graphStr.optional().describe("id of a group"),
  state: z.enum(["default", "active", "done", "muted", "danger"]).optional(),
});
const graphEdge = z.object({
  from: graphStr,
  to: graphStr,
  label: graphStr.optional(),
  style: z.enum(["solid", "dashed"]).optional(),
});
const graphGroup = z.object({ id: graphStr, label: graphStr });

/** `crates/surface::MAX_GRAPH_*`. */
const GRAPH_MAX = { nodes: 120, edges: 240, groups: 12, label: 80 };

/** `Button.action`'s schema object itself, so the prompt renders `ActionExpression` for it too. */
const buttonAction = def(openuiLibrary.components.Button!.props as AnyZod)
  .shape!.action!;

/**
 * `Graph` props. The structural rules (`crates/surface/src/validate.rs`) are a `superRefine`: JSON Schema cannot
 * carry them, so the parser never sees them; the parity test does.
 */
export const graphProps = z
  .object({
    nodes: z.array(graphNode),
    edges: z.array(graphEdge).describe("from/to name node ids"),
    groups: z.array(graphGroup).optional(),
    direction: z.enum(["LR", "TB"]).optional(),
    // `reactive()` marks the object it is given; `.describe()` clones, so it goes inside.
    highlight: reactive(
      z.array(graphStr).optional().describe("node ids drawn active"),
    ),
    onSelect: (buttonAction as unknown as z.ZodType).describe(
      "runs when a node is pressed; $value is the node id",
    ),
  })
  .superRefine((g, ctx) => {
    const bad = (path: (string | number)[], message: string) =>
      ctx.addIssue({ code: "custom", path, message });
    if (g.nodes.length > GRAPH_MAX.nodes)
      bad(["nodes"], `${g.nodes.length} nodes, limit ${GRAPH_MAX.nodes}`);
    if (g.edges.length > GRAPH_MAX.edges)
      bad(["edges"], `${g.edges.length} edges, limit ${GRAPH_MAX.edges}`);
    const groups = g.groups ?? [];
    if (groups.length > GRAPH_MAX.groups)
      bad(["groups"], `${groups.length} groups, limit ${GRAPH_MAX.groups}`);
    const gids = new Set<string>();
    groups.forEach((grp, i) => {
      if (gids.has(grp.id))
        bad(["groups", i, "id"], `duplicate group id ${JSON.stringify(grp.id)}`);
      gids.add(grp.id);
      if ([...grp.label].length > GRAPH_MAX.label)
        bad(["groups", i, "label"], `over ${GRAPH_MAX.label} characters`);
    });
    const nids = new Set<string>();
    g.nodes.forEach((n, i) => {
      if (nids.has(n.id))
        bad(["nodes", i, "id"], `duplicate node id ${JSON.stringify(n.id)}`);
      nids.add(n.id);
      if (n.group !== undefined && !gids.has(n.group))
        bad(["nodes", i, "group"], `unknown group ${JSON.stringify(n.group)}`);
      if ([...n.label].length > GRAPH_MAX.label)
        bad(["nodes", i, "label"], `over ${GRAPH_MAX.label} characters`);
    });
    g.edges.forEach((e, i) => {
      for (const end of ["from", "to"] as const)
        if (!nids.has(e[end]))
          bad(["edges", i, end], `unknown node ${JSON.stringify(e[end])}`);
      if (e.label !== undefined && [...e.label].length > GRAPH_MAX.label)
        bad(["edges", i, "label"], `over ${GRAPH_MAX.label} characters`);
    });
  });

export type GraphPropsValue = z.infer<typeof graphProps>;
/** The graph as the host renderer draws it: the wire props without the app-only `highlight`/`onSelect`. */
export type GraphValue = Omit<GraphPropsValue, "highlight" | "onSelect">;

/** What the host's Graph renderer receives. */
export interface GraphHostProps {
  props: GraphValue;
  /** Node ids to draw `active` (a node's own `danger`/`done` wins). */
  highlight?: readonly string[];
  /** A node was pressed. */
  onSelect?: (id: string) => void;
}

let graphHost: ComponentType<GraphHostProps> | null = null;

/**
 * Install the host's Graph renderer (the desktop's `GraphView`, `crates/desktop/ui`). This package has no
 * renderer of its own: until one is installed the component shows the adjacency text, which is also what
 * the headless helper and the tests see.
 */
export function setGraphRenderer(
  component: ComponentType<GraphHostProps> | null,
): void {
  graphHost = component;
}

/** `crates/surface::graph::adjacency_lines` in one string: what renders without a host. */
export function graphText(g: GraphValue): string {
  const glyph: Record<string, string> = {
    active: "◉",
    done: "●",
    muted: "○",
    danger: "✖",
  };
  const label = new Map(g.nodes.map((n) => [n.id, n.label]));
  const line = (n: GraphValue["nodes"][number], indent: string) => {
    const targets = g.edges
      .filter((e) => e.from === n.id)
      .map((e) =>
        e.label
          ? `${label.get(e.to) ?? e.to} (${e.label})`
          : (label.get(e.to) ?? e.to),
      );
    return `${indent}${glyph[n.state ?? ""] ?? "•"} ${n.label}${targets.length ? ` → ${targets.join(", ")}` : ""}`;
  };
  const groups = g.groups ?? [];
  const known = new Set(groups.map((grp) => grp.id));
  const out: string[] = [];
  for (const grp of groups) {
    out.push(grp.label);
    for (const n of g.nodes) if (n.group === grp.id) out.push(line(n, "  "));
  }
  for (const n of g.nodes)
    if (!(n.group !== undefined && known.has(n.group))) out.push(line(n, ""));
  return out.join("\n");
}

/**
 * `$value` → the pressed node id, in every deferred expression of an action plan. `@Set`'s value is the only
 * step upstream evaluates at press time (`@ToAssistant`/`@OpenUrl` arguments are evaluated when the props are);
 * `$value` is upstream's own name for a component's produced value in binding expressions (`resolveStateField`).
 */
function bindValue(node: ASTNode, id: string): ASTNode {
  const sub = (n: ASTNode): ASTNode => bindValue(n, id);
  switch (node.k) {
    case "StateRef":
      return node.n === "$value" ? { k: "Str", v: id } : node;
    case "Arr":
      return { ...node, els: node.els.map(sub) };
    case "Obj":
      return { ...node, entries: node.entries.map(([k, v]) => [k, sub(v)]) };
    case "Comp":
      return { ...node, args: node.args.map(sub) };
    case "Member":
      return { ...node, obj: sub(node.obj as ASTNode) };
    case "Index":
      return { ...node, obj: sub(node.obj as ASTNode), index: sub(node.index as ASTNode) };
    case "BinOp":
      return { ...node, left: sub(node.left), right: sub(node.right) };
    case "UnaryOp":
      return { ...node, operand: sub(node.operand) };
    case "Ternary":
      return { ...node, cond: sub(node.cond), then: sub(node.then), else: sub(node.else) };
    default:
      return node;
  }
}

interface Plan {
  steps: Array<{ valueAST?: ASTNode } & Record<string, unknown>>;
}

const Graph = defineReactComponent({
  name: "Graph",
  props: graphProps,
  description:
    "Typed boxes and arrows: nodes (id, label, kind?, group?, state?), directed edges (from, to, label?, style?), optional groups. `highlight` marks node ids active; `onSelect` runs an Action when a node is pressed, with `$value` bound to the node id (`@Set($selected, $value)`).",
  component: ({ props }: { props: GraphPropsValue }) => {
    const field = useStateField("highlight", props.highlight);
    const trigger = useTriggerAction();
    const { highlight: _highlight, onSelect, ...graph } = props;
    const highlight = Array.isArray(field.value)
      ? field.value.filter((s): s is string => typeof s === "string")
      : undefined;
    const plan = onSelect as Plan | undefined;
    const select =
      plan && Array.isArray(plan.steps)
        ? (id: string) =>
            trigger(id, undefined, {
              ...plan,
              steps: plan.steps.map((s) =>
                s.valueAST ? { ...s, valueAST: bindValue(s.valueAST, id) } : s,
              ),
            } as never)
        : undefined;
    const Host = graphHost;
    return Host
      ? createElement(Host, { props: graph, highlight, onSelect: select })
      : createElement("pre", { "data-graph": "" }, graphText(graph));
  },
});

/**
 * Components defined here, not upstream; they join `keep` by name like the upstream ones. The widening drops
 * the renderer's precise props type, as upstream's own `components` record does for every component.
 */
const LOCAL_COMPONENTS: Record<string, DefinedComponent> = {
  Graph: Graph as unknown as DefinedComponent,
};

/**
 * Build the curated library. `keep` must be dependency-closed: a required field whose component
 * union would become empty is a generation error, not a silent `any`.
 */
export function buildCuratedLibrary(
  keep: readonly string[] = CURATED_COMPONENTS,
  root: string = LIBRARY_ROOT,
): Library {
  const upstream = openuiLibrary.components;
  const propsToName = new Map<AnyZod, string>();
  for (const [name, comp] of Object.entries(upstream))
    propsToName.set(comp.props as AnyZod, name);
  const keepSet = new Set(keep);
  for (const name of keep)
    if (!upstream[name] && !LOCAL_COMPONENTS[name])
      throw new Error(
        `[apps/library] "${name}" is neither an upstream openuiLibrary component nor a local one`,
      );
  /** Local components' refs join every child union that holds component refs (a `Graph` inside a `Stack`). */
  const localRefs = keep
    .filter((name) => LOCAL_COMPONENTS[name] && !upstream[name])
    .map((name) => LOCAL_COMPONENTS[name]!.ref as AnyZod);

  const pruned = new Map<string, AnyZod>();
  const visiting = new Set<string>();

  /** Rebuild `schema` without references to excluded components. `undefined` = the schema itself is an excluded component ref. */
  function prune(
    schema: AnyZod,
    owner: string,
    path: string,
  ): AnyZod | undefined {
    const refName = path === "" ? undefined : propsToName.get(schema);
    if (refName !== undefined) {
      if (!keepSet.has(refName)) return undefined;
      return prunedProps(refName);
    }
    const d = def(schema);
    switch (d.type) {
      case "optional":
      case "nullable":
      case "default": {
        const inner = prune(d.innerType!, owner, path);
        if (!inner) return undefined;
        if (inner === d.innerType) return schema;
        const z4 = inner as unknown as z.ZodType;
        const wrapped =
          d.type === "optional"
            ? z4.optional()
            : d.type === "nullable"
              ? z4.nullable()
              : z4.default(d.defaultValue as never);
        return withMeta(schema, wrapped as unknown as AnyZod);
      }
      case "array": {
        const el = prune(d.element!, owner, path);
        if (!el) return undefined;
        if (el === d.element) return schema;
        return withMeta(
          schema,
          z.array(el as unknown as z.ZodType) as unknown as AnyZod,
        );
      }
      case "union": {
        const options = d.options!;
        const kept = options
          .map((o) => prune(o, owner, path))
          .filter((o): o is AnyZod => o !== undefined);
        if (kept.length === 0) return undefined;
        if (options.some((o) => propsToName.has(o))) kept.push(...localRefs);
        if (
          kept.length === options.length &&
          kept.every((o, i) => o === options[i])
        )
          return schema;
        if (kept.length === 1) return kept[0];
        return withMeta(
          schema,
          z.union(
            kept as unknown as [z.ZodType, z.ZodType, ...z.ZodType[]],
          ) as unknown as AnyZod,
        );
      }
      case "object": {
        const shape = d.shape!;
        let changed = false;
        const next: Record<string, AnyZod> = {};
        for (const [key, field] of Object.entries(shape)) {
          const out = prune(field, owner, `${path}.${key}`);
          if (!out) {
            const required = !["optional", "default", "nullable"].includes(
              def(field).type,
            );
            if (required)
              throw new Error(
                `[apps/library] ${owner}${path}.${key} requires components outside the curated set; add them to CURATED_COMPONENTS`,
              );
            changed = true;
            continue;
          }
          if (out !== field) changed = true;
          next[key] = out;
        }
        if (!changed) return schema;
        return withMeta(
          schema,
          z.object(next as Record<string, z.ZodType>) as unknown as AnyZod,
        );
      }
      default:
        return schema;
    }
  }

  function prunedProps(name: string): AnyZod {
    const done = pruned.get(name);
    if (done) return done;
    if (visiting.has(name))
      throw new Error(
        `[apps/library] cyclic component schema reference through "${name}" is not supported by pruning`,
      );
    visiting.add(name);
    const comp = upstream[name]!;
    const out = prune(comp.props as AnyZod, name, "") ?? comp.props;
    visiting.delete(name);
    pruned.set(name, out as AnyZod);
    return out as AnyZod;
  }

  const components: DefinedComponent[] = keep.map((name) => {
    if (!upstream[name]) return LOCAL_COMPONENTS[name]!;
    const comp = upstream[name]! as UpstreamComponent;
    const props = prunedProps(name);
    if (props === comp.props) return comp;
    // prune() only ever rebuilds an object root as z.object(), so the widened AnyZod is a $ZodObject here.
    const objectProps = props as core.$ZodObject;
    return defineComponent({
      name,
      props: objectProps,
      description: comp.description,
      component: comp.component,
    });
  });

  return createLibrary({
    components,
    componentGroups: curatedGroups(keepSet, Object.keys(upstream)),
    root,
    id: "theuth-apps",
  });
}

/** Upstream notes that describe excluded components without naming them. */
const MISLEADING_NOTE_PREFIXES = ["- data is an array of { id, values } rows"];

/** Upstream groups restricted to kept components; notes that mention excluded components are dropped. */
function curatedGroups(
  keep: Set<string>,
  allNames: string[],
): ComponentGroup[] {
  const excluded = allNames.filter((n) => !keep.has(n));
  const mentionsExcluded = (note: string) =>
    excluded.some((n) => new RegExp(`\\b${n}\\b`).test(note)) ||
    MISLEADING_NOTE_PREFIXES.some((p) => note.startsWith(p));
  const groups: ComponentGroup[] = [];
  for (const g of openuiComponentGroups) {
    const components = g.components.filter((c) => keep.has(c));
    if (!components.length) continue;
    const notes = g.notes?.filter((n) => !mentionsExcluded(n));
    groups.push({
      name: g.name,
      components,
      ...(notes?.length ? { notes } : {}),
    });
  }
  return groups;
}

/** Prop names whose schema carries upstream's WeakSet `reactive()` marker (`$binding<…>`), per component. */
export function reactiveProps(library: Library): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const [name, comp] of Object.entries(library.components)) {
    const shape = def(comp.props as AnyZod).shape ?? {};
    const names = Object.entries(shape)
      .filter(([, s]) => isReactiveSchema(s))
      .map(([k]) => k);
    if (names.length) out[name] = names;
  }
  return out;
}

/** The curated library used by the frontend renderer. Built once per module load. */
export const library: Library = buildCuratedLibrary();
