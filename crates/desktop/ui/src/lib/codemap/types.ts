/**
 * 73 §5 — the CodeMap contract. Wire shapes come from the engine's scoped OpenRPC
 * schemas; only client-side slice types and supported visualizations live here.
 */
import type { Api } from '@/rpc/bindings/client'
import type { CodeToolSummary } from "@/rpc/bindings/event.codeindex_2eevents.output"
import type { CodeindexEvent } from "@/rpc/bindings/event.codeindex_2eevents.output"

export type { ChangeFile } from "@/rpc/bindings/method.codeindex.output"
export type { ChangeLens } from "@/rpc/bindings/method.codeindex.output"
export type { ChangeSym } from "@/rpc/bindings/method.codeindex.output"
export type { CodeToolSummary } from "@/rpc/bindings/event.codeindex_2eevents.output"
export type { DirRow } from "@/rpc/bindings/method.codeindex.output"
export type { FitEdge } from "@/rpc/bindings/method.codeindex.output"
export type { FitLens } from "@/rpc/bindings/method.codeindex.output"
export type { FitNode } from "@/rpc/bindings/method.codeindex.output"
export type { HereLens } from "@/rpc/bindings/method.codeindex.output"
export type { LensMeta } from "@/rpc/bindings/method.codeindex.output"
export type { MapRow } from "@/rpc/bindings/method.codeindex.output"

const CODE_TOOLS = ['code_map', 'code_find', 'code_outline', 'code_refs', 'code_symbol', 'code_impact', 'code_overview'] as const
export type CodeTool = (typeof CODE_TOOLS)[number]
export const isCodeTool = (tool: string): tool is CodeTool => CODE_TOOLS.some(known => known === tool)

/** `CodeindexEvent::CodeToolRan` — once per `Tool::call`, after the envelope exists (errors too). */
export type CodeToolRan = Extract<CodeindexEvent, { type: 'CodeToolRan' }>

/** `CodeindexEvent::CodeReconciled` — no `session`: global; the UI filters by `root`. */
export type CodeReconciled = Extract<CodeindexEvent, { type: 'CodeReconciled' }>

/** 147 §10.1 `CodeindexEvent::CodeMapProgress` — global (no `session` scope); the UI filters by `root`. */
export type CodeMapProgress = Extract<CodeindexEvent, { type: 'CodeMapProgress' }>

/** 147 §10.1 `CodeindexEvent::CodeMapChanged` — element ids to refetch and the ones that went stale. */
export type CodeMapChanged = Extract<CodeindexEvent, { type: 'CodeMapChanged' }>

/** §5.2 — the panel's subset of the generated API. */
export type CodemapApi = Pick<Api, 'codemap_here' | 'codemap_fit' | 'codemap_change' | 'codemap_touch'>

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
