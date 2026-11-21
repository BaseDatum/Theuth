import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'
import type { CompactorProfile, EndlessConfig, TailMode } from "@/rpc/bindings/method.agent.output"

/** The engine's defaults of `EndlessConfig`'s optional keys (137F/137G, 201 §12.1) and `jobs`: an engine before a key omits it, so these are the placeholders and the values used while it is absent. */
export const ENDLESS_DEFAULTS: Required<
  Pick<
    EndlessConfig,
    | 'attempt_secs'
    | 'local_attempt_secs'
    | 'local_ctx_bytes'
    | 'local_jobs'
    | 'raw_bytes'
    | 'compactor_ctx_bytes'
    | 'batch_units'
    | 'max_usd_per_call'
    | 'max_usd_per_session_day'
    | 'jobs'
    | 'tail_mode'
    | 'tail_low_bytes'
    | 'tail_high_bytes'
    | 'tail_low_share'
    | 'tail_high_share'
    | 'cold_after_secs'
    | 'profile'
    | 'merge_at'
    | 'ledger'
    | 'ledger_bytes'
    | 'ledger_suggestions'
    | 'prefetch_bytes'
    | 'age_by_use'
    | 'describe_images'
  >
> = {
  attempt_secs: 120,
  local_attempt_secs: 600,
  local_ctx_bytes: 24_000,
  local_jobs: 1,
  raw_bytes: 32_000,
  compactor_ctx_bytes: 48_000,
  batch_units: 16,
  max_usd_per_call: 0.1,
  max_usd_per_session_day: 5,
  jobs: 8,
  tail_mode: 'fixed',
  tail_low_bytes: 24_000,
  tail_high_bytes: 64_000,
  tail_low_share: 0.1,
  tail_high_share: 0.25,
  cold_after_secs: 0,
  profile: 'auto',
  merge_at: 0.75,
  ledger: true,
  ledger_bytes: 4_000,
  ledger_suggestions: true,
  prefetch_bytes: 2_000,
  age_by_use: true,
  describe_images: 'auto',
}

/** 201 §12.1: the tail marks' ranges per mode, in the wire unit (bytes for `fixed`, shares for `scaled`). */
export const TAIL_RANGES = {
  fixed: { low: [8_000, 256_000], high: [16_000, 512_000] },
  scaled: { low: [0.02, 0.4], high: [0.05, 0.6] },
} as const

/** A tail mark as the UI names it: `64 KB` for bytes, `25 %` for a share. */
export const tailMark = (mode: TailMode, v: number): string => (mode === 'fixed' ? `${Math.round(v / 1000)} KB` : `${Math.round(v * 100)} %`)

/**
 * 201 §12.1: why a tail pair would be refused, in the UI's words; `null` when it is valid. The engine refuses the same
 * pair (`validate_201`), so the page checks it in place before it writes.
 */
export function tailPairError(mode: TailMode, low: number, high: number): string | null {
  const r = TAIL_RANGES[mode]
  if (low < r.low[0] || low > r.low[1]) return `Kept after a trim must be ${tailMark(mode, r.low[0])}–${tailMark(mode, r.low[1])}.`
  if (high < r.high[0] || high > r.high[1]) return `Trim at must be ${tailMark(mode, r.high[0])}–${tailMark(mode, r.high[1])}.`
  if (high <= low) return `Trim at (${tailMark(mode, high)}) must be above Kept after a trim (${tailMark(mode, low)}).`
  return null
}

/**
 * 201 §4.2 `tail_marks`, estimated in the UI for a model of `contextTokens`: the turn allowance taken as the window at
 * ~3 bytes per token, both marks clamped to 8–512 KB, scaled down together past half the allowance. `null` without a
 * known window. An estimate: the engine's allowance also leaves room for the reply.
 */
export function scaledMarks(lowShare: number, highShare: number, contextTokens: number): { low: number; high: number } | null {
  if (!(contextTokens > 0)) return null
  const allowance = contextTokens * 3
  const clamp = (v: number) => Math.min(512_000, Math.max(8_000, v))
  let low = clamp(lowShare * allowance)
  let high = Math.max(clamp(highShare * allowance), low + 1)
  if (high > allowance / 2) {
    const k = allowance / 2 / high
    low *= k
    high *= k
  }
  return { low: Math.round(low), high: Math.round(high) }
}

/** 201 §7.1 `auto` as the settings page can tell it: `small` for a local model or a window of 32 k tokens or less. */
export function autoProfile(model: { local?: boolean; context: number } | undefined): { profile: CompactorProfile; reason: string } | null {
  if (!model) return null
  if (model.local) return { profile: 'small', reason: 'the model is local' }
  if (model.context > 0 && model.context <= 32_000) return { profile: 'small', reason: 'the model reads 32 k tokens or less' }
  return { profile: 'standard', reason: model.context > 0 ? 'the model reads more than 32 k tokens' : 'the model is a cloud model' }
}

/** 137 R1-D4: the first view line of a capped backfill; the Memory panel renders it as a note, not a band line. */
export const EARLIER_HISTORY = '(earlier history, not summarized; recall searches it)'

/**
 * 137G: the view's placeholder lines (`fold.rs` `folded_text` / `prefix_text`): `(4512 lines not yet summarized — recall
 * searches them)` for waiting lines, `(4512 older lines folded to fit the view — zoom or recall reach them)` for built
 * lines that cannot merge yet. No marker in the bindings: matched on the text.
 */
export const isFoldedNote = (text: string): boolean => /^\([\d,]+ (?:lines? not yet summarized|older lines? folded to fit the view)\b/.test(text)

/**
 * 137 R1-D3, the compactor chain as the UI names it: `agent.endless.model` when set, else
 * "Same as title model · <agent.title_model>" when one is set, else "Same as the session".
 */
export function useCompactorLabel(model: string | null | undefined, enabled = true): string {
  const canQuery1 = useQuerySupport(["title_model"])
  const api = useApi()
  const title = useQuery({ queryKey: queryKeys.titleModel, queryFn: () => api.title_model(), enabled: enabled && canQuery1 && (!model) })
  if (model) return model
  return title.data ? `Same as title model · ${title.data.id}` : 'Same as the session'
}

/** `about 21 k tokens` for 64,000 bytes: the page's one estimate, ~3 bytes per token (137 §2; `scaledMarks` uses the
 * same). R3-13: an estimate, so it says "about"; the engine counts per model. */
export const tokenEstimate = (bytes: number): string => `about ${Math.round(bytes / 3 / 1000)} k tokens`
