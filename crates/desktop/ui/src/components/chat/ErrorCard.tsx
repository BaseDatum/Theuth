import { ChevronRight, CircleAlert, FileWarning, Gauge, Hourglass, KeyRound, type LucideIcon, ServerCrash, TriangleAlert, WifiOff } from 'lucide-react'
import { memo, useMemo } from 'react'
import { Details } from './disclosure'
import { type ErrorKind, parseError, providerName } from './error-card'
import { CopyButton } from './MessageAssistant'

const ICONS: Record<ErrorKind, LucideIcon> = {
  overloaded: Gauge,
  rate_limited: Hourglass,
  auth: KeyRound,
  context_too_long: FileWarning,
  bad_request: CircleAlert,
  server: ServerCrash,
  timeout: Hourglass,
  network: WifiOff,
  unknown: TriangleAlert,
}

/** Lines of the message shown before `+N more`; the rest stays in the details. */
const SHOWN_LINES = 2

const CHIP = 'max-w-full truncate rounded-full border border-danger-border/30 bg-layer-2 px-2 text-xs leading-5 text-fg-secondary'
const PRE = 'mt-1 max-h-80 overflow-auto rounded-md bg-layer-2 px-3 py-2 font-mono text-xs leading-5 whitespace-pre-wrap wrap-anywhere text-fg-secondary'

/** An `error` row as a card: danger header (icon, title), the provider's own message, chips for model / provider /
 * status, a hint, and the full text verbatim under a collapsed `Full error` fold. `child`: the subagent a parent relays
 * it from (else read from a saved `↳ name error:` prefix). A one-line error with nothing to parse is the header alone. */
export const ErrorCard = memo(function ErrorCard({ text, child }: { text: string; child?: string }) {
  const e = useMemo(() => parseError(text), [text])
  const Icon = ICONS[e.kind]
  const from = child ?? e.child
  const lines = e.message?.split('\n').filter((l) => l.trim()) ?? []
  const more = lines.length - SHOWN_LINES
  const providerChip = e.adapter ?? (e.provider ? providerName(e.provider) : undefined)
  const statusChip = e.status !== undefined ? `${e.status} ${e.errorType ?? e.reason ?? ''}`.trim() : e.errorType
  const showBody = e.prettyBody !== undefined && e.prettyBody !== e.body
  return (
    <div role="alert" className="my-2 min-w-0 rounded-xl border border-danger-border/40 bg-danger-bg/40 px-4 py-3 text-sm">
      {from && <div className="mb-1 truncate text-xs text-fg-tertiary">↳ {from}</div>}
      <div className="flex min-w-0 items-start gap-2">
        <Icon className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden="true" />
        <span className={`min-w-0 flex-1 font-medium text-danger wrap-anywhere ${e.compact ? 'whitespace-pre-wrap' : ''}`}>{e.title}</span>
      </div>
      {lines.length > 0 && (
        <div className="mt-1.5 pl-6 leading-6 wrap-anywhere text-fg-primary">
          {lines.slice(0, SHOWN_LINES).map((l, i) => (
            <p key={i}>{l}</p>
          ))}
          {more > 0 && <p className="text-xs text-fg-tertiary">+{more} more</p>}
        </div>
      )}
      {(e.model || providerChip || statusChip) && (
        <div className="mt-2 flex min-w-0 flex-wrap gap-1.5 pl-6">
          {e.model && (
            <span className={`${CHIP} font-mono`} title={`model ${e.model}`}>
              {e.model}
            </span>
          )}
          {providerChip && (
            <span className={CHIP} title={e.adapter ? `adapter ${e.adapter}` : `provider ${e.provider}`}>
              {providerChip}
            </span>
          )}
          {statusChip && (
            <span className={`${CHIP} font-mono`} title={e.reason ? `HTTP ${e.status} ${e.reason}` : undefined}>
              {statusChip}
            </span>
          )}
        </div>
      )}
      {e.hint && <p className="mt-2 pl-6 text-xs text-fg-secondary">{e.hint}</p>}
      {!e.compact && (
        <Details
          part="error"
          className="group/error mt-2 border-t border-danger-border/20 pt-2"
          triggerClassName="flex min-h-8 items-center gap-1 rounded-sm text-xs text-fg-tertiary hover:text-fg-secondary focus-visible:outline-2"
          trigger={
            <>
              <ChevronRight className="size-3.5 shrink-0 transition-transform group-data-[state=open]/error:rotate-90" aria-hidden="true" />
              <span>Full error</span>
            </>
          }
        >
          <div className="mt-1 min-w-0">
            <div className="flex justify-end">
              <CopyButton text={e.text} label="Copy error" />
            </div>
            <pre className={PRE}>{e.text}</pre>
            {showBody && (
              <>
                <div className="mt-2 text-xs text-fg-tertiary">Body (formatted)</div>
                <pre className={PRE}>{e.prettyBody}</pre>
              </>
            )}
          </div>
        </Details>
      )}
    </div>
  )
})
