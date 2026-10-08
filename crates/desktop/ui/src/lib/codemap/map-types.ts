/**
 * 147 §4.5 / §10 — a local mirror of the map contract (`crates/codeindex/src/map/mod.rs`, ts-rs under `ts`) so the Map tab
 * compiles before the contract wave's bindings land. Replaced by `@/rpc/bindings/Map*` imports when C is merged; nothing
 * here is exported from `types.ts` beyond that point.
 */
import type { GraphProps } from '@/rpc/bindings/GraphProps'
import type { MapRow } from '@/rpc/bindings/MapRow'

export type MapState = 'None' | 'Structure' | 'Generating' | 'Ready' | 'Partial'
export type MapOrigin = 'Structure' | 'Generated' | 'Imported'

export type MapJob = {
  id: string
  session: string
  started_ms: number
  stage: string
  done: number
  total: number
  calls: number
  cost_nanos: number
  currency: string
  error: string | null
}

export type MapMeta = {
  root: string
  seq: number
  schema: number
  state: MapState
  index_state: string
  git_head: string | null
  stale_elements: number
  job: MapJob | null
  ms: number
}

export type MapTreeRow = {
  id: string
  node: string
  parent: string | null
  depth: number
  name: string
  path: string | null
  kind: string
  children: number
  leaf: boolean
  stale: boolean
  origin: MapOrigin
  marks: string[]
}

export type MapPerspective = {
  id: string
  kind: string
  title: string
  origin: MapOrigin
  elements: number
  stale: number
  tree: MapTreeRow[]
}

export type MapIndex = {
  meta: MapMeta
  perspectives: MapPerspective[]
  profile: unknown
}

export type MapEvidence =
  | { Manifest: { from: string; to: string; kind: string } }
  | { Imports: { count: number; sample: string } }
  | { Refs: { count: number; sample: string; confidence: number } }
  | { Call: { site: string; from: string; to: string } }
  | { Bindings: { generator: string; output_dir: string } }
  | { Process: { site: string; verb: string } }
  | { Inferred: { reason: string } }

export type MapEdge = { from: string; to: string; label: string; backed: boolean; evidence: MapEvidence[] }

export type MapChild = {
  id: string
  node: string
  name: string
  path: string | null
  kind: string
  files: number
  lines: number
  share: number
  leaf: boolean
  stale: boolean
  thumbnail: GraphProps | null
}

export type MapAnchor = { path: string; line_start: number | null; line_end: number | null; exists: boolean }

export type MapFields = {
  description: string
  context: string | null
  constraint: string | null
  concern: string | null
  todo: string | null
  note: string | null
}

export type MapProvenance = {
  model: string
  git_head: string | null
  index_state: string
  generated_ms: number
  call_ids: string[]
  cost_nanos: number
  currency: string
  input_tokens: number
  output_tokens: number
  repaired: boolean
  facts_hash: string
}

export type MapListing = { files: [string, number, number][]; symbols: MapRow[]; more: number }

export type MapElement = {
  meta: MapMeta
  id: string
  node: string
  perspective: string
  parent: string | null
  crumbs: [string, string][]
  name: string
  path: string | null
  kind: string
  members: string[]
  anchors: MapAnchor[]
  fields: MapFields
  diagram: GraphProps | null
  mermaid: string | null
  edges: MapEdge[]
  children: MapChild[]
  also_in: string[]
  stale: boolean
  origin: MapOrigin
  provenance: MapProvenance | null
  listing: MapListing | null
}

export type MapHit = { element: string; name: string; path: string | null; chain: string[] }
export type MapExport = { dir: string; elements: number; files: number; skipped: string[] }

/** §10.1 — global (no `session`); the UI filters by `root`. */
export type CodeMapProgress = {
  type: 'CodeMapProgress'
  root: string
  job: string
  session: string
  stage: string
  done: number
  total: number
  element: string | null
  calls: number
  cost_nanos: number
  currency: string
  error: string | null
  seq: number
}
export type CodeMapChanged = { type: 'CodeMapChanged'; root: string; elements: string[]; stale: string[]; more: number; seq: number }
