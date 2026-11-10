import { CircleHelp, Hourglass, X } from 'lucide-react'
import { useQueries } from '@tanstack/react-query'
import { Fragment, useMemo } from 'react'
import { toast } from 'sonner'
import { Streamdown } from 'streamdown'
import { queryKeys, useApi } from '@/lib/rpc'
import { formatAt } from '@/lib/time'
import { cn } from '@/lib/utils'
import { type Entry, useSessionField } from '@/store/sessions'
import { ToolSpinner } from './Indicators'
import { Chip, Clock } from './MessagePeer'
import { NIL, type Track, isOutside, parseReceipts, resolveSender, trackOf, useDirectory } from './peer-identity'

type Tool = Extract<Entry, { kind: 'tool' }>
type Peer = Extract<Entry, { kind: 'peer' }>

const STEPS = ['Sent', 'Read', 'Answered'] as const

/** Sent → Read → Answered, lit up to `step`. */
function TrackLine({ track }: { track: Track }) {
  return (
    <div className="mt-2.5 flex items-center text-[11.5px]" aria-label={`Delivery: ${track.word}`}>
      {STEPS.map((s, i) => (
        <Fragment key={s}>
          {i > 0 ? <span className={cn('mx-1.5 h-0.5 min-w-3.5 flex-1 rounded-full', i < track.step ? 'bg-success' : 'bg-border-subtle')} /> : null}
          <span className={cn('flex shrink-0 items-center gap-1.5', i < track.step ? 'text-success' : 'text-fg-tertiary')}>
            <span className="size-2 rounded-full bg-current" aria-hidden="true" />
            {s}
          </span>
        </Fragment>
      ))}
    </div>
  )
}

const NOTE: Partial<Record<Track['tone'], string>> = { warning: 'text-warning', danger: 'text-danger', muted: 'text-fg-secondary' }

/** `held` (warning), `failed`/`suppressed` (danger) and `unknown` (muted) say why, from the receipt's `error`. */
function Note({ track, who }: { track: Track; who?: string }) {
  if (!track.note && track.tone !== 'muted') return null
  const text = track.note ?? 'Outcome unknown.'
  return (
    <p className={cn('mt-2 flex items-start gap-2 text-[13px] wrap-anywhere', NOTE[track.tone])}>
      {track.tone === 'warning' ? <Hourglass className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" /> : track.tone === 'danger' ? <X className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" /> : <CircleHelp className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />}
      <span>
        {who ? <b className="font-semibold">{who}: </b> : null}
        {track.tone === 'muted' && track.note ? `Outcome unknown: ${track.note}` : text}
      </span>
    </p>
  )
}

/** Scroll the answer's card into view; a card outside the rendered window (or folded in a group) cannot be reached from here. */
function jump(id: string) {
  const el = document.getElementById(`peer-msg-${id}`)
  if (el) el.scrollIntoView({ block: 'center', behavior: 'smooth' })
  else toast.message('That message is not on screen: scroll down to it or open its group.')
}

/**
 * One `send_message` call (Sky, 2026-11-10): `You → <recipient>`, two lines of what was sent, and a live delivery track
 * per recipient. Each receipt's state is `peer_message(session, id)`, refetched by `PeerMessageChanged` (the
 * `queryKeys.peerMessage` invalidation in `lib/rpc`), so no polling. Answered: the first lines of `receipt.reply`, else of
 * the incoming message whose `reply_to` is this id, with a jump to its card. Several recipients: one chip each.
 */
export function SendCard({ tool, session, at }: { tool: Tool; session: string; at?: number }) {
  const api = useApi()
  const args = (tool.args ?? {}) as { to?: unknown; message?: unknown; await_reply?: unknown }
  const message = typeof args.message === 'string' ? args.message : ''
  const receipts = useMemo(() => (tool.result ? parseReceipts(tool.result.output) : []), [tool.result])
  const addresses = receipts.length
    ? receipts.map((r) => r.to)
    : typeof args.to === 'string'
      ? [args.to]
      : Array.isArray(args.to)
        ? args.to.filter((t): t is string => typeof t === 'string')
        : []
  const dir = useDirectory(session, addresses.some(isOutside))
  const records = useQueries({
    queries: receipts.map((r) => ({
      queryKey: queryKeys.peerMessage(session, r.id),
      queryFn: () => api.peer_message(session, r.id),
      enabled: r.id !== NIL,
    })),
  })
  const entries = useSessionField(session, 'entries')
  const rows = receipts.map((r, i) => {
    const rec = records[i]?.data?.receipt
    // A string, so `held` reads the same before and after the bindings name it.
    const state: string = rec?.state ?? r.state
    const error = rec?.error ?? r.error
    const answer = entries?.find((e): e is Peer => e.kind === 'peer' && e.origin?.reply_to === r.id)
    return { id: r.id, name: resolveSender(r.to, dir).name, track: trackOf(state, error), reply: rec?.reply ?? r.reply ?? answer?.text, answerAt: answer?.at, target: answer?.origin?.id }
  })
  const one = rows.length <= 1 ? rows[0] : undefined
  // `peer:<alias>/<export>`: the export is part of who it went to (`sky · hi-there`).
  const exp = addresses.length === 1 && addresses[0]?.startsWith('peer:') ? addresses[0].split('/')[1] : undefined
  const name = addresses.length > 1 ? `${addresses.length} recipients` : addresses[0] ? `${resolveSender(addresses[0], dir).name}${exp ? ` · ${exp}` : ''}` : '?'
  // Refused before any receipt (communication off, a bad `to`): the output is the reason.
  const refused = tool.result && receipts.length === 0 ? tool.result.output : undefined
  const tone = refused ? 'danger' : one?.track.tone
  return (
    <div
      className={cn(
        'my-2 min-w-0 rounded-xl border px-4 py-2.5 text-sm',
        tone === 'danger' ? 'border-danger-border bg-layer-1' : tone === 'warning' ? 'border-warning-border bg-warning-bg' : 'border-border-subtle bg-layer-2/40',
      )}
    >
      <div className="flex min-w-0 items-center gap-2 text-[13px]">
        <span className="shrink-0 text-fg-tertiary">You →</span>
        <span className="min-w-0 truncate font-semibold" title={addresses.join(', ')}>
          {name}
        </span>
        <span className="flex-1" />
        {tone === 'danger' ? <Chip tone="danger">Not delivered</Chip> : null}
        <Clock at={at} />
      </div>
      {message ? <p className="mt-1.5 line-clamp-2 text-[13.5px] text-fg-secondary wrap-anywhere">{message}</p> : null}
      {!tool.result ? (
        <p className="mt-2 flex items-center gap-2 text-xs text-fg-secondary">
          <ToolSpinner />
          {args.await_reply === true ? 'Sent; waiting for the answer…' : 'Sending…'}
        </p>
      ) : refused ? (
        <p className="mt-2 flex items-start gap-2 text-[13px] text-danger wrap-anywhere">
          <X className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          <span>{refused}</span>
        </p>
      ) : one ? (
        <>
          {one.track.tone !== 'danger' ? <TrackLine track={one.track} /> : null}
          <Note track={one.track} />
          {one.reply ? (
            <div className="mt-2.5 border-t border-dashed border-border-subtle pt-2 text-[13px]">
              <div className="mb-0.5 flex flex-wrap items-center gap-x-1 text-xs text-fg-secondary">
                Answer
                {one.answerAt !== undefined ? <span>· {formatAt(one.answerAt)}</span> : null}
                {one.target ? (
                  <button type="button" className="text-fg-accent hover:underline pointer-coarse:min-h-11" onClick={() => jump(one.target as string)}>
                    · jump to message ↓
                  </button>
                ) : null}
              </div>
              {/* An outside agent's words stay plain text, as on its own card. */}
              {addresses.some(isOutside) ? (
                <p className="line-clamp-3 whitespace-pre-wrap wrap-anywhere">{one.reply}</p>
              ) : (
                <Streamdown mode="static" className="max-h-[4.5rem] overflow-hidden text-[13px] [&_p]:my-0">
                  {one.reply}
                </Streamdown>
              )}
            </div>
          ) : null}
        </>
      ) : (
        <>
          <div className="mt-2 flex flex-wrap gap-1.5">
            {rows.map((r) => (
              <Chip key={`${r.id}-${r.name}`} tone={r.track.tone} title={r.track.note}>
                {r.name} · {r.track.word}
              </Chip>
            ))}
          </div>
          {rows.map((r) => (r.track.tone === 'danger' || r.track.tone === 'warning' ? <Note key={`${r.id}-${r.name}-note`} track={r.track} who={r.name} /> : null))}
        </>
      )}
    </div>
  )
}
