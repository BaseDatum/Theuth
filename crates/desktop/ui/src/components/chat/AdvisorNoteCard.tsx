import { ChevronRight, CircleX, Info, TriangleAlert } from 'lucide-react'
import { useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { NOTES_SHOWN, TONE, cardTitle, worstSeverity } from '@/lib/advisor-format'
import { formatAt, fullAt } from '@/lib/time'
import type { AdvisorSeverity } from '@/rpc/bindings/AdvisorSeverity'
import type { AdvisoryNote } from '@/rpc/bindings/AdvisoryNote'

const ICON: Record<AdvisorSeverity, typeof Info> = { advice: Info, nit: Info, concern: TriangleAlert, blocker: CircleX }

/** The notes a card lists: all of them when `expanded`, else the first `NOTES_SHOWN`; `hidden` is what `Show more (N)` reveals. */
export function collapseNotes(notes: readonly AdvisoryNote[], expanded: boolean): { shown: AdvisoryNote[]; hidden: number } {
  const shown = expanded ? [...notes] : notes.slice(0, NOTES_SHOWN)
  return { shown, hidden: notes.length - shown.length }
}

/**
 * 125 §8.3: one Advisory — every note the advisor raised at one step boundary — as the `EngineNotice` shell tinted by
 * the worst severity (nit → info, concern → warning, blocker → danger). Collapsed: the header line; open: one row per
 * note with its lower-case severity chip, three at a time behind `Show more (N)`.
 * R1.2 consultation variant (`question` set): a `Consulted` badge and the question as the title, the answer in the body.
 */
export function AdvisorNoteCard({ notes, at, question }: { notes: AdvisoryNote[]; at?: number; question?: string }) {
  const [expanded, setExpanded] = useState(false)
  const worst = worstSeverity(notes)
  const tone = TONE[worst]
  const Icon = ICON[worst]
  const { shown, hidden } = collapseNotes(notes, expanded)
  return (
    <details className={`group/advisory my-2 min-w-0 rounded-xl border px-4 py-3 text-sm ${tone.shell}`}>
      <summary className="flex min-h-11 cursor-pointer list-none items-center gap-3 rounded-sm focus-visible:outline-2 [&::-webkit-details-marker]:hidden">
        <Icon className={`size-4 shrink-0 ${tone.fg}`} aria-hidden="true" />
        <span className="min-w-0 flex-1">
          <span className={`flex items-center gap-2 font-medium ${tone.fg}`}>
            {question !== undefined && (
              <Badge variant="outline" className={`shrink-0 ${tone.fg}`}>
                Consulted
              </Badge>
            )}
            <span className="min-w-0 truncate">{cardTitle(notes.length, question)}</span>
          </span>
          <span className="block truncate text-xs text-fg-secondary">{notes[0]?.note}</span>
        </span>
        {at !== undefined && (
          <span className="shrink-0 text-xs text-fg-tertiary tabular-nums" title={fullAt(at)}>
            {formatAt(at)}
          </span>
        )}
        <ChevronRight className={`size-4 shrink-0 transition-transform group-open/advisory:rotate-90 ${tone.fg}`} aria-hidden="true" />
      </summary>
      <ul className={`mt-3 flex flex-col gap-2 border-t pt-3 leading-6 ${tone.border}`}>
        {shown.map((n, i) => (
          <li key={i} className="flex min-w-0 items-start gap-2">
            {question === undefined && (
              <Badge variant="outline" className={`mt-1 shrink-0 ${TONE[n.severity].fg}`}>
                {n.severity}
              </Badge>
            )}
            <span className="min-w-0 wrap-anywhere whitespace-pre-wrap text-fg-secondary">{n.note}</span>
          </li>
        ))}
      </ul>
      {notes.length > NOTES_SHOWN && (
        <button type="button" className="mt-2 min-h-11 text-xs text-fg-tertiary hover:text-fg-secondary" aria-expanded={expanded} onClick={() => setExpanded((v) => !v)}>
          {expanded ? 'Show less' : `Show more (${hidden})`}
        </button>
      )}
    </details>
  )
}
