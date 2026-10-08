// 125 §8: the pure half of the advisor UI — severity tones, the pill's label by state, note counts and the
// popover's context/tokens lines — so the components stay thin and the logic is vitest-able.

import type { AdvisorSeverity } from '@/rpc/bindings/AdvisorSeverity'
import type { AdvisorState } from '@/rpc/bindings/AdvisorState'
import type { AdvisorStatus } from '@/rpc/bindings/AdvisorStatus'
import type { AdvisoryNote } from '@/rpc/bindings/AdvisoryNote'
import { k } from '@/components/chat/CostPill'

/** Severity order for the card's worst-of pick; `advice` (a consultation's answer, 125 R1) never wins. */
const RANK: Record<AdvisorSeverity, number> = { advice: 0, nit: 1, concern: 2, blocker: 3 }

/** The severity a card is tinted by: the worst of its notes (`nit` for none). */
export function worstSeverity(notes: readonly AdvisoryNote[]): AdvisorSeverity {
  let worst: AdvisorSeverity = 'nit'
  for (const n of notes) if (RANK[n.severity] > RANK[worst]) worst = n.severity
  return worst
}

/** The status tokens of `styles.css` by severity (§8.3): advice/nit → info, concern → warning, blocker → danger. */
export const TONE: Record<AdvisorSeverity, { shell: string; fg: string; border: string }> = {
  advice: { shell: 'border-info-border/30 bg-info-bg/40', fg: 'text-info', border: 'border-info-border/20' },
  nit: { shell: 'border-info-border/30 bg-info-bg/40', fg: 'text-info', border: 'border-info-border/20' },
  concern: { shell: 'border-warning-border/30 bg-warning-bg/40', fg: 'text-warning', border: 'border-warning-border/20' },
  blocker: { shell: 'border-danger-border/30 bg-danger-bg/40', fg: 'text-danger', border: 'border-danger-border/20' },
}

/** `Advisor · 2 notes` (§8.6). */
export const cardTitle = (count: number): string => `Advisor · ${count} note${count === 1 ? '' : 's'}`

/** How many notes a card shows collapsed (§8.3). */
export const NOTES_SHOWN = 3

/** The pill's text by state (§8.2): `idle` · `reviewing…` · `N notes` · `paused` · `halted`; `''` when off. */
export function pillLabel(s: Pick<AdvisorStatus, 'state' | 'notes'>): string {
  switch (s.state) {
    case 'off':
      return ''
    case 'reviewing':
      return 'reviewing…'
    case 'paused':
      return 'paused'
    case 'halted':
      return 'halted'
    case 'idle': {
      const n = noteCount(s.notes)
      return n === 0 ? 'idle' : `${n} note${n === 1 ? '' : 's'}`
    }
  }
}

/** Every note the advisor emitted for this link. */
export const noteCount = (c: AdvisorStatus['notes']): number => c.nit + c.concern + c.blocker

/** The pill's colour by state (§8.2). */
export const STATE_FG: Record<AdvisorState, string> = {
  off: 'text-fg-secondary',
  idle: 'text-fg-secondary',
  reviewing: 'text-info',
  paused: 'text-warning',
  halted: 'text-danger',
}

/** `38.2k / 200k · 19%` (§8.2); `—` when the window is unknown. */
export function contextLine(used: number, limit: number): string {
  if (limit <= 0) return used > 0 ? k(used) : '—'
  return `${k(used)} / ${k(limit)} · ${Math.round((used / limit) * 100)}%`
}

/** `1.2k in / 340 out`. */
export const tokensLine = (tokensIn: number, tokensOut: number): string => `${k(tokensIn)} in / ${k(tokensOut)} out`

/** The popover's one-line reason when paused/halted: `Advisor paused — usage limit` / `Advisor halted — <detail>` (§8.6). */
export function detailLine(s: Pick<AdvisorStatus, 'state' | 'detail'>): string | undefined {
  if (s.state !== 'paused' && s.state !== 'halted') return undefined
  return `Advisor ${s.state} — ${s.detail ?? (s.state === 'paused' ? 'usage limit' : 'model error')}`
}
