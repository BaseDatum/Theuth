/** What `followViewport` reads and writes on the transcript's scroller. */
export type Scroller = { clientHeight: number; scrollHeight: number; scrollTop: number }

/**
 * The scroller's viewport changed height while its content did not (the "Next:" block above the background area
 * mounted, grew or went; the composer grew): pinned to the bottom, land on the bottom again so the last lines stay
 * visible; scrolled up, leave the position alone. A growing viewport needs no write: the browser clamps `scrollTop`.
 * Returns the height to remember for the next call.
 */
export function followViewport(el: Scroller, stuck: boolean, last: number): number {
  if (el.clientHeight === last) return last
  if (stuck && el.clientHeight < last) el.scrollTop = el.scrollHeight
  return el.clientHeight
}
