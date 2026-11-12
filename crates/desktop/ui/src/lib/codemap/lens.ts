/** 73 §6.1 — the lens state machine, pure. */
import type { CodeTool, CodeToolRan, CodeToolSummary } from './types'
import { isCodeTool } from './types'

export type Lens = 'here' | 'fit' | 'change'
export type LensParams = {
  here: { focus: string[] | null; edited: string[] | null }
  fit: { symbol: string } | null
  change: { git_range: string | null; files: string[] | null; include_tests: boolean }
}
export type LensState = {
  mode: 'auto' | 'pinned'
  /** The lens shown. */
  lens: Lens
  /** What auto would show now; pinned && auto !== lens → the auto tab gets a dot. */
  auto: Lens
  params: LensParams
  /** Fit/Here highlight: the definition moniker (code_refs/code_symbol) or null. */
  focus: string | null
  /** call_id that produced the shown lens (trail row underline); null before any. */
  callId: string | null
}

export function initialLens(): LensState {
  return {
    mode: 'auto',
    lens: 'here',
    auto: 'here',
    params: { here: { focus: null, edited: null }, fit: null, change: { git_range: null, files: null, include_tests: true } },
    focus: null,
    callId: null,
  }
}

const strs = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : typeof v === 'string' ? [v] : [])

/** What a call would show, or null when it must not switch (73 §6.1: `is_error` never switches). */
export function lensFor(tool: CodeTool, args: Record<string, unknown>, summary: CodeToolSummary): { lens: Lens; params: Partial<LensParams>; focus: string | null } | null {
  if (summary.is_error) return null
  switch (tool) {
    case 'code_map':
      return { lens: 'here', params: { here: { focus: [...strs(args.focus_files), ...strs(args.focus_idents), ...strs(args.focus)], edited: null } }, focus: null }
    case 'code_find':
      return { lens: 'here', params: { here: { focus: strs(args.name ?? args.query), edited: null } }, focus: null }
    case 'code_outline':
      return { lens: 'here', params: { here: { focus: strs(summary.path ?? args.path), edited: null } }, focus: null }
    case 'code_refs':
    case 'code_symbol': {
      const moniker = summary.definition?.moniker ?? null
      return { lens: 'fit', params: { fit: { symbol: moniker ?? String(args.symbol ?? args.name) } }, focus: moniker }
    }
    case 'code_impact':
      return {
        lens: 'change',
        params: {
          change: {
            git_range: typeof args.git_range === 'string' ? args.git_range : null,
            files: Array.isArray(args.files) ? strs(args.files) : null,
            include_tests: args.include_tests !== false,
          },
        },
        focus: null,
      }
    // 147 §12.1: the map tool counts as a `code_*` call but never switches a lens.
    case 'code_overview':
      return null
  }
}

export function onCodeToolRan(s: LensState, e: CodeToolRan): LensState {
  if (!isCodeTool(e.tool)) return s
  const r = lensFor(e.tool, e.args, e.summary)
  if (!r) return s
  const params = { ...s.params, ...r.params }
  if (s.mode === 'pinned') return { ...s, auto: r.lens, params }
  return { ...s, auto: r.lens, params, lens: r.lens, focus: r.focus, callId: e.call_id }
}

export function pin(s: LensState, lens: Lens, params?: Partial<LensParams>, focus?: string | null): LensState {
  return { ...s, mode: 'pinned', lens, params: { ...s.params, ...params }, focus: focus === undefined ? s.focus : focus, callId: null }
}

export function follow(s: LensState): LensState {
  return { ...s, mode: 'auto', lens: s.auto }
}

/** A trail row replayed as pinned (73 §6.1); error rows replay nothing. */
export function replay(s: LensState, row: { tool: CodeTool; args: Record<string, unknown>; summary: CodeToolSummary; callId: string }): LensState {
  const r = lensFor(row.tool, row.args, row.summary)
  return r ? { ...pin(s, r.lens, r.params, r.focus), callId: row.callId } : s
}
