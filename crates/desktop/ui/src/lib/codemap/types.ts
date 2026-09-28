/**
 * 73 §5 — the CodeMap contract. The wire shapes come from the engine's `ts_rs` bindings
 * (`crates/codeindex/src/{event,lens}.rs`); only the client-side slice types live here.
 */
import type { ChangeLens } from '@/rpc/bindings/ChangeLens'
import type { CodeToolSummary } from '@/rpc/bindings/CodeToolSummary'
import type { CodeindexEvent } from '@/rpc/bindings/CodeindexEvent'
import type { FitLens } from '@/rpc/bindings/FitLens'
import type { HereLens } from '@/rpc/bindings/HereLens'
import type { LensMeta } from '@/rpc/bindings/LensMeta'

export type { ChangeFile } from '@/rpc/bindings/ChangeFile'
export type { ChangeLens } from '@/rpc/bindings/ChangeLens'
export type { ChangeSym } from '@/rpc/bindings/ChangeSym'
export type { CodeToolSummary } from '@/rpc/bindings/CodeToolSummary'
export type { DirRow } from '@/rpc/bindings/DirRow'
export type { FitEdge } from '@/rpc/bindings/FitEdge'
export type { FitLens } from '@/rpc/bindings/FitLens'
export type { FitNode } from '@/rpc/bindings/FitNode'
export type { HereLens } from '@/rpc/bindings/HereLens'
export type { LensMeta } from '@/rpc/bindings/LensMeta'
export type { MapRow } from '@/rpc/bindings/MapRow'

export type CodeTool = 'code_map' | 'code_find' | 'code_outline' | 'code_refs' | 'code_symbol' | 'code_impact'

/** `CodeindexEvent::CodeToolRan` — once per `Tool::call`, after the envelope exists (errors too). */
export type CodeToolRan = Extract<CodeindexEvent, { type: 'CodeToolRan' }>

/** `CodeindexEvent::CodeReconciled` — no `session`: global; the UI filters by `root`. */
export type CodeReconciled = Extract<CodeindexEvent, { type: 'CodeReconciled' }>

/** §5.2 — the four lens methods, positional params, flat names (a structural subset of `Api`). */
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
