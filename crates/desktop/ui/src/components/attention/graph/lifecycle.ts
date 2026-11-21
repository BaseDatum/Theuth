// 208 review U4: when the graph's stream runs, how failed calls retry, when the strip says "stale". Pure; the hook in
// `queries.ts` applies it.

/** Retry delays after the 1st, 2nd and later consecutive failed `agent_graph` calls. */
export const RETRY_MS = [5_000, 15_000, 60_000] as const

/**
 * The snapshot + delta stream + 120 s renewal run while the Attention page is mounted (the region's strip shows live
 * counts collapsed too), the engine supports the graph, a source exists and the document is visible. Hidden → paused
 * (the engine's lease runs out after 300 s); visible again → a fresh full snapshot.
 */
export function streamRuns(p: { supported: boolean; source: boolean; visible: boolean }): boolean {
  return p.supported && p.source && p.visible
}

/** The delay before retrying after `failures` consecutive failed calls (≥ 1); the last delay repeats. */
export function retryDelay(failures: number): number {
  return RETRY_MS[Math.min(Math.max(failures, 1), RETRY_MS.length) - 1] ?? 60_000
}

/** The strip's "stale" marker: the last call failed but an earlier state is still shown. */
export function isStale(hasState: boolean, error: string | null): boolean {
  return hasState && error !== null
}
