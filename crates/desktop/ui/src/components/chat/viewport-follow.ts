/** What `followViewport` reads and writes on the transcript's scroller. */
export type Scroller = { clientHeight: number; scrollHeight: number; scrollTop: number }

/**
 * The scroller's viewport changed height while its content did not (the background area's dash row or the composer
 * grew): pinned to the bottom, land on the bottom again so the last lines stay
 * visible; scrolled up, leave the position alone. A growing viewport needs no write: the browser clamps `scrollTop`.
 * Returns the height to remember for the next call.
 */
export function followViewport(el: Scroller, stuck: boolean, last: number): number {
  if (el.clientHeight === last) return last
  if (stuck && el.clientHeight < last) el.scrollTop = el.scrollHeight
  return el.clientHeight
}

/**
 * The scroller's content changed height (deltas, measurements, the "Next:" suggestions after the last reply mounting
 * or going): pinned to the bottom, land on the new bottom so they stay in view; scrolled up, nothing moves. Returns the
 * content height to remember; an unchanged one writes nothing, so scroll-only renders never undo a small upward scroll.
 */
export function followContent(el: Scroller, stuck: boolean, last: number): number {
  if (el.scrollHeight === last) return last
  if (stuck) el.scrollTop = el.scrollHeight
  return el.scrollHeight
}
