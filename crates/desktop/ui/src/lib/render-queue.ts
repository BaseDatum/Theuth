// Design 65 §4.3: a streaming diagram re-renders at most every `intervalMs`, never concurrently, and always
// ends on the latest source. One render in flight, one trailing request (the newest replaces the older),
// at least `intervalMs` between starts. Not a resetting debounce: a fence that streams for ten seconds
// still redraws every `intervalMs`.

export interface RenderQueue<T> {
  /** Queue `req`; it replaces any request not yet started. */
  request(req: T): void
  /** Drop the trailing request; `isCurrent()` of the running render turns false. */
  dispose(): void
}

/**
 * `render(req, isCurrent)` runs one request; it should check `isCurrent()` before touching state, since a newer
 * request (or `dispose`) may have superseded it while it awaited. A rejected render is swallowed here — the
 * renderer reports its own errors.
 */
export function createRenderQueue<T>(render: (req: T, isCurrent: () => boolean) => Promise<void>, intervalMs = 80): RenderQueue<T> {
  let pending: { req: T } | null = null
  let running = false
  let lastStart = Number.NEGATIVE_INFINITY
  let scheduled = false
  let seq = 0
  let disposed = false

  const start = () => {
    if (running || disposed || !pending) return
    const wait = lastStart + intervalMs - Date.now()
    if (wait > 0) {
      if (!scheduled) {
        scheduled = true
        setTimeout(() => {
          scheduled = false
          start()
        }, wait)
      }
      return
    }
    const { req } = pending
    pending = null
    running = true
    lastStart = Date.now()
    const id = seq
    const isCurrent = () => id === seq && !disposed
    Promise.resolve()
      .then(() => render(req, isCurrent))
      .catch(() => {})
      .finally(() => {
        running = false
        start()
      })
  }

  return {
    request(req) {
      if (disposed) return
      seq++
      pending = { req }
      start()
    },
    dispose() {
      disposed = true
      pending = null
    },
  }
}
