// Design 135 §3.2: the pure core of the transcript's top slack. A size change above what the reader is looking at
// is absorbed by shrinking the slack (the virtualizer's `paddingStart`), so nothing visible moves and `scrollTop` is
// never written while the user scrolls; the slack is refilled toward `target` with one compensated write at rest.
// `library: true` hands the change back to the virtualizer's own adjustment (slack exhausted: today's behaviour).

/** Row under the viewport's top edge, and the anchor element's top relative to the scroller. */
export type Anchor = { index: number; offset: number }
export type State = { slack: number; target: number; anchor: Anchor | null }
export type Event =
  /** A row resized; `moved` = how far the anchor element moved in the DOM (0 unless `index === anchor.index`). */
  | { kind: 'grow'; index: number; delta: number; moved: number }
  /** Rows were prepended; `estimated` = Σ estimateSize over the new leading rows. */
  | { kind: 'prepend'; estimated: number }
  /** Scroller at rest: rebase toward `target`. `scrollTop` bounds a negative write. */
  | { kind: 'rest'; scrollTop: number }
  | { kind: 'target'; target: number }
export type Effect = { slack: number; write: number; library: boolean }

const none = (s: State): [State, Effect] => [s, { slack: s.slack, write: 0, library: false }]

export function step(s: State, e: Event): [State, Effect] {
  switch (e.kind) {
    case 'grow': {
      const a = s.anchor
      if (!a || e.index > a.index) return none(s)
      if (e.index < a.index) {
        // The whole delta would move the anchor row at commit: absorb it, or let the library write exactly.
        if (s.slack - e.delta < 0) return [s, { slack: s.slack, write: 0, library: true }]
        const slack = s.slack - e.delta
        return [{ ...s, slack }, { slack, write: 0, library: false }]
      }
      // Anchor row: only growth above the anchor element matters; growth below is content the reader has not reached.
      // Never `library: true` here — its rule would drag the viewport on every streaming delta (#1218).
      if (e.moved === 0) return none(s)
      if (s.slack - e.moved < 0) return none({ ...s, anchor: { index: a.index, offset: a.offset + e.moved } })
      const slack = s.slack - e.moved
      return [{ ...s, slack }, { slack, write: 0, library: false }]
    }
    case 'prepend': {
      // The remainder (`estimated − absorbed`) is handled by `anchorTo: 'end'`: the library's anchor is computed from
      // measurements that already include the reduced `paddingStart`.
      const slack = Math.max(0, s.slack - e.estimated)
      return [{ ...s, slack }, { slack, write: 0, library: false }]
    }
    case 'rest': {
      // Compensated write: positions and `scrollTop` move together, nothing visible changes — unless the viewport top
      // is inside the slack (blank, the reader out-ran a page), where the write is bounded by `scrollTop ≥ 0` and the
      // rows slide up into view instead of leaving a stranded blank above the oldest row.
      const write = Math.max(s.target - s.slack, -e.scrollTop)
      return [{ ...s, slack: s.target }, { slack: s.target, write, library: false }]
    }
    case 'target':
      return none({ ...s, target: e.target })
  }
}
