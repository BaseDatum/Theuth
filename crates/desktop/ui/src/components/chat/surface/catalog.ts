import { defineCatalog } from '@json-render/core'
import { schema } from '@json-render/react/schema'
import { z } from 'zod'

// Catalog v0 (design 30 §10). Props mirror `crates/surface`'s `Component` enum exactly: optional = `.optional()`,
// enums lowercase, `strictObject` so an unknown prop is an error like serde's `deny_unknown_fields`.
const str = z.string()
const metric = z.strictObject({
  label: str,
  value: str,
  trend: z.enum(['up', 'down', 'flat']).optional(),
  delta: str.optional(),
  spark: z.array(z.number()).optional(),
})
const badgeVariant = z.enum(['default', 'secondary', 'success', 'warning', 'destructive']).optional()
const dayState = z.enum(['done', 'active', 'pending']).optional()
const graphLabel = str
/** A `ref` (71 §2.3) is a plain string here; the grammar is checked in `parse.ts` like the Rust `validate.rs`. */
const ref = str.optional()
/** `GraphState`: the state vocabulary of every diagram unit (123 §4). */
export const unitState = z.enum(['default', 'active', 'done', 'muted', 'danger'])
const state = unitState.optional()
const num = z.number()
const uint = z.number().int().min(0)
const title = str.optional()
const lineStyle = z.enum(['solid', 'dashed']).optional()
const direction = z.enum(['LR', 'TB']).optional()
/** `GraphIcon` (123 §4.1): a closed set drawn from lucide. */
export const graphIcon = z.enum([
  'server', 'database', 'cloud', 'user', 'users', 'globe', 'lock', 'shield', 'key', 'queue', 'cpu', 'box', 'package',
  'file', 'folder', 'terminal', 'laptop', 'phone', 'mail', 'clock', 'settings', 'code', 'git-branch', 'bot', 'browser',
])
const graphNode = z.strictObject({
  id: str,
  label: graphLabel,
  kind: z.enum(['box', 'rounded', 'pill', 'diamond', 'cylinder', 'actor', 'hexagon', 'circle', 'note']).optional(),
  group: str.optional(),
  state,
  ref,
  sub: str.optional(),
  tag: str.optional(),
  icon: graphIcon.optional(),
})
const graphEdge = z.strictObject({ from: str, to: str, label: graphLabel.optional(), style: lineStyle, state, arrow: z.enum(['forward', 'both', 'none']).optional() })
const graphGroup = z.strictObject({ id: str, label: graphLabel, parent: str.optional(), kind: z.enum(['zone', 'boundary', 'cluster']).optional() })
/** `Graph` props (65 §3.1, 71 §2.4, 123 §4.1); the id/endpoint/cap rules live in `parse.ts` like the Rust `validate.rs` ones. */
export const graphProps = z.strictObject({
  nodes: z.array(graphNode),
  edges: z.array(graphEdge),
  groups: z.array(graphGroup).optional(),
  direction,
  legend: z.array(z.strictObject({ label: graphLabel, state, style: lineStyle, kind: graphNode.shape.kind })).optional(),
  title,
})
/** One side of a `GraphDelta` (123 §4.2): a `Graph` without direction. */
export const graphSide = z.strictObject({ nodes: z.array(graphNode), edges: z.array(graphEdge), groups: z.array(graphGroup).optional() })
/** `Sequence` props (71 §2.4, 123 §4.1): actors, steps between them, optional title, fragments and notes. */
export const sequenceProps = z.strictObject({
  actors: z.array(z.strictObject({ id: str, label: str })),
  steps: z.array(z.strictObject({ from: str, to: str, label: str, style: z.enum(['call', 'return', 'async']).optional(), ref, note: str.optional(), state })),
  title,
  fragments: z.array(z.strictObject({ kind: z.enum(['alt', 'opt', 'loop', 'par']), from: uint, to: uint, label: str.optional(), else: uint.optional() })).optional(),
})
/** `Series` (123 §4.2): one chart series. */
const series = z.strictObject({ name: str, values: z.array(num), state })
const seriesChart = { labels: z.array(str), series: z.array(series), xLabel: str.optional(), yLabel: str.optional(), unit: str.optional(), title }
// ---- 123 ELK family (W1) ----
export const stateMachineProps = z.strictObject({
  states: z.array(z.strictObject({ id: str, label: str, kind: z.enum(['initial', 'final', 'state', 'choice']).optional(), sub: str.optional(), state, ref })),
  transitions: z.array(z.strictObject({ from: str, to: str, label: str.optional(), guard: str.optional(), style: lineStyle, state })),
  direction,
  title,
})
export const entitiesProps = z.strictObject({
  entities: z.array(
    z.strictObject({
      id: str,
      label: str,
      kind: z.enum(['entity', 'table', 'class', 'interface', 'enum']).optional(),
      fields: z.array(z.strictObject({ name: str, type: str.optional(), key: z.enum(['pk', 'fk', 'uk']).optional(), nullable: z.boolean().optional(), note: str.optional() })),
      methods: z.array(z.strictObject({ name: str, sig: str.optional() })).optional(),
      state,
      ref,
    }),
  ),
  relations: z.array(
    z.strictObject({
      from: str,
      to: str,
      kind: z.enum(['one-one', 'one-many', 'many-many', 'inherits', 'implements', 'composes', 'aggregates', 'depends', 'references']),
      label: str.optional(),
      from_field: str.optional(),
      to_field: str.optional(),
    }),
  ),
  detail: z.enum(['model', 'schema', 'class']).optional(),
  title,
})
export const swimlaneProps = z.strictObject({
  lanes: z.array(z.strictObject({ id: str, label: str })),
  steps: z.array(z.strictObject({ id: str, lane: str, label: str, kind: z.enum(['step', 'decision', 'start', 'end', 'data']).optional(), sub: str.optional(), state, ref })),
  edges: z.array(z.strictObject({ from: str, to: str, label: str.optional(), style: lineStyle })),
  direction,
  title,
})
export const graphDeltaProps = z.strictObject({ before: graphSide, after: graphSide, direction, title })
// ---- 123 chart family (W2) ----
export const barChartProps = z.strictObject({ ...seriesChart, variant: z.enum(['grouped', 'stacked', 'dumbbell', 'horizontal']).optional() })
export const lineChartProps = z.strictObject({ ...seriesChart, variant: z.enum(['line', 'area', 'slope', 'bump', 'step']).optional() })
export const scatterChartProps = z.strictObject({
  points: z.array(z.strictObject({ x: num, y: num, label: str.optional(), size: num.optional(), group: str.optional(), state, ref })),
  xLabel: str,
  yLabel: str,
  groups: z.array(z.strictObject({ id: str, label: str })).optional(),
  variant: z.enum(['scatter', 'bubble', 'beeswarm']).optional(),
  title,
})
export const radarChartProps = z.strictObject({ axes: z.array(str), series: z.array(series), max: num.optional(), variant: z.enum(['radar', 'polar']).optional(), title })
export const waterfallProps = z.strictObject({
  steps: z.array(z.strictObject({ label: str, value: num, kind: z.enum(['delta', 'total', 'subtotal']).optional(), state })),
  unit: str.optional(),
  title,
})
export const funnelProps = z.strictObject({
  stages: z.array(z.strictObject({ label: str, value: num.optional(), note: str.optional(), state })),
  variant: z.enum(['funnel', 'pyramid']).optional(),
  unit: str.optional(),
  title,
})
export const ganttProps = z.strictObject({
  tasks: z.array(
    z.strictObject({ id: str, label: str, start: str, end: str, group: str.optional(), state, progress: num.min(0).max(1).optional(), depends: z.array(str).optional(), ref }),
  ),
  today: str.optional(),
  title,
})
export const quadrantProps = z.strictObject({
  xLabel: str,
  yLabel: str,
  items: z.array(z.strictObject({ id: str.optional(), label: str, x: num.min(0).max(1), y: num.min(0).max(1), size: num.optional(), state, ref })),
  quadrants: z.array(str).optional(),
  xTicks: z.array(str).optional(),
  yTicks: z.array(str).optional(),
  title,
})
// ---- 123 part-of-whole + misc (W3) ----
export const treemapProps = z.strictObject({ items: z.array(z.strictObject({ label: str, value: num, group: str.optional(), state, ref })), title })
export const sankeyProps = z.strictObject({
  nodes: z.array(z.strictObject({ id: str, label: str, stage: uint.optional(), state })),
  flows: z.array(z.strictObject({ from: str, to: str, value: num, label: str.optional(), state })),
  unit: str.optional(),
  title,
})
export const vennProps = z.strictObject({
  sets: z.array(z.strictObject({ id: str, label: str, size: num.optional() })),
  overlaps: z.array(z.strictObject({ sets: z.array(str), label: str })).optional(),
  title,
})
export const loopProps = z.strictObject({ steps: z.array(z.strictObject({ label: str, note: str.optional(), state, ref })), hub: str.optional(), title })
export const layersProps = z.strictObject({
  layers: z.array(z.strictObject({ label: str, items: z.array(str).optional(), note: str.optional(), state, ref })),
  direction: z.enum(['top-down', 'bottom-up']).optional(),
  title,
})
export const journeyProps = z.strictObject({
  stages: z.array(str),
  rows: z.array(z.strictObject({ label: str, cells: z.array(str) })),
  mood: z.array(num).optional(),
  pains: z.array(z.strictObject({ stage: uint, text: str })).optional(),
  title,
})
export const fishboneProps = z.strictObject({ effect: str, causes: z.array(z.strictObject({ category: str, items: z.array(str) })), title })
/** `Excerpt` props (71 §2.5): quoted lines pinned to `path` from `start`; `mark` pairs are line numbers, `symbol` names the definition. */
export const excerptProps = z.strictObject({
  path: str,
  text: str,
  start: z.number().int().min(1).optional(),
  lang: str.optional(),
  caption: str.optional(),
  mark: z.array(z.array(z.number().int().min(0)).length(2)).optional(),
  symbol: str.optional(),
})

/** The 60 components (v0 + 123's 19 + 229's 3 Deck interactive) with their zod prop schemas; containers declare `slots: ['default']`. */
export const catalog = defineCatalog(schema, {
  components: {
    Card: { props: z.strictObject({ title: str.optional(), description: str.optional() }), slots: ['default'] },
    Stack: {
      props: z.strictObject({
        direction: z.enum(['vertical', 'horizontal']).optional(),
        gap: z.enum(['none', 'sm', 'md', 'lg']).optional(),
      }),
      slots: ['default'],
    },
    Grid: { props: z.strictObject({ columns: z.number().int().min(1).max(6) }), slots: ['default'] },
    Separator: { props: z.strictObject({}) },
    Heading: { props: z.strictObject({ text: str, level: z.number().int().min(1).max(4).optional() }) },
    Text: { props: z.strictObject({ text: str, variant: z.enum(['body', 'muted', 'caption', 'lead', 'code']).optional() }) },
    Badge: { props: z.strictObject({ text: str, variant: badgeVariant }) },
    Alert: {
      props: z.strictObject({
        title: str.optional(),
        message: str,
        type: z.enum(['info', 'success', 'warning', 'error']).optional(),
      }),
    },
    Table: { props: z.strictObject({ columns: z.array(str), rows: z.array(z.array(str)) }) },
    KeyValueList: { props: z.strictObject({ items: z.array(z.strictObject({ key: str, value: str })) }) },
    Metric: { props: metric },
    MetricGrid: { props: z.strictObject({ items: z.array(metric) }) },
    Progress: { props: z.strictObject({ value: z.number(), max: z.number().optional(), label: str.optional() }) },
    Bars: {
      props: z.strictObject({ items: z.array(z.strictObject({ label: str, value: z.number() })), max: z.number().optional() }),
    },
    Timeline: {
      props: z.strictObject({
        items: z.array(
          z.strictObject({
            when: str,
            title: str,
            detail: str.optional(),
            state: dayState,
            until: str.optional(),
            id: str.optional(),
            ref,
          }),
        ),
        scale: z.enum(['list', 'time']).optional(),
        title,
      }),
    },
    Checklist: { props: z.strictObject({ items: z.array(z.strictObject({ text: str, done: z.boolean() })) }) },
    Tabs: { props: z.strictObject({ labels: z.array(str) }), slots: ['default'] },
    Accordion: { props: z.strictObject({ titles: z.array(str) }), slots: ['default'] },
    Graph: { props: graphProps },
    Sequence: { props: sequenceProps },
    Steps: { props: z.strictObject({ items: z.array(z.strictObject({ title: str, detail: str.optional() })) }) },
    Callout: {
      props: z.strictObject({ text: str, title: str.optional(), variant: z.enum(['note', 'tip', 'question', 'important']).optional() }),
    },
    Code: { props: z.strictObject({ text: str, lang: str.optional(), title: str.optional() }) },
    Diff: { props: z.strictObject({ before: str, after: str, from: str.optional(), to: str.optional() }) },
    Excerpt: { props: excerptProps },
    Tree: {
      props: z.strictObject({
        items: z.array(z.strictObject({ label: str, depth: z.number().int().min(0).max(6).optional(), detail: str.optional(), id: str.optional(), state, ref })),
        layout: z.enum(['list', 'nodes', 'nested', 'org']).optional(),
        title,
      }),
    },
    Glossary: { props: z.strictObject({ items: z.array(z.strictObject({ term: str, text: str })) }) },
    Quote: { props: z.strictObject({ text: str, source: str.optional() }) },
    Image: { props: z.strictObject({ src: str, alt: str, caption: str.optional() }) },
    LinkList: { props: z.strictObject({ items: z.array(z.strictObject({ title: str, url: str, note: str.optional() })) }) },
    Tags: { props: z.strictObject({ items: z.array(z.strictObject({ text: str, variant: badgeVariant })) }) },
    Gauge: { props: z.strictObject({ value: z.number(), label: str.optional(), max: z.number().optional(), min: z.number().optional() }) },
    Sparkline: { props: z.strictObject({ values: z.array(z.number()), label: str.optional() }) },
    Heatmap: {
      props: z.strictObject({
        rows: z.array(str),
        cols: z.array(str),
        cells: z.array(z.array(z.number())),
        max: z.number().optional(),
        palette: z.enum(['sequential', 'diverging']).optional(),
        title,
      }),
    },
    Person: { props: z.strictObject({ name: str, role: str.optional(), contact: str.optional() }) },
    FileList: { props: z.strictObject({ items: z.array(z.strictObject({ path: str, kind: str.optional(), size: str.optional() })) }) },
    Kanban: {
      props: z.strictObject({
        columns: z.array(
          z.strictObject({
            title: str,
            cards: z.array(
              z.strictObject({ title: str, detail: str.optional(), tag: str.optional(), state: z.enum(['default', 'blocked', 'done']).optional(), id: str.optional(), ref, row: uint.optional() }),
            ),
            limit: uint.optional(),
          }),
        ),
        rows: z.array(str).optional(),
        cut: uint.optional(),
        title,
      }),
    },
    Calendar: {
      props: z.strictObject({ days: z.array(z.strictObject({ day: str, label: str.optional(), state: dayState })), title: str.optional() }),
    },
    // ---- 123 ELK family (W1) ----
    StateMachine: { props: stateMachineProps },
    Entities: { props: entitiesProps },
    Swimlane: { props: swimlaneProps },
    GraphDelta: { props: graphDeltaProps },
    // ---- 123 chart family (W2) ----
    BarChart: { props: barChartProps },
    LineChart: { props: lineChartProps },
    ScatterChart: { props: scatterChartProps },
    RadarChart: { props: radarChartProps },
    Waterfall: { props: waterfallProps },
    Funnel: { props: funnelProps },
    Gantt: { props: ganttProps },
    Quadrant: { props: quadrantProps },
    // ---- 123 part-of-whole + misc (W3) ----
    Treemap: { props: treemapProps },
    Sankey: { props: sankeyProps },
    Venn: { props: vennProps },
    Loop: { props: loopProps },
    Layers: { props: layersProps },
    Journey: { props: journeyProps },
    Fishbone: { props: fishboneProps },
    // ---- 229 Decks interactive components (inert outside Deck context) ----
    ActionButton: {
      props: z.strictObject({
        label: str,
        action: str,
        args_json: str.optional(),
        tone: z.enum(['default', 'primary', 'danger']).optional(),
        disabled: z.boolean().optional(),
        confirm: str.optional(),
      }),
    },
    StreamControls: {
      props: z.strictObject({
        title: str.optional(),
        show_run_once: z.boolean().optional(),
      }),
    },
    ResultBanner: {
      props: z.strictObject({
        title: str.optional(),
        empty_text: str.optional(),
        action_filter: str.optional(),
      }),
    },
  },
  actions: {},
})

/** Component name → its zod prop schema. */
export const COMPONENTS: Record<string, z.ZodType> = Object.fromEntries(
  Object.entries(catalog.data.components).map(([name, def]) => [name, def.props]),
)

/** The 60 component names, catalog order. */
export const COMPONENT_NAMES = Object.keys(COMPONENTS)

/** True for the five containers (the ones declaring a default slot); only they may have `children`. */
export function isContainer(name: string): boolean {
  return !!(catalog.data.components as Record<string, { slots?: string[] }>)[name]?.slots?.length
}
