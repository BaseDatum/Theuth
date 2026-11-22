/**
 * Curated subset of the upstream `openuiLibrary` plus theuth's own components (design 46 §4).
 *
 * The library is derived, not rebuilt: `createLibrary` takes the upstream components with a `Card` root, then
 * `Library.extend` removes what the apps do not teach, adds theuth's components (`Graph` and the 123 diagram
 * family) into the content slots they may appear in, and adds the pure functions and the typed action. Every
 * component keeps its upstream renderer, description and field schemas; `extend` rebinds only the unions that
 * change, preserving `reactive()` markers, `tagSchemaId` tags and `.describe()` metadata.
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
  defineAction,
  defineComponent as defineReactComponent,
  defineFunction,
  reactive,
  useStateField,
  useTriggerAction,
} from "@openuidev/react-lang";
import {
  isReactiveSchema,
  type ASTNode,
} from "@openuidev/lang-core";
import {
  openuiAdditionalRules,
  openuiChatAdditionalRules,
  openuiComponentGroups,
  openuiExamples,
  openuiChatLibrary,
  openuiLibrary,
} from "@openuidev/react-ui/genui-lib";
import { type ComponentType, createElement } from "react";
import { z } from "zod/v4";
import type * as core from "zod/v4/core";
import { DIAGRAMS, diagramText, graphEdge, graphGroup, graphNode, graphRules } from "./diagrams.ts";

/**
 * Component names in library order (base components in upstream order, then the extension's additions); the order
 * is part of the compatibility hash. Upstream components left out: `RadialChart`/`Slice` (`PieChart` covers it),
 * `VisualCardBlock`/`VisualCardItem` and `MetricIndicatorWithStrikethrough` (commerce-only, image-led).
 */
export const CURATED_COMPONENTS = [
  "Card",
  "CardHeader",
  "TextContent",
  "MarkDownRenderer",
  "Callout",
  "TextCallout",
  "Image",
  "ImageBlock",
  "ImageGallery",
  "CodeBlock",
  "InlineHeader",
  "Table",
  "Col",
  "EditableTable",
  "BarChart",
  "LineChart",
  "AreaChart",
  "RadarChart",
  "HorizontalBarChart",
  "Series",
  "PieChart",
  "SingleStackedBarChart",
  "ScatterChart",
  "ScatterSeries",
  "Point",
  "Form",
  "FormControl",
  "Label",
  "Input",
  "TextArea",
  "Select",
  "SelectItem",
  "DatePicker",
  "Slider",
  "CheckBoxGroup",
  "CheckBoxItem",
  "RadioGroup",
  "RadioItem",
  "SwitchGroup",
  "SwitchItem",
  "ChipItem",
  "Chips",
  "OptionCard",
  "OptionCards",
  "Button",
  "Buttons",
  "IconButton",
  "Stack",
  "Tabs",
  "TabItem",
  "Accordion",
  "AccordionItem",
  "Steps",
  "StepsItem",
  "Carousel",
  "Separator",
  "TagBlock",
  "Tag",
  "Icon",
  "EntityList",
  "ListItem",
  "Text",
  "BoldText",
  "IconText",
  "ImageText",
  "ImageTextLarge",
  "MetricIndicatorInline",
  "SnippetCardItem",
  "SnippetCardBlock",
  "OverviewCardItem",
  "OverviewCardBlock",
  "ContextCardItem",
  "ContextCardBlock",
  "CompositeCardItem",
  "CompositeCardBlock",
  "SectionBlock",
  "SectionItem",
  "FollowUpBlock",
  "FollowUpItem",
  "ListBlock",
  "Modal",
  "Graph",
  "StateMachine",
  "Entities",
  "Swimlane",
  "GraphDelta",
  "Waterfall",
  "Funnel",
  "Gantt",
  "Quadrant",
  "Treemap",
  "Sankey",
  "Venn",
  "Loop",
  "Layers",
  "Journey",
  "Fishbone",
] as const;

export type CuratedComponentName = (typeof CURATED_COMPONENTS)[number];

/** Every app is `root = Card([...])`; `Stack` stays a component for row/column layouts inside it. */
export const LIBRARY_ROOT: CuratedComponentName = "Card";

type AnyZod = core.$ZodType;

interface Def {
  type: string;
  innerType?: AnyZod;
  element?: AnyZod;
  options?: AnyZod[];
  shape?: Record<string, AnyZod>;
  defaultValue?: unknown;
}

const def = (s: AnyZod): Def => s._zod.def as unknown as Def;

// ── Graph (design 65 §3.5, 123 §4.1): zod mirror of `crates/surface` `GraphProps` (node/edge/group shapes in
// `diagrams.ts`), plus reactive `highlight` and `onSelect` ──

const graphStr = z.string();

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
    // 123 §4.1 extensions AFTER highlight/onSelect: the positional order of 65's walkthrough apps and the `apps`
    // skill (`Graph(nodes, edges, groups, direction, highlight, onSelect)`) keeps working.
    legend: z
      .array(
        z.object({
          label: graphStr,
          state: graphNode.shape.state,
          style: graphEdge.shape.style,
          kind: graphNode.shape.kind,
        }),
      )
      .optional(),
    title: graphStr.optional(),
  })
  .superRefine((g, ctx) => graphRules(g, ctx));

export type GraphPropsValue = z.infer<typeof graphProps>;
/** The graph as the host renderer draws it: the wire props without the app-only `highlight`/`onSelect`. */
export type GraphValue = Omit<GraphPropsValue, "highlight" | "onSelect">;

/** What a host diagram renderer receives (123 §6): the wire props and the apps' reactive `highlight` / `onSelect`. */
export interface DiagramHostProps<P = Record<string, unknown>> {
  props: P;
  /** Unit ids to draw `active` (a unit's own `danger`/`done` wins). */
  highlight?: readonly string[];
  /** A unit was pressed. */
  onSelect?: (id: string) => void;
}

/** What the host's Graph renderer receives. */
export type GraphHostProps = DiagramHostProps<GraphValue>;

const hosts = new Map<string, ComponentType<DiagramHostProps<never>>>();

/**
 * Install the host's renderer for diagram `name` (the desktop's `<NameView>`, `crates/desktop/ui`). This package
 * has no renderer of its own: until one is installed the component shows its text form (`graphText` for Graph,
 * `<Name>: n <units>` otherwise), which is also what the headless helper and the tests see.
 */
export function setDiagramRenderer<P>(
  name: string,
  component: ComponentType<DiagramHostProps<P>> | null,
): void {
  if (component) hosts.set(name, component as ComponentType<DiagramHostProps<never>>);
  else hosts.delete(name);
}

/** The installed renderer for `name`, if any. */
export function diagramRenderer(
  name: string,
): ComponentType<DiagramHostProps<never>> | null {
  return hosts.get(name) ?? null;
}

/** `setDiagramRenderer("Graph", …)`. */
export function setGraphRenderer(
  component: ComponentType<GraphHostProps> | null,
): void {
  setDiagramRenderer("Graph", component);
}

/** `crates/surface::graph::adjacency_lines` in one string: what renders without a host. */
export function graphText(g: GraphValue): string {
  const glyph: Record<string, string> = {
    active: "◉",
    done: "●",
    muted: "○",
    danger: "✗",
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

/** The `onSelect` plan as a function of the pressed unit id, or `undefined` without a plan. */
function selectOf(
  trigger: ReturnType<typeof useTriggerAction>,
  onSelect: unknown,
): ((id: string) => void) | undefined {
  const plan = onSelect as Plan | undefined;
  return plan && Array.isArray(plan.steps)
    ? (id: string) =>
        trigger(id, undefined, {
          ...plan,
          steps: plan.steps.map((s) =>
            s.valueAST ? { ...s, valueAST: bindValue(s.valueAST, id) } : s,
          ),
        } as never)
    : undefined;
}

/** The reactive `highlight` field as a string array, or `undefined`. */
function highlightOf(value: unknown): string[] | undefined {
  return Array.isArray(value)
    ? value.filter((s): s is string => typeof s === "string")
    : undefined;
}

const Graph = defineReactComponent({
  name: "Graph",
  props: graphProps,
  description:
    "Typed boxes and arrows: nodes (id, label, kind?, group?, state?, sub?, tag?, icon?), directed edges (from, to, label?, style?, state?, arrow?), optional groups (nested by parent) and legend. `highlight` marks node ids active; `onSelect` runs an Action when a node is pressed, with `$value` bound to the node id (`@Set($selected, $value)`).",
  component: ({ props }: { props: GraphPropsValue }) => {
    const field = useStateField("highlight", props.highlight);
    const trigger = useTriggerAction();
    const { highlight: _highlight, onSelect, ...graph } = props;
    const Host = diagramRenderer("Graph") as ComponentType<GraphHostProps> | null;
    return Host
      ? createElement(Host, { props: graph, highlight: highlightOf(field.value), onSelect: selectOf(trigger, onSelect) })
      : createElement("pre", { "data-graph": "" }, graphText(graph));
  },
});

/**
 * One 123 diagram component (design 123 §6): the wire schema of `diagrams.ts` plus reactive `highlight` and
 * `onSelect` exactly as `Graph`; mounted through the host renderer installed by `setDiagramRenderer(name)`, else
 * its text form.
 */
function defineDiagram(name: string): DefinedComponent {
  const def = DIAGRAMS[name]!;
  const props = def.props.extend({
    highlight: reactive(
      z.array(graphStr).optional().describe("unit ids drawn active"),
    ),
    onSelect: (buttonAction as unknown as z.ZodType).describe(
      "runs when a unit is pressed; $value is the unit id",
    ),
  });
  return defineReactComponent({
    name,
    props,
    description: def.description,
    component: ({ props }: { props: Record<string, unknown> }) => {
      const field = useStateField("highlight", props.highlight);
      const trigger = useTriggerAction();
      const { highlight: _highlight, onSelect, ...wire } = props;
      const Host = diagramRenderer(name) as ComponentType<DiagramHostProps> | null;
      return Host
        ? createElement(Host, { props: wire, highlight: highlightOf(field.value), onSelect: selectOf(trigger, onSelect) })
        : createElement("pre", { "data-diagram": name }, diagramText(name, wire));
    },
  }) as unknown as DefinedComponent;
}

/**
 * Components defined here, not upstream; they join `keep` by name like the upstream ones. The widening drops
 * the renderer's precise props type, as upstream's own `components` record does for every component.
 */
const LOCAL_COMPONENTS: Record<string, DefinedComponent> = {
  Graph: Graph as unknown as DefinedComponent,
  ...Object.fromEntries(Object.keys(DIAGRAMS).map((name) => [name, defineDiagram(name)])),
};

// ── Pure functions and the typed action (library.extend `functions` / `actions`) ──

const num = (n: number, digits: number | undefined, extra: Intl.NumberFormatOptions = {}) =>
  new Intl.NumberFormat("en-US", {
    ...extra,
    ...(digits === undefined ? {} : { minimumFractionDigits: digits, maximumFractionDigits: digits }),
  }).format(n);

/** `@Percent(part, total, digits?)` → `"42%"`. Pure: fixed `en-US`, no clock, no timezone. */
const Percent = defineFunction({
  name: "Percent",
  description: 'part / total as a percentage string, e.g. @Percent(3, 4) → "75%"; 0% when total is 0',
  params: z.object({
    part: z.number(),
    total: z.number(),
    digits: z.number().int().min(0).max(6).optional().describe("fraction digits, default 0"),
  }),
  returns: z.string(),
  fn: ({ part, total, digits }) =>
    num(total === 0 ? 0 : part / total, digits ?? 0, { style: "percent" }),
});

/** `@FormatNumber(value, notation?, digits?, currency?)` → `"1.2K"`, `"$1,234.50"`. */
const FormatNumber = defineFunction({
  name: "FormatNumber",
  description:
    'number for display, en-US: @FormatNumber(1234.5) → "1,234.5"; "compact" → "1.2K"; currency is an ISO code, e.g. @FormatNumber(1234.5, "standard", 2, "USD") → "$1,234.50"',
  params: z.object({
    value: z.number(),
    notation: z.enum(["standard", "compact"]).optional().describe("default standard"),
    digits: z.number().int().min(0).max(6).optional().describe("fixed fraction digits"),
    currency: z.string().length(3).optional().describe("ISO 4217 code, e.g. USD"),
  }),
  returns: z.string(),
  fn: ({ value, notation, digits, currency }) =>
    num(value, digits, {
      notation: notation ?? "standard",
      ...(currency ? { style: "currency", currency: currency.toUpperCase() } : {}),
    }),
});

const DATE_STYLES = {
  short: { year: "numeric", month: "short", day: "numeric" },
  long: { year: "numeric", month: "long", day: "numeric" },
} satisfies Record<string, Intl.DateTimeFormatOptions>;

/** `@FormatDate(value, style?)` → `"Jan 5, 2026"`. Pure: always UTC and `en-US`; never relative to now. */
const FormatDate = defineFunction({
  name: "FormatDate",
  description:
    'ISO date/datetime string or epoch milliseconds in UTC: @FormatDate("2026-01-05") → "Jan 5, 2026"; style "iso" → "2026-01-05", "long" → "January 5, 2026", "datetime" → "Jan 5, 2026, 14:30 UTC"',
  params: z.object({
    value: z.union([z.string(), z.number()]),
    style: z.enum(["short", "long", "iso", "datetime"]).optional().describe("default short"),
  }),
  returns: z.string(),
  fn: ({ value, style }) => {
    const at = new Date(value);
    if (Number.isNaN(at.getTime())) throw new RangeError("invalid date");
    if (style === "iso") return at.toISOString().slice(0, 10);
    const date = (opts: Intl.DateTimeFormatOptions) =>
      new Intl.DateTimeFormat("en-US", { ...opts, timeZone: "UTC" }).format(at);
    // The date and the time are formatted apart: ICU joins a combined pattern with a version-dependent " at ".
    if (style === "datetime")
      return `${date(DATE_STYLES.short)}, ${date({ hour: "2-digit", minute: "2-digit", hourCycle: "h23" })} UTC`;
    return date(DATE_STYLES[style ?? "short"]);
  },
});

/**
 * `@CopyToClipboard(text)`. The host performs the copy: `onAction` receives
 * `{ type: "CopyToClipboard", params: { text }, humanFriendlyMessage }` (`LibraryActionEvent`).
 */
const CopyToClipboard = defineAction({
  name: "CopyToClipboard",
  description: "copy text to the user's clipboard (host action; use for values the user will paste elsewhere)",
  params: z.object({ text: z.string().describe("the exact text to copy") }),
});

export const LIBRARY_FUNCTIONS = [Percent, FormatNumber, FormatDate] as const;
export const LIBRARY_ACTIONS = [CopyToClipboard] as const;

// ── Extension plan ──

/**
 * Where theuth's display components may appear. `Stack.children` is `any[]`, so it needs no slot. `SectionItem` and
 * `Modal` are added by this extension, the rest are upstream content unions.
 */
const DIAGRAM_SLOTS = [
  "Card.children",
  "TabItem.content",
  "AccordionItem.content",
  "Carousel.children",
  "Modal.children",
  "SectionItem.content",
  "CompositeCardItem.body",
];

/** Upstream components that need a union they are absent from: pulled out of the base and added with slots. */
const UPSTREAM_SLOTTED: Record<string, string[]> = {
  ListBlock: ["Card.children", "TabItem.content", "AccordionItem.content", "Carousel.children", "SectionItem.content", "CompositeCardItem.body"],
  Modal: ["Card.children"],
};

/** `FollowUp*` exist upstream only in the chat library; their props reference no other component, so they are reused as is. */
const CHAT_REUSED: Record<string, string[]> = {
  FollowUpBlock: ["Card.children"],
  FollowUpItem: [],
};

const unionOptions = (schema: AnyZod): AnyZod[] => {
  const d = def(schema);
  if (d.type === "array") return unionOptions(d.element!);
  if (d.type === "optional" || d.type === "nullable") return unionOptions(d.innerType!);
  return d.options ?? [];
};

/**
 * `SectionBlock`/`SectionItem` with the app library's content: upstream's own are the chat library's, whose unions
 * name that library's `Tabs`/`Accordion`. Same renderers and descriptions; `SectionItem.content` is `TabItem.content`
 * plus `Tabs` and `Accordion`, as in the chat library.
 */
function sectionComponents(base: Library): DefinedComponent[] {
  const chat = openuiChatLibrary.components;
  const tabContent = def(base.components.TabItem!.props as AnyZod).shape!.content!;
  const nested = [...unionOptions(tabContent)];
  for (const n of ["Tabs", "Accordion"]) {
    const c = base.components[n];
    if (c) nested.push(c.props as AnyZod);
  }
  const SectionItem = defineReactComponent({
    name: "SectionItem",
    props: z.object({
      value: z.string(),
      trigger: z.string(),
      content: z.array(z.union(nested as unknown as [z.ZodType, z.ZodType, ...z.ZodType[]])),
    }),
    description: chat.SectionItem!.description,
    component: chat.SectionItem!.component as never,
  });
  const SectionBlock = defineReactComponent({
    name: "SectionBlock",
    props: z.object({
      sections: z.array(SectionItem.ref),
      isFoldable: z.boolean().optional(),
    }),
    description: chat.SectionBlock!.description,
    component: chat.SectionBlock!.component as never,
  });
  return [SectionBlock, SectionItem] as unknown as DefinedComponent[];
}

// ── Groups, notes, examples, rules ──

const TOKEN = (n: string) => new RegExp(`\\b${n}\\b`);

/** Upstream groups restricted to kept components; notes that mention an excluded component are dropped. */
function curatedGroups(keep: Set<string>, allUpstream: string[]): ComponentGroup[] {
  const excluded = allUpstream.filter((n) => !keep.has(n));
  const mentionsExcluded = (note: string) => excluded.some((n) => TOKEN(n).test(note));
  const groups: ComponentGroup[] = [];
  for (const g of openuiComponentGroups) {
    const components = g.components.filter((c) => keep.has(c));
    if (g.name === "Layout") components.push(...["SectionBlock", "SectionItem"].filter((c) => keep.has(c)));
    if (g.name === "Content") components.push(...["FollowUpBlock", "FollowUpItem"].filter((c) => keep.has(c)));
    if (!components.length) continue;
    const notes = g.notes?.filter((n) => !mentionsExcluded(n)) ?? [];
    if (g.name === "Layout")
      notes.push(
        "- The root is a Card: root = Card([...]). Use Stack inside it for rows/columns and app layouts (direction \"row\", wrap, gap, align); do not make Stack the root.",
        "- SectionBlock groups long content into foldable SectionItem(value, trigger, content[]) sections; never the first child of the root Card.",
      );
    if (g.name === "Content")
      notes.push("- FollowUpBlock([FollowUpItem(\"question\"), ...]) at the END of the root Card suggests what to ask next; pressing one sends its text to the assistant.");
    groups.push({ name: g.name, components, ...(notes.length ? { notes } : {}) });
  }
  const diagrams = ["Graph", ...Object.keys(DIAGRAMS)].filter((c) => keep.has(c));
  if (diagrams.length)
    groups.push({
      name: "Diagrams",
      components: diagrams,
      notes: [
        "- Every diagram takes `highlight` (a $binding of active unit ids) and `onSelect` (one action; `$value` is the pressed unit id): onSelect = @Set($selected, $value).",
        "- Diagram data is plain literals — do not compute it with @Each.",
      ],
    });
  return groups;
}

/** The rule whose text starts with `prefix`; a missing one is a generation error (upstream wording moved). */
function pick(rules: readonly string[], prefixes: readonly string[]): string[] {
  return prefixes.map((p) => {
    const hit = rules.filter((r) => r.startsWith(p));
    if (hit.length !== 1) throw new Error(`[apps/library] upstream rule "${p}" matched ${hit.length} rules`);
    return hit[0]!;
  });
}

/** Drop an excluded component from an enumeration inside an upstream rule (`A, B, VisualCardBlock)`). */
const scrub = (rule: string, excluded: readonly string[]): string =>
  excluded.reduce((r, n) => r.replace(new RegExp(`(?:,\\s*|\\s*/\\s*)${n}\\b`), ""), rule);

const EXCLUDED_UPSTREAM = ["RadialChart", "Slice", "VisualCardBlock", "VisualCardItem", "MetricIndicatorWithStrikethrough"];

/** Upstream's general examples that still teach the curated library, with the app root. */
const upstreamExamples = [0, 2, 3, 4].map((i) =>
  openuiExamples[i]!.replace(/^root = Stack\(\[/m, "root = Card(["),
);

/** theuth-only examples (checked against the generated schema in `library.test.ts`). */
const LOCAL_EXAMPLES = [
  `Example 5 — Diagram with selection, formatting and a copy action:

root = Card([head, graph, detail, copyBtn])
$selected = "api"
head = CardHeader("Request path", "Press a node")
graph = Graph([n1, n2, n3], [e1, e2], null, "LR", [$selected], @Set($selected, $value))
n1 = { id: "edge", label: "Edge" }
n2 = { id: "api", label: "API", state: "done" }
n3 = { id: "db", label: "Database", kind: "cylinder" }
e1 = { from: "edge", to: "api" }
e2 = { from: "api", to: "db" }
detail = TextContent("Selected: " + $selected + " · p99 " + @FormatNumber(1234.5, "standard", 1) + " ms · " + @Percent(97, 100) + " ok · since " + @FormatDate("2026-01-05"))
copyBtn = Button("Copy node id", @CopyToClipboard($selected), "secondary")`,
];

export const promptExamples: readonly string[] = [...upstreamExamples, ...LOCAL_EXAMPLES].map((e, i) =>
  e.replace(/^Example \d+/, `Example ${i + 1}`),
);

const UPSTREAM_RULE_PREFIXES = [
  "When asked about data",
  "For grid-like layouts",
  "For forms, define one FormControl",
  "For forms, always provide the second Form",
  "Never nest Form inside Form",
  "Use @Reset(",
  "Multi-query refresh",
  "$variables are reactive",
  "Use existing components",
  "Text / BoldText",
  "EntityList size='small'",
  "Image URLs must be real",
  "Always pass a category to Icon",
  "EditableTable requires",
];

const CHAT_RULE_PREFIXES = [
  "Use FollowUpBlock at the END",
  "Use ListBlock when presenting",
  "Use SectionItem inside SectionBlock",
  "Carousel takes an array of slides",
  "IMPORTANT: Every slide in a Carousel",
  "CardHeader is the page-level heading",
  "Never use Callout or TextCallout",
  "If no image URL is in context",
  "Argument-order trap",
  "Card blocks (",
  "The card-block action is BLOCK-LEVEL",
  "Card items have NO per-item action",
  "Tag variant must be semantically correct",
  "TagBlock takes plain strings",
  "ListBlock variant",
  "Use SectionBlock to divide a long response",
  "Accordion is for supplementary",
  "Charts: LineChart/AreaChart",
  "For large chart values",
  "Table is column-oriented",
  "Every Form MUST have EXACTLY ONE submit Button",
  "Prefer structured inputs",
  "Validation rules (",
  "Every Button label",
];

const LOCAL_RULES = [
  "Every app is root = Card([...]): Card stacks its children vertically with the house styling. Use Stack only inside it for rows/columns (direction \"row\", wrap) and compact layouts; never make Stack the root.",
  "An action is ONE bare step — Button(\"Ask\", @ToAssistant(\"Tell me more\")) — or Action([@Run(m), @Set($v, 1)]) for several steps in order.",
  "To copy a value for the user, use @CopyToClipboard(text) as a button action.",
  "Format numbers, percentages and dates for display with @FormatNumber, @Percent and @FormatDate instead of building the strings by hand.",
];

/** The curated rules in order: theuth's, then upstream's app rules, then the chat library's that still apply. */
export function promptRules(): string[] {
  const picked = [
    ...pick(openuiAdditionalRules, UPSTREAM_RULE_PREFIXES),
    ...pick(openuiChatAdditionalRules, CHAT_RULE_PREFIXES),
  ].map((r) => scrub(r, EXCLUDED_UPSTREAM));
  for (const r of picked)
    for (const n of EXCLUDED_UPSTREAM)
      if (TOKEN(n).test(r)) throw new Error(`[apps/library] rule still names excluded ${n}: ${r.slice(0, 60)}`);
  return [...LOCAL_RULES, ...picked];
}

/** `Library.prompt()` options for the apps: upstream's general guidance, filtered, plus theuth's. */
export const promptOptions = {
  examples: promptExamples as string[],
  additionalRules: promptRules(),
};

// ── Build ──

/**
 * Build the curated library. `keep` names the components that remain; a prop left with no component type by
 * removing the others is a generation error from `Library.extend`, not a silent `any`.
 */
export function buildCuratedLibrary(
  keep: readonly string[] = CURATED_COMPONENTS,
  root: string = LIBRARY_ROOT,
): Library {
  const upstream = openuiLibrary.components;
  const keepSet = new Set(keep);
  const slotted = Object.keys(UPSTREAM_SLOTTED);
  const added = new Set([...Object.keys(LOCAL_COMPONENTS), ...Object.keys(CHAT_REUSED), "SectionBlock", "SectionItem"]);
  for (const name of keep)
    if (!upstream[name] && !added.has(name))
      throw new Error(`[apps/library] "${name}" is neither an upstream openuiLibrary component nor a local one`);
  if (!keepSet.has(root)) throw new Error(`[apps/library] root "${root}" is not in the kept components`);

  const base = createLibrary({
    root,
    id: "theuth-apps",
    componentGroups: curatedGroups(keepSet, Object.keys(upstream)),
    components: Object.values(upstream).filter((c) => !slotted.includes(c.name)),
  });

  const slotsFor = (slots: string[]) => slots.filter((s) => keepSet.has(s.split(".")[0]!));
  const entry = (component: DefinedComponent, slots: string[]) => {
    const at = slotsFor(slots);
    return at.length ? { component, slots: at } : component;
  };
  const diagrams = Object.entries(LOCAL_COMPONENTS).map(([, c]) => entry(c, DIAGRAM_SLOTS));
  const sections = sectionComponents(base).map((c) => entry(c, c.name === "SectionBlock" ? ["Card.children"] : []));
  const reused = Object.entries(CHAT_REUSED).map(([name, slots]) =>
    entry(openuiChatLibrary.components[name] as unknown as DefinedComponent, slots),
  );
  const slottedUp = slotted.map((name) => entry(upstream[name] as unknown as DefinedComponent, UPSTREAM_SLOTTED[name]!));
  const names = (e: DefinedComponent | { component: DefinedComponent; slots: string[] }) => ("slots" in e ? e.component.name : e.name);
  const additions = [...sections, ...reused, ...slottedUp, ...diagrams].filter((e) => keepSet.has(names(e)));

  const remove = Object.keys(upstream).filter((n) => !keepSet.has(n) && !slotted.includes(n));
  return base.extend({
    components: { add: additions, remove },
    functions: { add: [...LIBRARY_FUNCTIONS] },
    actions: { add: [...LIBRARY_ACTIONS] },
  }) as unknown as Library;
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
