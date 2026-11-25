import type { Row } from './rows'

/**
 * Which scroll-area rows take the `A · Paper & Ink` entrance when they arrive live: tool runs, tool/deliverable cards,
 * prompts/forms, assistant blocks, peer/harness/info/error cards. User bubbles and time gaps stay grounded.
 */
export function shouldAnimateRow(row: Row): boolean {
  if ('gap' in row) return false
  if ('entry' in row) return row.entry.kind !== 'user'
  return true
}

/** Max newly-unseen rows in a single render that still count as a live arrival (guards against un-collapsing history). */
const LIVE_BURST_CAP = 4

/**
 * Tracks which transcript row keys arrived live during this session so only freshly appended blocks animate:
 * - Initial history hydration for a session seeds `seen` without animating.
 * - Prepending older pages (`i < firstSeenIndex`) seeds `seen` without animating.
 * - Virtualizer unmount/remount never replays because finished keys leave `active` while staying in `seen`.
 * - Toggling tool-run collapse across history (`> LIVE_BURST_CAP` unseen rows at once) stays instant.
 */
export class LiveRowTracker {
  private session = ''
  private hydrated = false
  private seen = new Set<string>()
  private active = new Set<string>()

  sync(session: string, rows: readonly Row[], running: boolean): void {
    if (session !== this.session) {
      this.session = session
      this.hydrated = false
      this.seen.clear()
      this.active.clear()
    }

    if (!this.hydrated) {
      if (rows.length === 0) return
      this.hydrated = true
      if (running && rows.length <= 2) {
        for (const row of rows) {
          this.seen.add(row.key)
          if (shouldAnimateRow(row)) this.active.add(row.key)
        }
        return
      }
      for (const row of rows) this.seen.add(row.key)
      return
    }

    if (rows.length === 0) return

    let firstSeen = -1
    for (let i = 0; i < rows.length; i++) {
      if (this.seen.has(rows[i]!.key)) {
        firstSeen = i
        break
      }
    }

    // Whole row set replaced (e.g. session reset/reload): re-seed silently.
    if (firstSeen === -1) {
      for (const row of rows) this.seen.add(row.key)
      return
    }

    // Leading rows before `firstSeen` are prepended older history pages — record silently.
    for (let i = 0; i < firstSeen; i++) this.seen.add(rows[i]!.key)

    const candidates: string[] = []
    for (let i = firstSeen + 1; i < rows.length; i++) {
      const row = rows[i]!
      if (this.seen.has(row.key)) continue
      this.seen.add(row.key)
      if (shouldAnimateRow(row)) candidates.push(row.key)
    }

    if (candidates.length > 0 && candidates.length <= LIVE_BURST_CAP) {
      for (const key of candidates) this.active.add(key)
    }
  }

  isEntering(key: string): boolean {
    return this.active.has(key)
  }

  finish(key: string): void {
    this.active.delete(key)
  }
}

const MAX_SURFACES = 256
const animatedSurfaces = new Set<string>()

/**
 * Claims a one-shot Paper & Ink entrance for a live-streaming surface (`json-render`, `openui`, `mermaid`) so it
 * animates once on first appearance and never replays if virtualised out and back in.
 */
export function claimSurfaceEntrance(key: string): boolean {
  if (animatedSurfaces.has(key)) return false
  if (animatedSurfaces.size >= MAX_SURFACES) {
    const oldest = animatedSurfaces.values().next().value
    if (oldest !== undefined) animatedSurfaces.delete(oldest)
  }
  animatedSurfaces.add(key)
  return true
}

/** Test-only reset for `claimSurfaceEntrance`. */
export function resetSurfaceEntrances(): void {
  animatedSurfaces.clear()
}
