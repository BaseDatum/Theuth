import { CircleHelp, Hourglass, X } from 'lucide-react'
import { useQueries } from '@tanstack/react-query'
import { Fragment, type MouseEvent, useMemo } from 'react'
import { Streamdown } from 'streamdown'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'
import { formatAt } from '@/lib/time'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import { type Entry, useSessions } from '@/store/sessions'
import { ToolSpinner } from './Indicators'
import { Chip, Clock } from './MessagePeer'
import { useScopedDisclosure } from './disclosure'
import { NIL, type Track, isOutside, isTerminal, parseReceipts, resolveSender, trackOf, useDirectory } from './peer-identity'

type Tool = Extract<Entry, { kind: 'tool' }>
type Peer = Extract<Entry, { kind: 'peer' }>

const TOUCH = 'pointer-coarse:min-h-11'

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

/**
 * The incoming message answering send `id` (its `reply_to`). The card selects this one entry, not `entries`: the reducer
 * replaces the array on every TextDelta but keeps every untouched entry object, so the selected value stays `===` and
 * the card does not re-render while a reply streams (`PeerCards.test.ts` pins the identity).
 */
export function answerFor(entries: readonly Entry[] | undefined, id: string | undefined): Peer | undefined {
  if (id === undefined) return undefined
  return entries?.find((e): e is Peer => e.kind === 'peer' && e.origin?.reply_to === id)
}

/**
 * Bring the answer's card into view through the virtualised transcript: open its group line in case it is folded,
 * then step the scroller down until the card mounts (the deep-link retry of `routes/c.$session.tsx` `scrollToCell`),
 * for at most 10 s. The button exists only while the answer entry does.
 */
function jump(e: MouseEvent<HTMLElement>, entry: string, exchange: string) {
  useToolRuns.getState().setDisclosed(disclosureKey(entry, 'line'), true)
  const scroller = e.currentTarget.closest<HTMLElement>('.overflow-y-auto')
  const find = () => {
    const el = document.getElementById(`peer-msg-${exchange}`)
    el?.scrollIntoView({ block: 'center', behavior: 'smooth' })
    return !!el
  }
  if (find()) return
  // ponytail: steps downward only; an answer always lands after its send.
  const timer = setInterval(() => {
    if (find() || !scroller || scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 1) stop()
    else scroller.scrollBy({ top: scroller.clientHeight * 0.8 })
  }, 150)
  const deadline = setTimeout(() => stop(), 10_000)
  const stop = () => {
    clearInterval(timer)
    clearTimeout(deadline)
  }
}

/** `Show all` / `Show less` under a clamped block; its open state is a scoped disclosure (171F B13). */
function More({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  return (
    <Button type="button" variant="ghost" size="xs" className={cn('-ml-2 mt-0.5 text-fg-secondary', TOUCH)} aria-expanded={open} onClick={onToggle}>
      {open ? 'Show less' : 'Show all'}
    </Button>
  )
}

/**
 * One `send_message` call (Sky, 2026-11-10): `You → <recipient>`, what was sent, and a live delivery track per
 * recipient. The tool text supplies only the receipt ids (`parseReceipts`, the leading receipt block); each receipt's
 * `peer_message(session, id)` record, refetched by `PeerMessageChanged` (`lib/rpc`) while it is not terminal, is the
 * truth for its state and reply. Answered: `receipt.reply`, else the incoming message whose `reply_to` is this id,
 * with a jump to its card. Several recipients: one chip each. Details shows the raw tool output.
 */
export function SendCard({ tool, session, at }: { tool: Tool; session: string; at?: number }) {
  const api = useApi()
  const args = (tool.args ?? {}) as { to?: unknown; message?: unknown }
  const message = typeof args.message === 'string' ? args.message : ''
  const to = typeof args.to === 'string' ? [args.to] : Array.isArray(args.to) ? args.to.filter((t): t is string => typeof t === 'string') : []
  const { receipts, rest } = useMemo(() => (tool.result ? parseReceipts(tool.result.output, Math.max(1, to.length)) : { receipts: [], rest: '' }), [tool.result, to.length])
  const addresses = receipts.length ? receipts.map((r) => r.to) : to
  const dir = useDirectory(session, addresses.some(isOutside))
  const records = useQueries({
    queries: receipts.map((r) => ({
      queryKey: queryKeys.peerMessage(session, r.id),
      queryFn: () => api.peer_message(session, r.id),
      enabled: r.id !== NIL && !isTerminal(r.state),
    })),
  })
  // One recipient: the full track and the answer. A block cut short by a reply (several awaited recipients) is not one.
  const single = receipts.length === 1 && to.length <= 1 ? receipts[0] : undefined
  const answer = useSessions((s) => answerFor(s.sessions[session]?.entries, single?.id))
  const [fullText, setFullText] = useScopedDisclosure('send-text')
  const [fullReply, setFullReply] = useScopedDisclosure('send-answer')
  const [details, setDetails] = useScopedDisclosure('send-details')
  const rows = receipts.map((r, i) => {
    const rec = records[i]?.data?.receipt
    // A string, so `held` reads the same before and after the bindings name it.
    const state: string = rec?.state ?? r.state
    return { id: r.id, name: resolveSender(r.to, dir).name, track: trackOf(state, rec ? rec.error : r.error) }
  })
  const one = single ? rows[0] : undefined
  const rec = records[0]?.data?.receipt
  // The text after a lone receipt is its awaited reply, shown as text and never parsed; the record's reply wins.
  const reply = single ? (rec?.reply ?? (rest.trim() || undefined) ?? answer?.text) : undefined
  // An awaited reply ended the receipt block before every recipient was read: their outcomes are only in Details.
  const unread = receipts.length > 0 && receipts.length < to.length && rest.trim() !== ''
  // `peer:<alias>/<export>`: the export is part of who it went to (`sky · hi-there`).
  const exp = addresses.length === 1 && addresses[0]?.startsWith('peer:') ? addresses[0].split('/')[1] : undefined
  const count = Math.max(addresses.length, to.length)
  const name = count > 1 ? `${count} recipients` : addresses[0] ? `${resolveSender(addresses[0], dir).name}${exp ? ` · ${exp}` : ''}` : '?'
  // Refused before any receipt (communication off, a bad `to`): the output is the reason.
  const refused = tool.result && receipts.length === 0 ? tool.result.output : undefined
  const tone = refused ? 'danger' : one?.track.tone
  const target = answer?.origin?.id
  const longText = message.split('\n').length > 2 || message.length > 180
  const longReply = !!reply && (reply.split('\n').length > 3 || reply.length > 280)
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
      {message ? (
        <>
          <p className={cn('mt-1.5 text-[13.5px] whitespace-pre-wrap text-fg-secondary wrap-anywhere', !fullText && 'line-clamp-2')}>{message}</p>
          {longText ? <More open={fullText} onToggle={() => setFullText((v) => !v)} /> : null}
        </>
      ) : null}
      {!tool.result ? (
        tool.interrupted ? (
          <div className="mt-2 text-xs text-fg-tertiary">Interrupted by restart</div>
        ) : (
          <div className="mt-2 flex items-center gap-2 text-xs text-fg-secondary">
            <ToolSpinner />
            Sending…
          </div>
        )
      ) : refused ? (
        <p className="mt-2 flex items-start gap-2 text-[13px] text-danger wrap-anywhere">
          <X className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          <span>{refused}</span>
        </p>
      ) : one ? (
        <>
          {one.track.tone !== 'danger' ? <TrackLine track={one.track} /> : null}
          <Note track={one.track} />
          {reply ? (
            <div className="mt-2.5 border-t border-dashed border-border-subtle pt-2 text-[13px]">
              <div className="mb-0.5 flex flex-wrap items-center gap-x-1 text-xs text-fg-secondary">
                Answer
                {answer?.at !== undefined ? <span>· {formatAt(answer.at)}</span> : null}
                {answer && target ? (
                  <button type="button" className={cn('text-fg-accent hover:underline', TOUCH)} onClick={(e) => jump(e, answer.id, target)}>
                    · jump to message ↓
                  </button>
                ) : null}
              </div>
              {/* An outside agent's words stay plain text, as on its own card. */}
              {addresses.some(isOutside) ? (
                <p className={cn('whitespace-pre-wrap wrap-anywhere', !fullReply && 'line-clamp-3')}>{reply}</p>
              ) : (
                <Streamdown mode="static" className={cn('text-[13px]', !fullReply && 'max-h-[4.5rem] overflow-hidden [&_p]:my-0')}>
                  {reply}
                </Streamdown>
              )}
              {longReply ? <More open={fullReply} onToggle={() => setFullReply((v) => !v)} /> : null}
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
      {unread ? <p className="mt-2 text-xs text-fg-secondary">The other recipients' outcomes follow the first answer: see Details.</p> : null}
      {tool.result ? (
        <div className="mt-1 flex justify-end">
          <Button type="button" variant="ghost" size="xs" className={TOUCH} aria-expanded={details} onClick={() => setDetails((v) => !v)}>
            {details ? 'Hide details' : 'Details'}
          </Button>
        </div>
      ) : null}
      {details && tool.result ? (
        <pre className="mt-1 max-h-72 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">{tool.result.output}</pre>
      ) : null}
    </div>
  )
}
