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
/** `Graph` props (65 §3.1); the id/endpoint/cap rules live in `parse.ts` like the Rust `validate.rs` ones. */
export const graphProps = z.strictObject({
  nodes: z.array(
    z.strictObject({
      id: str,
      label: graphLabel,
      kind: z.enum(['box', 'rounded', 'pill', 'diamond', 'cylinder', 'actor']).optional(),
      group: str.optional(),
      state: z.enum(['default', 'active', 'done', 'muted', 'danger']).optional(),
    }),
  ),
  edges: z.array(z.strictObject({ from: str, to: str, label: graphLabel.optional(), style: z.enum(['solid', 'dashed']).optional() })),
  groups: z.array(z.strictObject({ id: str, label: graphLabel })).optional(),
  direction: z.enum(['LR', 'TB']).optional(),
})

/** The 36 v0 components with their zod prop schemas; containers declare `slots: ['default']`. */
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
          }),
        ),
      }),
    },
    Checklist: { props: z.strictObject({ items: z.array(z.strictObject({ text: str, done: z.boolean() })) }) },
    Tabs: { props: z.strictObject({ labels: z.array(str) }), slots: ['default'] },
    Accordion: { props: z.strictObject({ titles: z.array(str) }), slots: ['default'] },
    Graph: { props: graphProps },
    Steps: { props: z.strictObject({ items: z.array(z.strictObject({ title: str, detail: str.optional() })) }) },
    Callout: {
      props: z.strictObject({ text: str, title: str.optional(), variant: z.enum(['note', 'tip', 'question', 'important']).optional() }),
    },
    Code: { props: z.strictObject({ text: str, lang: str.optional(), title: str.optional() }) },
    Diff: { props: z.strictObject({ before: str, after: str, from: str.optional(), to: str.optional() }) },
    Tree: {
      props: z.strictObject({
        items: z.array(z.strictObject({ label: str, depth: z.number().int().min(0).max(6).optional(), detail: str.optional() })),
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
      props: z.strictObject({ rows: z.array(str), cols: z.array(str), cells: z.array(z.array(z.number())), max: z.number().optional() }),
    },
    Person: { props: z.strictObject({ name: str, role: str.optional(), contact: str.optional() }) },
    FileList: { props: z.strictObject({ items: z.array(z.strictObject({ path: str, kind: str.optional(), size: str.optional() })) }) },
    Kanban: {
      props: z.strictObject({
        columns: z.array(z.strictObject({ title: str, cards: z.array(z.strictObject({ title: str, detail: str.optional(), tag: str.optional() })) })),
      }),
    },
    Calendar: {
      props: z.strictObject({ days: z.array(z.strictObject({ day: str, label: str.optional(), state: dayState })), title: str.optional() }),
    },
  },
  actions: {},
})

/** Component name → its zod prop schema. */
export const COMPONENTS: Record<string, z.ZodType> = Object.fromEntries(
  Object.entries(catalog.data.components).map(([name, def]) => [name, def.props]),
)

/** The 36 component names, catalog order. */
export const COMPONENT_NAMES = Object.keys(COMPONENTS)

/** True for the five containers (the ones declaring a default slot); only they may have `children`. */
export function isContainer(name: string): boolean {
  return !!(catalog.data.components as Record<string, { slots?: string[] }>)[name]?.slots?.length
}
