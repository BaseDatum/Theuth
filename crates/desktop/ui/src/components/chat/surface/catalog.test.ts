import { expect, test } from 'vitest'
import { z } from 'zod'
import schema from '@/rpc/bindings/surface-catalog.json'
import { COMPONENTS, COMPONENT_NAMES, isContainer } from './catalog'

// Parity with `crates/surface` (design 30 §8): both catalogs are normalized to the same `Shape` tree and deep-compared,
// so a prop added/removed/renamed, an optional flipped, an enum value, or a `Grid.columns`/`Heading.level` bound
// diverging on either side fails here.

type Shape =
  | { kind: 'string' }
  | { kind: 'boolean' }
  | { kind: 'number'; min?: number; max?: number }
  | { kind: 'enum'; values: unknown[] }
  | { kind: 'array'; items: Shape }
  | { kind: 'object'; required: Record<string, Shape>; optional: Record<string, Shape> }

interface Node {
  $ref?: string
  type?: string | string[]
  const?: unknown
  enum?: unknown[]
  oneOf?: Node[]
  anyOf?: Node[]
  properties?: Record<string, Node>
  required?: string[]
  items?: Node
  minimum?: number
  maximum?: number
}

const root = schema as unknown as Node & { $defs?: Record<string, Node>; definitions?: Record<string, Node> }
const defs = root.$defs ?? root.definitions ?? {}

function fromJson(n: Node): Shape {
  if (n.$ref) {
    const name = n.$ref.replace(/^#\/(\$defs|definitions)\//, '')
    const target = defs[name]
    if (!target) throw new Error(`unresolved ${n.$ref}`)
    return fromJson(target)
  }
  const alts = (n.oneOf ?? n.anyOf)?.filter((a) => a.type !== 'null')
  if (alts) {
    if (alts.every((a) => a.const !== undefined)) return { kind: 'enum', values: alts.map((a) => a.const) }
    if (alts.length === 1) return fromJson(alts[0]!)
    throw new Error(`unsupported union: ${JSON.stringify(n)}`)
  }
  if (n.enum) return { kind: 'enum', values: n.enum }
  const type = [n.type ?? []].flat().find((t) => t !== 'null')
  switch (type) {
    case 'string':
      return { kind: 'string' }
    case 'boolean':
      return { kind: 'boolean' }
    case 'integer':
    case 'number':
      return { kind: 'number', min: n.minimum, max: n.maximum }
    case 'array':
      return { kind: 'array', items: fromJson(n.items!) }
    case 'object': {
      const required: Record<string, Shape> = {}
      const optional: Record<string, Shape> = {}
      const req = n.required ?? []
      for (const [k, v] of Object.entries(n.properties ?? {})) (req.includes(k) ? required : optional)[k] = fromJson(v)
      return { kind: 'object', required, optional }
    }
    default:
      throw new Error(`unsupported node: ${JSON.stringify(n)}`)
  }
}

function fromZod(s: z.ZodType): Shape {
  switch (s.def.type) {
    case 'string':
      return { kind: 'string' }
    case 'boolean':
      return { kind: 'boolean' }
    case 'number': {
      // Unbounded zod numbers report ±Infinity (`.int()` reports ±MAX_SAFE_INTEGER); schemars omits the bound.
      const { minValue, maxValue } = s as z.ZodNumber
      const bound = (v: number | null | undefined) => (v != null && Number.isFinite(v) && Math.abs(v) !== Number.MAX_SAFE_INTEGER ? v : undefined)
      return { kind: 'number', min: bound(minValue), max: bound(maxValue) }
    }
    case 'enum':
      return { kind: 'enum', values: (s as z.ZodEnum).options }
    case 'array':
      return { kind: 'array', items: fromZod((s as z.ZodArray<z.ZodType>).element) }
    case 'object': {
      const required: Record<string, Shape> = {}
      const optional: Record<string, Shape> = {}
      for (const [k, v] of Object.entries((s as z.ZodObject<Record<string, z.ZodType>>).shape)) {
        if (v.def.type === 'optional') optional[k] = fromZod((v as z.ZodOptional<z.ZodType>).unwrap())
        else required[k] = fromZod(v)
      }
      return { kind: 'object', required, optional }
    }
    default:
      throw new Error(`unsupported zod type: ${s.def.type}`)
  }
}

/** Rust `Component` variants, from the `Element` oneOf/anyOf: name → props shape. */
function rustComponents(): Record<string, Shape> {
  const element = defs.Element
  const variants = element?.oneOf ?? element?.anyOf
  if (!variants) throw new Error('Element union missing from surface-catalog.json')
  const out: Record<string, Shape> = {}
  for (const v of variants) {
    const tag = v.properties?.type
    const name = (tag?.const ?? tag?.enum?.[0]) as string | undefined
    const props = v.properties?.props
    if (!name || !props) throw new Error(`malformed variant: ${JSON.stringify(v)}`)
    out[name] = fromJson(props)
  }
  return out
}

const rust = rustComponents()

test('same component names as the Rust catalog', () => {
  expect([...COMPONENT_NAMES].sort()).toEqual(Object.keys(rust).sort())
  expect(COMPONENT_NAMES).toHaveLength(60)
})

test.each(Object.keys(rust))('%s props match the Rust catalog', (name) => {
  const zodSide = COMPONENTS[name]
  expect(zodSide, `${name} missing from catalog.ts`).toBeDefined()
  expect(fromZod(zodSide!)).toEqual(rust[name])
})

test('containers are exactly Card, Stack, Grid, Tabs, Accordion', () => {
  expect(COMPONENT_NAMES.filter(isContainer).sort()).toEqual(['Accordion', 'Card', 'Grid', 'Stack', 'Tabs'])
})
