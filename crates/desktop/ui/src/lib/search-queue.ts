// 49 §4.3/§5.3 (SEARCH-DEBOUNCE): every owner matches under a one-permit semaphore and refuses a second
// concurrent query ("… is busy; retry") — by design, so the client coalesces. One `SearchQueue` per search
// surface enforces the discipline: a resetting debounce (nothing is sent while typing continues), at most
// one `search` RPC in flight, the latest settled query held as pending behind it (depth 1: an older pending
// query is dropped when a newer one arrives), and a clear that cancels both. Results live in React Query
// under a key that carries the query text, so an in-flight answer for an older query lands under a key
// nothing observes: stale responses and stale errors are discarded the same way.

export const DEBOUNCE_MS = 200

/** Rejection of a pending request a newer query replaced (or `clear` dropped) before it was sent. */
export const SUPERSEDED = new Error('superseded by a newer search')

type Pending = { query: string; start(): void; drop(): void }

export class SearchQueue {
  /** `globalThis.setTimeout` handle: a number in the browser, an object under vitest's node runtime. */
  private timer: number | NodeJS.Timeout | undefined
  private inflight = 0
  /** Every entry is for the same query text; sent FIFO once the slot frees. */
  private pending: Pending[] = []

  constructor(readonly debounceMs = DEBOUNCE_MS) {}

  /** Restart the debounce: `settle(query)` fires only once typing has paused for `debounceMs`. */
  type(query: string, settle: (query: string) => void): void {
    clearTimeout(this.timer)
    this.timer = setTimeout(() => {
      this.timer = undefined
      settle(query)
    }, this.debounceMs)
  }

  /** Stop a running debounce (Enter settled it by hand, or the effect re-ran). */
  cancel(): void {
    clearTimeout(this.timer)
    this.timer = undefined
  }

  /**
   * Send through the single slot. Runs `send` now when nothing is in flight; otherwise waits behind it.
   * Requests for one query text queue FIFO (the base page, a Show more page, an indexing poll); a request
   * for a different text drops every waiting one with `SUPERSEDED`. An in-flight request is never
   * cancelled — its caller decides whether the answer still matters.
   */
  run<T>(query: string, send: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const start = () => {
        this.inflight++
        send().then(resolve, reject).finally(() => {
          this.inflight--
          this.next()
        })
      }
      if (this.inflight === 0 && this.pending.length === 0) return start()
      if (this.pending[0]?.query !== query) this.dropPending()
      this.pending.push({ query, start, drop: () => reject(SUPERSEDED) })
    })
  }

  /** Whether a request is in flight or waiting. */
  get busy(): boolean {
    return this.inflight > 0 || this.pending.length > 0
  }

  /** Box emptied or surface closed: no debounce fires, nothing waiting is sent. */
  clear(): void {
    this.cancel()
    this.dropPending()
  }

  private dropPending(): void {
    const dropped = this.pending
    this.pending = []
    for (const p of dropped) p.drop()
  }

  private next(): void {
    if (this.inflight > 0) return
    this.pending.shift()?.start()
  }
}
