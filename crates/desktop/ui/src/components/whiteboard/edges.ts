// 218 design §4.5 "Off-screen work": a chevron on the edge of the whiteboard pointing at each card the agent is working
// on outside the view (a placeholder, or a card its latest turn touched). Pure geometry; the view renders the result.

export type Rect = { left: number; top: number; width: number; height: number }
export type EdgeMark = { id: string; x: number; y: number; angle: number }

/** Where each off-screen target's chevron sits: on the view's edge (inset), on the line from the centre to the target. */
export function edgeMarks(view: Rect, targets: readonly { id: string; rect: Rect }[], inset = 28): EdgeMark[] {
  const cx = view.left + view.width / 2
  const cy = view.top + view.height / 2
  const hw = Math.max(1, view.width / 2 - inset)
  const hh = Math.max(1, view.height / 2 - inset)
  const out: EdgeMark[] = []
  for (const { id, rect } of targets) {
    const visible =
      rect.left < view.left + view.width && rect.left + rect.width > view.left && rect.top < view.top + view.height && rect.top + rect.height > view.top
    if (visible) continue
    const dx = rect.left + rect.width / 2 - cx
    const dy = rect.top + rect.height / 2 - cy
    // Scale the direction onto the inset box's border.
    const s = 1 / Math.max(Math.abs(dx) / hw, Math.abs(dy) / hh, 1e-9)
    out.push({ id, x: cx + dx * s - view.left, y: cy + dy * s - view.top, angle: (Math.atan2(dy, dx) * 180) / Math.PI })
  }
  return out
}

/** Follow (design §4.5): a finger touch turns it off for this long, then it comes back. */
export const FOLLOW_PAUSE_MS = 60_000
