import type { Spec, UIElement } from '@json-render/core'
import { jsonrepair } from 'jsonrepair'
import type { GraphData } from '@/lib/graph-layout'
import { COMPONENT_NAMES, COMPONENTS, isContainer } from './catalog'

// Mirrors `crates/surface::parse` (design 30 §3.2, §4 R3/R4): strict JSON → jsonrepair → JSON, then the same
// structural checks with the same JSON-Pointer error paths, so the desktop and the TUI reject the same bodies.

/** `crates/surface::MAX_ELEMENTS`. */
export const MAX_ELEMENTS = 256
/** `crates/surface::MAX_DEPTH`. */
export const MAX_DEPTH = 32
/** `crates/surface::MAX_GRAPH_*`. */
export const MAX_GRAPH = { nodes: 120, edges: 240, groups: 12, label: 80 }

type Obj = Record<string, unknown>
type Element = UIElement & { props: Obj; children: string[] }
type Elements = Record<string, Element>

const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)
const isStr = (v: unknown): v is string => typeof v === 'string'

class SpecError extends Error {
  constructor(path: string, message: string) {
    super(`${path}: ${message}`)
  }
}

/** `JSON.parse`, then `jsonrepair` + `JSON.parse`; throws when both fail. */
function parseJson(body: string): { value: unknown; repaired: boolean } {
  try {
    return { value: JSON.parse(body), repaired: false }
  } catch {
    return { value: JSON.parse(jsonrepair(body)), repaired: true }
  }
}

/** Typed validation of one element; the zod prop check uses the catalog schema (unknown props are errors). */
function element(id: string, raw: unknown): Element {
  const at = `/elements/${id}`
  if (!isObj(raw)) throw new SpecError(at, 'expected object')
  for (const key of Object.keys(raw)) {
    if (key !== 'type' && key !== 'props' && key !== 'children') throw new SpecError(`${at}/${key}`, 'unknown key')
  }
  const { type, props = {}, children = [] } = raw
  if (!isStr(type) || !(type in COMPONENTS)) {
    throw new SpecError(`${at}/type`, `unknown component ${JSON.stringify(type)}; catalog: ${COMPONENT_NAMES.join(', ')}`)
  }
  if (!isObj(props)) throw new SpecError(`${at}/props`, 'expected object')
  const issue = COMPONENTS[type]!.safeParse(props).error?.issues[0]
  if (issue) throw new SpecError([`${at}/props`, ...issue.path.map(String)].join('/'), issue.message)
  if (type === 'Graph') graph(props as unknown as GraphData, `${at}/props`)
  if (!Array.isArray(children)) throw new SpecError(`${at}/children`, 'expected array')
  if (children.length && !isContainer(type)) throw new SpecError(`${at}/children`, `${type} cannot have children`)
  children.forEach((c, i) => {
    if (!isStr(c)) throw new SpecError(`${at}/children/${i}`, 'expected string')
  })
  return { type, props, children: children as string[] }
}

/** `crates/surface::validate::graph`, same paths: caps, unique ids, known endpoints and groups, label length. */
function graph(g: GraphData, at: string): void {
  if (g.nodes.length > MAX_GRAPH.nodes) throw new SpecError(`${at}/nodes`, `${g.nodes.length} nodes, limit ${MAX_GRAPH.nodes}`)
  if (g.edges.length > MAX_GRAPH.edges) throw new SpecError(`${at}/edges`, `${g.edges.length} edges, limit ${MAX_GRAPH.edges}`)
  const groups = g.groups ?? []
  if (groups.length > MAX_GRAPH.groups) throw new SpecError(`${at}/groups`, `${groups.length} groups, limit ${MAX_GRAPH.groups}`)
  const label = (path: string, text: string) => {
    const n = [...text].length
    if (n > MAX_GRAPH.label) throw new SpecError(path, `${n} characters, limit ${MAX_GRAPH.label}`)
  }
  const gids: Record<string, true> = {}
  groups.forEach((grp, i) => {
    if (gids[grp.id]) throw new SpecError(`${at}/groups/${i}/id`, `duplicate group id ${JSON.stringify(grp.id)}`)
    gids[grp.id] = true
    label(`${at}/groups/${i}/label`, grp.label)
  })
  const nids: Record<string, true> = {}
  g.nodes.forEach((n, i) => {
    if (nids[n.id]) throw new SpecError(`${at}/nodes/${i}/id`, `duplicate node id ${JSON.stringify(n.id)}`)
    nids[n.id] = true
    if (n.group != null && !gids[n.group]) throw new SpecError(`${at}/nodes/${i}/group`, `unknown group ${JSON.stringify(n.group)}`)
    label(`${at}/nodes/${i}/label`, n.label)
  })
  g.edges.forEach((e, i) => {
    for (const end of ['from', 'to'] as const) {
      if (!nids[e[end]]) throw new SpecError(`${at}/edges/${i}/${end}`, `unknown node ${JSON.stringify(e[end])}`)
    }
    if (e.label != null) label(`${at}/edges/${i}/label`, e.label)
  })
}

/** Tree checks after every element is typed: ids resolve, ≤ 1 parent, root exists, no cycle, limits. */
function tree(root: string, elements: Elements): void {
  const ids = Object.keys(elements)
  if (ids.length > MAX_ELEMENTS) throw new SpecError('/', `${ids.length} elements, limit ${MAX_ELEMENTS}`)
  if (!(root in elements)) throw new SpecError('/root', `${JSON.stringify(root)} is not an element id`)
  const parent: Record<string, string> = {}
  for (const id of ids) {
    elements[id]!.children.forEach((c, i) => {
      if (!(c in elements)) throw new SpecError(`/elements/${id}/children/${i}`, `${JSON.stringify(c)} is not an element id`)
      if (c in parent) throw new SpecError('/', `element ${JSON.stringify(c)} has two parents (${parent[c]}, ${id})`)
      parent[c] = id
    })
  }
  const path: Record<string, true> = {}
  const walk = (id: string, depth: number): void => {
    if (path[id]) throw new SpecError('/', `cycle through ${JSON.stringify(id)}`)
    if (depth > MAX_DEPTH) throw new SpecError('/', `depth ${depth}, limit ${MAX_DEPTH}`)
    path[id] = true
    for (const c of elements[id]!.children) walk(c, depth + 1)
    delete path[id]
  }
  walk(root, 1)
}

/** Whole-body validation; throws `SpecError` at the first failure. */
function validate(value: unknown): Spec {
  if (!isObj(value)) throw new SpecError('/', 'expected object')
  for (const key of Object.keys(value)) {
    if (key !== 'root' && key !== 'elements') throw new SpecError(`/${key}`, 'unknown key')
  }
  const { root, elements: raw } = value
  if (!isStr(root)) throw new SpecError('/root', 'expected string')
  if (!isObj(raw)) throw new SpecError('/elements', 'expected object')
  const elements: Elements = {}
  for (const [id, el] of Object.entries(raw)) elements[id] = element(id, el)
  tree(root, elements)
  return { root, elements }
}

/** `parseSpec`'s outcome: a validated spec (and whether repair was needed) or the first located error. */
export type ParseResult = { spec: Spec; repaired: boolean } | { error: string }

/**
 * A closed fence (R3): strict parse, repair on failure, then full validation.
 * `error` is `"<json-pointer>: <message>"`, the same shape `crates/surface` sends the model.
 */
export function parseSpec(body: string): ParseResult {
  try {
    const { value, repaired } = parseJson(body)
    return { spec: validate(value), repaired }
  } catch (e) {
    if (e instanceof SpecError) return { error: e.message }
    return { error: `: invalid JSON after repair: ${e instanceof Error ? e.message : String(e)}` }
  }
}

/**
 * An open fence while it streams (R4): repair closes the truncated body, then every element that is not yet
 * well-formed (unknown type, half-written props) is dropped and dangling child ids are pruned, so what remains
 * renders progressively. `null` when there is nothing renderable yet. Never throws.
 */
export function parsePartial(body: string): Spec | null {
  let value: unknown
  try {
    value = JSON.parse(jsonrepair(body))
  } catch {
    return null
  }
  if (!isObj(value) || !isStr(value.root) || !isObj(value.elements)) return null
  const elements: Elements = {}
  for (const [id, el] of Object.entries(value.elements)) {
    if (!isObj(el) || !isStr(el.type) || !(el.type in COMPONENTS)) continue
    const props = isObj(el.props) ? el.props : {}
    if (!COMPONENTS[el.type]!.safeParse(props).success) continue
    const children = Array.isArray(el.children) ? el.children.filter(isStr) : []
    elements[id] = { type: el.type, props, children: isContainer(el.type) ? children : [] }
  }
  for (const el of Object.values(elements)) el.children = el.children.filter((c) => c in elements)
  try {
    tree(value.root, elements)
  } catch {
    return null
  }
  return { root: value.root, elements }
}
