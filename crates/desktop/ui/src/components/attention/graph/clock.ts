// 208: the region's clock in engine time. Node ages are `now - last_event_ms` with `last_event_ms` stamped by the
// engine, so a client whose clock is off by minutes would expire 10-minute children early or keep them far too long.
// Every snapshot and delta carries the engine's `at_ms`; the skew is re-measured from each one.

let clock: () => number = Date.now
let skew = 0

/** Dev fixture only: the local clock the region reads (null restores `Date.now`). */
export function setGraphClock(fn: (() => number) | null): void {
  clock = fn ?? Date.now
  skew = 0
}

/** An engine timestamp just received (`at_ms` of a snapshot or delta): the skew becomes `atMs - local now`. */
export function noteEngineTime(atMs: number): void {
  if (Number.isFinite(atMs) && atMs > 0) skew = atMs - clock()
}

/** The region's `now`, in engine time. */
export function graphNow(): number {
  return clock() + skew
}
