/**
 * 73 §5 — the CodeMap contract, hand-typed until the engine's `ts_rs` bindings land (`CodeindexEvent.ts`, `lens.rs`).
 * Field names and shapes are the artifact contract with the engine worker: nothing here that §5 does not spell.
 */
import type { SessionId } from '@/rpc/bindings/SessionId'

/** `CodeToolRan.summary` (§5.1): one compact envelope per `code_*` call; rows never ride events. */
export type CodeToolSummary = {
  rows: number
  dropped: number
  is_error: boolean
  stale: boolean
  pending: number
  truncated_walk: boolean
  suspect_root: boolean
  hint: string | null
  /** `code_map` only: `focus_files ∪ focus_idents` as given. */
  focus?: string[]
  /** `code_outline` only. */
  path?: string
  /** `code_refs` / `code_symbol` when exactly one definition matched; `null` when `candidates`. */
  definition?: {
    moniker: string
    path: string
    line_start: number
    line_end: number
    name: string
    kind: string
  } | null
  /** `code_impact` only. */
  impact?: {
    mode: string
    range?: { old: string; new: string } | null
    include_tests: boolean
    changed: number
    direct: number
    transitive: number
    tests: number
    ambiguous_skipped: number
    unresolved_imports: number
  }
}

export type CodeTool = 'code_map' | 'code_find' | 'code_outline' | 'code_refs' | 'code_symbol' | 'code_impact'

/** `CodeindexEvent::CodeToolRan` — once per `Tool::call`, after the envelope exists (errors too). */
export type CodeToolRan = {
  type: 'CodeToolRan'
  session: SessionId
  /** Canonical, as `Envelope.root`. */
  root: string
  call_id: string
  tool: CodeTool
  /** The tool's args as received, minus `budget` and `root`. */
  args: Record<string, unknown>
  summary: CodeToolSummary
  seq: number
}

/** `CodeindexEvent::CodeReconciled` — no `session`: global; the UI filters by `root`. */
export type CodeReconciled = {
  type: 'CodeReconciled'
  root: string
  reparsed: string[]
  removed: string[]
  more?: number
  stale: boolean
  pending: number
  suspect_root: boolean
  hint: string | null
  git_head: string | null
  seq: number
}

/** §5.3 `LensMeta` — on every lens reply. */
export type LensMeta = {
  root: string
  seq: number
  indexed_files: number
  stale: boolean
  pending: number
  suspect_root: boolean
  hint: string | null
  truncated_walk: boolean
  git_head: string | null
  /** Engine time for this reply, ms. */
  ms: number
}

export type MapRow = {
  moniker: string
  path: string
  line: number
  line_end: number
  kind: string
  name: string
  /** ≤ 200 chars. */
  signature: string
  rank: number
  exported: boolean
  importers: number
  callers: number
  recent: boolean
  entry: boolean
  is_test: boolean
}

/** Coverage fallback: directories with no row in `rows`. */
export type DirRow = { path: string; files: number; ranked_rows: number; importers: number }

export type HereLens = {
  meta: LensMeta
  /** File order by best rank, symbols by line. */
  rows: MapRow[]
  dropped: number
  dirs: DirRow[]
  personalised: boolean
  cache_hit: boolean
}

export type FitNode = {
  moniker: string
  path: string
  line_start: number
  line_end: number
  kind: string
  name: string
  signature: string
  is_test: boolean
}

export type FitEdge = {
  /** Monikers. */
  from: string
  to: string
  count: number
  /** `call | type | …` */
  kind: string
  /** `same_file | import | package | unique` */
  provenance: string
  confidence: number
}

export type FitLens = {
  meta: LensMeta
  focus: FitNode
  callers: FitNode[]
  callees: FitNode[]
  edges: FitEdge[]
  /** Refs named like the focus with `to_symbol` NULL. */
  ambiguous: number
  more_callers: number
  more_callees: number
  /** Non-empty when `symbol` matched several; `focus` = the best-ranked. */
  candidates: FitNode[]
}

export type ChangeSym = { moniker: string; name: string; kind: string; line: number }

export type ChangeFile = {
  path: string
  /** `added | modified | deleted`; changed band only. */
  status: string | null
  /** ≤ 12. */
  symbols: ChangeSym[]
  /** Direct band: names or `"<import>"`. */
  via: string[]
  is_test: boolean
  /** `"ref" | "subject"` (impact sibling). */
  test_link: string | null
  depth: number
}

export type ChangeLens = {
  meta: LensMeta
  /** `worktree | range | files` */
  mode: string
  range: [string, string] | null
  changed: ChangeFile[]
  direct: ChangeFile[]
  transitive: ChangeFile[]
  tests: ChangeFile[]
  /** Dropped per band: changed, direct, transitive, tests. */
  more: [number, number, number, number]
  ambiguous_skipped: number
  unresolved_imports: number
  unindexed: string[]
  cache_hit: boolean
}

/** §5.2 — the four lens methods, positional params, flat names. */
export interface CodemapApi {
  codemap_here(root: string, focus: string[] | null, edited: string[] | null, limit: number | null): Promise<HereLens>
  codemap_fit(root: string, symbol: string, limit: number | null): Promise<FitLens>
  codemap_change(root: string, git_range: string | null, files: string[] | null, include_tests: boolean | null): Promise<ChangeLens>
  codemap_touch(root: string, paths: string[] | null): Promise<LensMeta>
}

/** §4 — what the session slice keeps per engine (`SessionSlice.codemap`). */
export type CodemapSlice = {
  /** Touched this session, insertion order, ≤ 8 (oldest dropped). `"?"` = known from history, root unresolved. */
  roots: string[]
  last: { root: string; tool: CodeTool; callId: string; args: unknown; summary: CodeToolSummary; seq: number } | null
  /** ≤ 20 newest first. `target` is what the call was about — the `►` marker sits on it; `args`/`summary` let a row replay its lens (§6.1). */
  trail: TrailRow[]
  /** Root-relative paths from `edit` / `write_file` this session (≤ 200, oldest dropped), by root. */
  edited: Record<string, string[]>
}

export type TrailRow = {
  root: string
  tool: CodeTool
  callId: string
  target: string | null
  seq: number
  at: number
  args: Record<string, unknown>
  summary: CodeToolSummary
}
