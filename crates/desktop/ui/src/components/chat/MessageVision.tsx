import { ChevronRight, Eye } from 'lucide-react'
import { memo } from 'react'
import type { Entry } from '@/store/sessions'

type Vision = Extract<Entry, { kind: 'vision' }>

/**
 * A vision consultation note (52 §6): collapsed to `Vision observation · <model>` with one chip per inspected source,
 * expanding to the main model's question and the fallback's answer as stored. Image-derived, fallible evidence: never
 * rendered as a `you` bubble. Statuses come from the typed note; a live card after a failed consultation marks every
 * source NOT SEEN, a live card after success shows the labels until the settled reload brings the note.
 */
export const MessageVision = memo(function MessageVision({ entry }: { entry: Vision }) {
  const failed = new Set(entry.note?.observations.flatMap((o) => (o.status === 'failed' ? [o.source.fingerprint] : [])))
  // The `<vision_observation>` envelope is the model's marker, not for humans.
  const body = entry.text.replace(/^\s*<vision_observation>\s*/, '').replace(/\s*<\/vision_observation>\s*$/, '')
  return (
    <details className="group/vision my-2 min-w-0 rounded-xl border border-info-border/30 bg-info-bg/40 px-4 py-3 text-sm">
      <summary className="flex min-h-11 cursor-pointer list-none items-center gap-3 rounded-sm focus-visible:outline-2 [&::-webkit-details-marker]:hidden">
        <Eye className="size-4 shrink-0 text-info" aria-hidden="true" />
        <span className="min-w-0 flex-1">
          <span className="block font-medium text-info">Vision observation · {entry.model}</span>
          <span className="flex flex-wrap gap-1 text-xs text-fg-secondary">
            {entry.sources.map((s) => {
              const notSeen = entry.failed || failed.has(s.fingerprint)
              return (
                <span key={JSON.stringify(s.target)} className="rounded bg-layer-2 px-1.5 py-0.5">
                  {s.label}
                  {notSeen && <span className="ml-1 font-medium text-danger">NOT SEEN</span>}
                </span>
              )
            })}
          </span>
        </span>
        <ChevronRight className="size-4 shrink-0 text-info transition-transform group-open/vision:rotate-90" aria-hidden="true" />
      </summary>
      <div className="mt-3 border-t border-info-border/20 pt-3 leading-6 wrap-anywhere whitespace-pre-wrap text-fg-secondary">
        {body || <span className="italic">Question and answer arrive when the turn settles.</span>}
      </div>
    </details>
  )
})
