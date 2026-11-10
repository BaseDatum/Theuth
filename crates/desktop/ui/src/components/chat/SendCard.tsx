import { CircleHelp, Hourglass, X } from 'lucide-react'
import { useQueries } from '@tanstack/react-query'
import { Fragment, type MouseEvent, useEffect, useMemo, useRef } from 'react'
import { toast } from 'sonner'
import { Streamdown } from 'streamdown'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'
import { formatAt } from '@/lib/time'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import { type Entry, useSessions } from '@/store/sessions'
import { ToolSpinner } from './Indicators'
import { Chip, Clock, useOverflow } from './MessagePeer'
import { useScopedDisclosure } from './disclosure'
import { NIL, type SentReceipt, type Track, isOutside, isTerminal, parseReceipts, recipientLabel, trackOf, useDirectory } from './peer-identity'

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

/** `held` (warning), `failed`/`suppressed` (danger) and `unknown` (muted) say why, from the receipt's `error`; `lead`
 * names whom it is about (`ivan: `, or `13 not delivered · ` for a collapsed group), `title` lists them on hover. */
function Note({ track, lead, title }: { track: Track; lead?: string; title?: string }) {
  if (!track.note && track.tone !== 'muted') return null
  const text = track.note ?? 'Outcome unknown.'
  return (
    <p title={title} className={cn('mt-2 flex items-start gap-2 text-[13px] wrap-anywhere', NOTE[track.tone])}>
      {track.tone === 'warning' ? <Hourglass className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" /> : track.tone === 'danger' ? <X className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" /> : <CircleHelp className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />}
      <span>
        {lead ? <b className="font-semibold">{lead}</b> : null}
        {track.tone === 'muted' && track.note ? `Outcome unknown: ${track.note}` : text}
      </span>
    </p>
  )
}

/** Chips shown before `+N more`. */
const CHIPS = 6

/**
 * The incoming message answering send `id` (its `reply_to`). The card selects this one entry, not `entries`: the reducer
 * replaces the array on every TextDelta but keeps every untouched entry object, so the selected value stays `===` and
 * the card does not re-render while a reply streams (`PeerCards.test.ts` pins the identity).
 */
export function answerFor(entries: readonly Entry[] | undefined, id: string | undefined): Peer | undefined {
  if (id === undefined) return undefined
  return entries?.find((e): e is Peer => e.kind === 'peer' && e.origin?.reply_to === id)
}

const GIVE_UP = "The answer isn't loaded in this view"
/** The reader's own input on the scroller ends a jump: the position is theirs again. */
const INPUT = ['wheel', 'touchstart', 'keydown', 'pointerdown'] as const

/** The one running jump: a new click, the reader's input, or the owning card's unmount cancels it. */
let jumping: { owner: object; stop: () => void } | null = null

/** What the jump loop drives: the transcript scroller (an element in the app, a stand-in in tests). */
export type Scroller = Pick<HTMLElement, 'scrollTop' | 'clientHeight' | 'scrollHeight' | 'addEventListener' | 'removeEventListener'>

/**
 * Step `scroller` down until `find()` brings the card into view: the deep-link retry of `routes/c.$session.tsx`
 * `scrollToCell` for a virtualised transcript, where the card mounts only once its row is in range. Only the reader's
 * input (wheel, touch, a key, a pointer press), the owner's unmount or a new jump cancels it: the virtualiser rewrites
 * `scrollTop` itself as rows measure, so a moved `scrollTop` is no sign of the reader. Reaching the bottom or 10 s
 * without the card gives up with a toast.
 */
export function stepTo(scroller: Scroller, find: () => boolean, owner: object): void {
  jumping?.stop()
  const stop = (gaveUp: boolean) => {
    clearInterval(timer)
    clearTimeout(deadline)
    for (const type of INPUT) scroller.removeEventListener(type, cancel)
    if (jumping?.stop === cancel) jumping = null
    if (gaveUp) toast.message(GIVE_UP)
  }
  const cancel = () => stop(false)
  for (const type of INPUT) scroller.addEventListener(type, cancel, { passive: true })
  // ponytail: steps downward only; an answer always lands after its send.
  const timer = setInterval(() => {
    if (find()) return stop(false)
    if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 1) return stop(true)
    scroller.scrollTop += scroller.clientHeight * 0.8
  }, 150)
  const deadline = setTimeout(() => stop(true), 10_000)
  jumping = { owner, stop: cancel }
}

/** Jump to the answer's card: open its group line in case it is folded, then scroll to it (`stepTo` while unmounted). */
function jump(e: MouseEvent<HTMLElement>, owner: object, entry: string, exchange: string) {
  jumping?.stop()
  useToolRuns.getState().setDisclosed(disclosureKey(entry, 'line'), true)
  const find = () => {
    const el = document.getElementById(`peer-msg-${exchange}`)
    el?.scrollIntoView({ block: 'center', behavior: 'smooth' })
    return !!el
  }
  if (find()) return
  const scroller = e.currentTarget.closest<HTMLElement>('.overflow-y-auto')
  if (scroller) stepTo(scroller, find, owner)
  else toast.message(GIVE_UP)
}

/** `Show all` / `Show less` under a clamped block; its open state is a scoped disclosure (171F B13). */
function More({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  return (
    <Button type="button" variant="ghost" size="xs" className={cn('-ml-2 mt-0.5 text-fg-secondary', TOUCH)} aria-expanded={open} onClick={onToggle}>
      {open ? 'Show less' : 'Show all'}
    </Button>
  )
}

/** One recipient's answer, clamped to about three lines with a measured `Show all`. An outside agent's words stay plain text. */
function Answer({ label, text, outside, part, at, onJump }: { label: string; text: string; outside: boolean; part: string; at?: number; onJump?: (e: MouseEvent<HTMLElement>) => void }) {
  const [open, setOpen] = useScopedDisclosure(part)
  const [ref, hidden] = useOverflow<HTMLDivElement>(!open)
  return (
    <div className="mt-2.5 border-t border-dashed border-border-subtle pt-2 text-[13px]">
      <div className="mb-0.5 flex flex-wrap items-center gap-x-1 text-xs text-fg-secondary">
        {label}
        {at !== undefined ? <span>· {formatAt(at)}</span> : null}
        {onJump ? (
          <button type="button" className={cn('text-fg-accent hover:underline', TOUCH)} onClick={onJump}>
            · jump to message ↓
          </button>
        ) : null}
      </div>
      <div ref={ref} className={cn(!open && 'max-h-[4.5rem] overflow-hidden')}>
        {outside ? (
          <p className="whitespace-pre-wrap wrap-anywhere">{text}</p>
        ) : (
          <Streamdown mode="static" className="text-[13px] [&_p]:my-0">
            {text}
          </Streamdown>
        )}
      </div>
      {hidden || open ? <More open={open} onToggle={() => setOpen((v) => !v)} /> : null}
    </div>
  )
}

/**
 * One `send_message` call (Sky, 2026-11-10): `You → <recipient>`, what was sent, and a live delivery track per
 * recipient. The tool text supplies the receipt ids and, as a fallback, the replies (`parseReceipts`, either engine's
 * format); each receipt's `peer_message(session, id)` record, refetched by `PeerMessageChanged` (`lib/rpc`), is the
 * truth for its state and reply. It is fetched while not terminal, and for every `replied` receipt of a fan-out. A
 * lone recipient's answer: `receipt.reply`, else the reply text, else the incoming message whose `reply_to` is this id,
 * with a jump to its card. Several recipients: one chip and one answer each. Details: what was sent and the raw output.
 */
export function SendCard({ tool, session, at }: { tool: Tool; session: string; at?: number }) {
  const api = useApi()
  const owner = useRef({}).current
  useEffect(
    () => () => {
      const j = jumping
      if (j?.owner === owner) j.stop()
    },
    [owner],
  )
  const args = (tool.args ?? {}) as { to?: unknown; message?: unknown }
  const message = typeof args.message === 'string' ? args.message : ''
  const to = [...new Set(typeof args.to === 'string' ? [args.to] : Array.isArray(args.to) ? args.to.filter((t): t is string => typeof t === 'string') : [])]
  const parsed = useMemo(() => (tool.result ? parseReceipts(tool.result.output, Math.max(1, to.length)) : { receipts: [], replies: {}, unexpected: false }), [tool.result, to.length])
  const { receipts, replies, unexpected } = parsed
  // Distinct receipts, not `to` entries: two spellings of one peer send once. An older engine's reply that cut a
  // fan-out short leaves recipients unread: the call's `to` count then.
  const count = receipts.length === 0 ? to.length : unexpected ? Math.max(receipts.length, to.length) : receipts.length
  const single = count === 1 && receipts.length === 1 ? receipts[0] : undefined
  // Display labels come from the call's `to` (receipts arrive in `to` order; a receipt's address is an engine detail
  // that may be a binding uuid). When two spellings sent once, positions no longer line up: the receipt's then.
  const addresses = receipts.length === 0 || receipts.length === to.length ? to : receipts.map((r) => r.to)
  const dir = useDirectory(session, addresses.some(isOutside))
  // One query per distinct minted id: a refusal's nil id has nothing to look up, and repeats share a key.
  const ids = [...new Set(receipts.filter((r) => r.id !== NIL).map((r) => r.id))]
  const results = useQueries({
    queries: ids.map((id) => {
      const r = receipts.find((x) => x.id === id) as SentReceipt
      return {
        queryKey: queryKeys.peerMessage(session, id),
        queryFn: () => api.peer_message(session, id),
        enabled: !isTerminal(r.state) || (!single && r.state === 'replied'),
      }
    }),
  })
  const answer = useSessions((s) => answerFor(s.sessions[session]?.entries, single?.id))
  const [fullText, setFullText] = useScopedDisclosure('send-text')
  const [textRef, textHidden] = useOverflow<HTMLParagraphElement>(!fullText)
  const [details, setDetails] = useScopedDisclosure('send-details')
  const [allChips, setAllChips] = useScopedDisclosure('send-chips')
  const outside = addresses.some(isOutside)
  const rows = receipts.map((r, i) => {
    const rec = results[ids.indexOf(r.id)]?.data?.receipt
    // A string, so `held` reads the same before and after the bindings name it.
    const state: string = rec?.state ?? r.state
    // A block an older engine's reply cut short: its trailing text mixes recipients, so only records answer.
    return { id: r.id, name: recipientLabel(addresses[i] ?? r.to, dir), track: trackOf(state, rec ? rec.error : r.error), reply: rec?.reply ?? (unexpected ? undefined : replies[r.id]) }
  })
  // Identical problem lines (same tone, same words) collapse into one: `13 not delivered · unknown message`.
  const problems: { track: Track; names: string[] }[] = []
  for (const r of rows) {
    if (r.track.tone !== 'danger' && r.track.tone !== 'warning') continue
    const same = problems.find((p) => p.track.tone === r.track.tone && p.track.word === r.track.word && p.track.note === r.track.note)
    if (same) same.names.push(r.name)
    else problems.push({ track: r.track, names: [r.name] })
  }
  const one = single ? rows[0] : undefined
  const reply = one ? (one.reply ?? answer?.text) : undefined
  const name = count > 1 ? `${count} recipients` : addresses[0] ? recipientLabel(addresses[0], dir) : '?'
  // Refused before any receipt (communication off, a bad `to`): the output is the reason.
  const refused = tool.result && receipts.length === 0 ? tool.result.output : undefined
  const tone = refused ? 'danger' : one?.track.tone
  const target = answer?.origin?.id
  const chips = allChips ? rows : rows.slice(0, CHIPS)
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
          <p ref={textRef} className={cn('mt-1.5 text-[13.5px] whitespace-pre-wrap text-fg-secondary wrap-anywhere', !fullText && 'line-clamp-2')}>
            {message}
          </p>
          {textHidden || fullText ? <More open={fullText} onToggle={() => setFullText((v) => !v)} /> : null}
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
            <Answer
              label="Answer"
              text={reply}
              outside={outside}
              part="send-answer"
              at={answer?.at}
              onJump={answer && target ? (e) => jump(e, owner, answer.id, target) : undefined}
            />
          ) : null}
        </>
      ) : (
        <>
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            {chips.map((r) => (
              <Chip key={`${r.id}-${r.name}`} tone={r.track.tone} title={r.track.note}>
                {r.name} · {r.track.word}
              </Chip>
            ))}
            {rows.length > CHIPS ? (
              <Button type="button" variant="ghost" size="xs" className={cn('h-5 px-1.5 text-[11.5px] text-fg-secondary', TOUCH)} aria-expanded={allChips} onClick={() => setAllChips((v) => !v)}>
                {allChips ? 'Show fewer' : `+${rows.length - CHIPS} more`}
              </Button>
            ) : null}
          </div>
          {problems.map((p) => (
            <Note
              key={`${p.track.word}-${p.track.note}`}
              track={p.track}
              lead={p.names.length === 1 ? `${p.names[0]}: ` : `${p.names.length} ${p.track.word} · `}
              title={p.names.length > 1 ? p.names.join(', ') : undefined}
            />
          ))}
          {rows.map((r) => (r.reply ? <Answer key={`${r.id}-${r.name}-answer`} label={`Answer from ${r.name}`} text={r.reply} outside={outside} part={`send-answer-${r.id}`} /> : null))}
        </>
      )}
      {unexpected ? <p className="mt-2 text-xs text-fg-secondary">Some recipients' outcomes could not be read here: see Details.</p> : null}
      {tool.result ? (
        <div className="mt-1 flex justify-end">
          <Button type="button" variant="ghost" size="xs" className={TOUCH} aria-expanded={details} onClick={() => setDetails((v) => !v)}>
            {details ? 'Hide details' : 'Details'}
          </Button>
        </div>
      ) : null}
      {details && tool.result ? (
        <div className="mt-1 flex flex-col gap-1 text-xs text-fg-secondary">
          <span>Sent</span>
          <pre className="max-h-72 overflow-auto rounded-lg bg-layer-2 p-3 font-mono leading-5 wrap-anywhere whitespace-pre-wrap text-foreground">{message}</pre>
          {rows.length > 1 ? (
            <>
              <span>Recipients</span>
              <ul className="flex flex-col gap-0.5 rounded-lg bg-layer-2 p-3 text-foreground">
                {rows.map((r) => (
                  <li key={`${r.id}-${r.name}`} className="wrap-anywhere">
                    <b className="font-semibold">{r.name}</b> · {r.track.word}
                    {r.track.note ? <span className="text-fg-secondary"> · {r.track.note}</span> : null}
                  </li>
                ))}
              </ul>
            </>
          ) : null}
          <span>Output</span>
          <pre className="max-h-72 overflow-auto rounded-lg bg-layer-2 p-3 font-mono leading-5 wrap-anywhere whitespace-pre-wrap text-foreground">{tool.result.output}</pre>
        </div>
      ) : null}
    </div>
  )
}
