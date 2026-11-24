// 217 §3: the active engine's bus events per second, for the status bar's sparkline and readout. `attach()` in App.tsx
// ticks once per event; the count lives outside zustand state, so only a finished second (or the host's once-a-second
// `roll`) re-renders.

import { create } from 'zustand'

export const BUCKETS = 30

const zeros = (n: number): number[] => Array.from({ length: n }, () => 0)

type EventRateState = {
  /** The last `BUCKETS` complete seconds, oldest first. */
  buckets: number[]
  tick(n?: number): void
  /** Close the seconds that ended by `now`, zeros for the silent ones. */
  roll(now?: number): void
  /** Engine switch: start over. */
  reset(): void
}

export const useEventRate = create<EventRateState>()((set, get) => {
  let count = 0
  let second = -1
  return {
    buckets: zeros(BUCKETS),
    tick(n = 1) {
      get().roll()
      count += n
    },
    roll(now = Date.now()) {
      const s = Math.floor(now / 1000)
      // A clock that steps back keeps counting into the open second rather than reopening a closed one.
      if (s <= second) return
      if (second >= 0) {
        const silent = zeros(Math.min(s - second - 1, BUCKETS))
        set({ buckets: [...get().buckets, count, ...silent].slice(-BUCKETS) })
      }
      count = 0
      second = s
    },
    reset() {
      count = 0
      second = -1
      set({ buckets: zeros(BUCKETS) })
    },
  }
})

/** The mean of the last two complete seconds. */
export const rate = (buckets: readonly number[]): number => ((buckets.at(-1) ?? 0) + (buckets.at(-2) ?? 0)) / 2

/**
 * `38`, `1.5k`, `15k`. Below 10k one decimal, rounded half up on the hundreds, so 9950 reads `10k` rather than a
 * `9.9k` that undersells it; a whole thousand drops its `.0`.
 */
export function formatRate(n: number): string {
  const r = Math.round(n)
  if (r < 1000) return String(r)
  const tenths = Math.round(r / 100) / 10
  return tenths < 10 ? `${tenths}k` : `${Math.round(r / 1000)}k`
}
