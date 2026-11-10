import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Copy, ShieldAlert, SquareTerminal, Timer } from 'lucide-react'
import { memo, type ReactNode, type RefObject, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import { Streamdown } from 'streamdown'
import { create } from 'zustand'
import { errorText, useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { routed } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import { formatAt, fullAt } from '@/lib/time'
import { cn } from '@/lib/utils'
import type { ContactNote } from '@/lib/injected'
import type { PeerOrigin } from '@/rpc/bindings/PeerOrigin'
import type { PeerRecord } from '@/rpc/bindings/PeerRecord'
import { useSessionField } from '@/store/sessions'
import { useScopedDisclosure } from './disclosure'
import { type Sender, type Track, firstLine, initials, isAddress, isOutside, resolveSender, senderColor, useDirectory } from './peer-identity'

const TOUCH = 'pointer-coarse:min-h-11'

/** Status chip colours: the harness's success, info, warning and danger tokens. */
const TONE: Record<Track['tone'], string> = {
  ok: 'border-success-border bg-success-bg text-success',
  info: 'border-info-border bg-info-bg text-info',
  warning: 'border-warning-border bg-warning-bg text-warning',
  danger: 'border-danger-border bg-danger-bg text-danger',
  muted: 'border-border-subtle bg-layer-2 text-fg-secondary',
}

export function Chip({ tone, children, title }: { tone: Track['tone']; children: ReactNode; title?: string }) {
  return (
    <span title={title} className={cn('inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2 py-px text-[11.5px] whitespace-nowrap', TONE[tone])}>
      <span className="size-1.5 rounded-full bg-current" aria-hidden="true" />
      {children}
    </span>
  )
}

/** The sender's circle: initials over one stable project-palette colour; a job or a reminder gets one fixed neutral
 * circle with its icon instead, the same for every job (a hashed mineral per label would be noise). */
export function SenderCircle({ sender, className }: { sender: Sender; className?: string }) {
  if (sender.kind) {
    const Icon = sender.kind === 'job' ? SquareTerminal : Timer
    return (
      <span aria-hidden="true" className={cn('grid size-7 shrink-0 place-items-center rounded-full bg-layer-3 text-fg-secondary', className)}>
        <Icon className="size-3.5" />
      </span>
    )
  }
  return (
    <span aria-hidden="true" className={cn('grid size-7 shrink-0 place-items-center rounded-full text-xs font-bold text-primary-foreground', senderColor(sender.key), className)}>
      {initials(sender.name)}
    </span>
  )
}

/** The circle, the name (with `count`, a folded group's size, never truncated) and the subtitle with its muted `tag`
 * (a contact's cap); `children` sit on the right (chips, the time). */
export function SenderHead({ sender, children, count, tag }: { sender: Sender; children?: ReactNode; count?: string; tag?: string }) {
  return (
    <div className="flex min-w-0 items-center gap-2.5">
      <SenderCircle sender={sender} />
      <div className="flex min-w-0 flex-col leading-tight">
        <span className="flex min-w-0 items-center gap-1.5 font-semibold">
          <span className="truncate">{sender.name}</span>
          {count ? <span className="shrink-0 text-xs font-normal whitespace-nowrap text-fg-tertiary tabular-nums">{count}</span> : null}
        </span>
        {sender.subtitle || tag ? (
          <span className="flex min-w-0 items-center gap-1.5 text-xs text-fg-secondary">
            {sender.subtitle ? <span className="truncate">{sender.subtitle}</span> : null}
            {tag ? <span className="shrink-0 rounded bg-layer-2 px-1.5 text-[11px] whitespace-nowrap text-fg-tertiary">{tag}</span> : null}
          </span>
        ) : null}
      </div>
      <span className="flex-1" />
      {children}
    </div>
  )
}

export function Clock({ at }: { at?: number }) {
  if (at === undefined) return null
  return (
    <span title={fullAt(at)} className="shrink-0 text-xs text-fg-tertiary tabular-nums">
      {formatAt(at)}
    </span>
  )
}

/** One participant's durable record of exchange `id`; nothing is fetched while `enabled` is false. */
function useRecord(session: string, id: string | null | undefined, enabled = true) {
  const api = useApi()
  return useQuery({
    queryKey: queryKeys.peerMessage(session, id ?? ''),
    queryFn: () => api.peer_message(session, id ?? ''),
    enabled: enabled && !!id,
  })
}

/** The exchange ids this session still owes an answer to (`list_peer_messages(pending_only)`); one fetch per session. */
export function useAwaitingAnswer(session: string) {
  const api = useApi()
  return useQuery({
    queryKey: queryKeys.peerMessages(session, true, 'page'),
    queryFn: () => api.list_peer_messages(session, null, null, true),
    select: (page) => page.awaiting_answer,
  })
}

function Row({ label, value }: { label: string; value: string | number | null | undefined }) {
  if (value === null || value === undefined || value === '') return null
  return (
    <>
      <dt className="text-fg-secondary">{label}</dt>
      <dd className="min-w-0 select-all font-mono wrap-anywhere">{String(value)}</dd>
    </>
  )
}

/** Who produced a reply, from the durable receipt (50 §5.3): `owner:<session>` means the human answered for that
 * session and no peer identity was forged; anything else is the replying endpoint. */
function answeredBy(r: PeerRecord | undefined): string | undefined {
  const by = r?.receipt.answered_by
  if (!by) return undefined
  return by.startsWith('owner:') ? `owner of session:${by.slice('owner:'.length)} (not the peer)` : by
}

/**
 * Typed provenance of a peer card (50 §9): the exchange id, sender, reply target and A2A scope the engine
 * authenticated, and the durable record. Every fact here is a `PeerOrigin` field or a `peer_message` reply, never
 * parsed from the body.
 */
function Provenance({ origin, seq, session, request, contact }: { origin: PeerOrigin; seq?: number; session: string; request: PeerRecord | undefined; contact?: PeerContact }) {
  const record = useRecord(session, origin.id)
  const rec = record.data
  return (
    <div id={`peer-record-${origin.id}`} className="mx-4 mb-3 rounded-lg bg-layer-2 p-3 text-xs">
      <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
        <Row label="from" value={origin.from} />
        <dt className="text-fg-secondary">message</dt>
        <dd className="flex min-w-0 items-center gap-1">
          <span className="min-w-0 select-all font-mono wrap-anywhere">{origin.id}</span>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label="Copy exchange id"
            onClick={() => navigator.clipboard.writeText(origin.id).then(() => toast.message('Copied'), (e: unknown) => toast.error(errorText(e)))}
          >
            <Copy />
          </Button>
        </dd>
        <Row label="reply to" value={origin.reply_to} />
        <Row label="scope" value={origin.scope} />
        <Row label="row" value={seq} />
        <Row label="to" value={rec?.receipt.to} />
        <Row label="state" value={rec?.receipt.state} />
        <Row label="answered by" value={answeredBy(rec)} />
        <Row label="task" value={rec?.receipt.task_id} />
        <Row label="error" value={rec?.receipt.error} />
        <Row label="accepted" value={rec?.created_at} />
        {origin.reply_to ? <Row label="request answered by" value={answeredBy(request)} /> : null}
      </dl>
      {contact ? (
        <>
          <p className="mt-3 mb-1 font-medium text-fg-secondary">What the agent was told</p>
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
            <Row label="Contact" value={contact.contact} />
            <Row label="Machine" value={contact.machine} />
            <Row label="Shared chat" value={contact.export} />
            <Row label="Access" value={capLabel(contact.cap)} />
            {contact.note ? (
              <>
                <dt className="text-fg-secondary">Note to the agent</dt>
                <dd className="min-w-0 wrap-anywhere">{contact.note}</dd>
              </>
            ) : null}
          </dl>
        </>
      ) : null}
      {record.isPending ? <p className="mt-2 text-fg-secondary">Loading record…</p> : null}
      {record.error ? (
        <p role="alert" className="mt-2 text-danger">
          {errorText(record.error)}
        </p>
      ) : null}
      {/* 188: the chat's Messaging… sheet, focused on this request; a link, so it works from any transcript. */}
      <Button asChild variant="link" size="xs" className={cn('mt-1 px-0', TOUCH)}>
        <Link to="/c/$session" params={{ session }} search={{ messaging: '1', peer: origin.id }}>
          Open in Messaging
        </Link>
      </Button>
    </div>
  )
}

/** Answer drafts by `session|exchange id`, in memory: every mount of the card shares one, so folding a group or the
 * virtualiser dropping the row never loses what the owner typed (171F B13: transcript row state lives in a store). */
export const useAnswerDrafts = create<Record<string, string>>()(() => ({}))

/**
 * Whether the clamped element hides content (`scrollHeight > clientHeight`), re-measured on every resize, so a phone
 * width always gets the expander. Measured only while `clamped`: once opened, the expander stays to close it again.
 * False before layout (and on the server): there is nothing to measure yet.
 */
export function useOverflow<T extends HTMLElement>(clamped: boolean): [RefObject<T | null>, boolean] {
  const ref = useRef<T>(null)
  const [hidden, setHidden] = useState(false)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || !clamped) return
    const measure = () => setHidden(el.scrollHeight > el.clientHeight + 1)
    measure()
    const watch = new ResizeObserver(measure)
    watch.observe(el)
    return () => watch.disconnect()
  }, [clamped])
  return [ref, hidden]
}

/** The owner answers a request this chat owes (`answer_peer_request`, as the chat's Messaging… sheet does). */
function AnswerForm({ session, id, onDone }: { session: string; id: string; onDone: () => void }) {
  const api = useApi()
  const key = `${session}|${id}`
  // The live state is the server snapshot too (as `useDisclosure`), so a preset draft shows on the first render.
  const read = () => useAnswerDrafts.getState()[key] ?? ''
  const text = useSyncExternalStore(useAnswerDrafts.subscribe, read, read)
  const setText = (t: string) => useAnswerDrafts.setState({ [key]: t })
  const send = useAction((t: string) => routed(session, () => api.answer_peer_request(session, id, t)), {
    keys: [['peer-messages', session], queryKeys.peerMessage(session, id)],
    onSuccess: (receipt) => {
      if (!receipt) return // parked behind the chat's OwnerOfflineCard: nothing was answered
      useAnswerDrafts.setState((s) => Object.fromEntries(Object.entries(s).filter(([k]) => k !== key)), true)
      onDone()
      toast(`Answered ${id.slice(0, 8)} · ${receipt.state}`)
    },
  })
  return (
    <form
      className="mx-4 mb-3 flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault()
        const t = text.trim()
        if (t) send.mutate(t)
      }}
    >
      <Textarea
        aria-label={`Answer request ${id} on behalf of this chat`}
        placeholder="Answer sent to the sender as a message from this chat, recorded as answered by you."
        value={text}
        disabled={send.isPending}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) e.currentTarget.form?.requestSubmit()
        }}
      />
      <div className="flex justify-end">
        <Button type="submit" size="sm" className={TOUCH} disabled={send.isPending || !text.trim()} aria-busy={send.isPending || undefined}>
          Send answer
        </Button>
      </div>
    </form>
  )
}

/** `remove`: `Message`'s remove-from-context control for a stored row; it sits in the header row, after the time, so it
 * never overlaps the outside band above. `contact`: the facts and harness note of the contact wrapper the row came in. */
type PeerContact = ContactNote & { note: string }
type Props = { from: string; text: string; origin?: PeerOrigin; seq?: number; session: string; at?: number; remove?: ReactNode; contact?: PeerContact }

/** A contact's cap in words: the header tag and Details › Access say it alike. */
export const capLabel = (cap: string) => (cap === 'read_only' ? 'read-only' : cap === 'mutating' ? 'can make changes' : cap)

/**
 * The card frame: sender header, the reply quote, the body (clamped), then the footer. `origin` parts (the answer
 * form, Details) come in through `chips`, `actions` and `after`. `full`: a contact's message under a full share (186,
 * the typed `PeerOrigin.contact_mode`), which reads like a mesh peer's.
 */
function PeerCard({
  text,
  at,
  remove,
  contact,
  sender,
  tone,
  id,
  chips,
  quote,
  actions,
  after,
  full,
}: Props & { sender: Sender; tone?: 'info'; id?: string; chips?: ReactNode; quote?: string; actions?: ReactNode; after?: ReactNode; full?: boolean }) {
  const [open, setOpen] = useScopedDisclosure('peer')
  // A job result is raw command output (14 §3): markdown would fold its lines into one paragraph (and make `omp 66726`
  // a heading). Live (`job j4`) and stored (`j4 · cmd · exit 0`) labels alike.
  const raw = sender.kind === 'job'
  // 183: an outside agent's words (a contact or an A2A peer: `contact:` / `peer:` senders) are untrusted: plain text,
  // never Markdown, so no remote image loads and no link is live. 186 D7: a full share's contact reads like the mesh.
  const outside = !full && (isOutside(sender.key) || !!contact)
  const [body, hidden] = useOverflow<HTMLDivElement>(!open)
  return (
    <div
      id={id}
      className={cn(
        // `data-jumped`: the brief highlight "jump to message" sets (`jump.ts` `settleOn`).
        'my-2 overflow-hidden rounded-xl border bg-layer-1 text-sm transition-shadow data-jumped:ring-2 data-jumped:ring-info-border data-jumped:ring-offset-2 data-jumped:ring-offset-background',
        outside ? 'border-warning-border' : tone === 'info' ? 'border-info-border' : 'border-border-subtle',
      )}
    >
      {outside ? (
        <div className="flex items-center gap-2 border-b border-warning-border bg-warning-bg px-4 py-1.5 text-xs text-warning">
          <ShieldAlert className="size-3.5 shrink-0" aria-hidden="true" />
          Outside your mesh · plain text, links and images are off
        </div>
      ) : null}
      <div className="px-4 pt-2.5">
        <SenderHead sender={sender} tag={contact ? capLabel(contact.cap) : full ? 'Contact · full' : undefined}>
          {chips}
          <Clock at={at} />
          {remove}
        </SenderHead>
      </div>
      {quote ? (
        <div className="mx-4 mt-2 truncate rounded-r-lg border-l-[3px] border-border-strong bg-layer-2 px-2.5 py-1.5 text-xs text-fg-secondary">
          ↳ Replying to <b className="font-semibold text-foreground">you</b>: {quote}
        </div>
      ) : null}
      <div
        ref={body}
        className={cn(
          'relative px-4 pt-2',
          !open && 'max-h-36 overflow-hidden',
          !open && hidden && "after:pointer-events-none after:absolute after:inset-x-0 after:bottom-0 after:h-9 after:bg-linear-to-b after:from-transparent after:to-layer-1 after:content-['']",
        )}
      >
        {raw ? (
          <pre className={cn('rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap', open && 'max-h-96 overflow-auto')}>{text}</pre>
        ) : outside ? (
          <div className="whitespace-pre-wrap wrap-anywhere">{text}</div>
        ) : (
          <Streamdown mode="static" className="text-sm">
            {text}
          </Streamdown>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2 px-4 pt-2 pb-2.5">
        {hidden || open ? (
          <Button type="button" variant="outline" size="xs" className={TOUCH} aria-expanded={open} onClick={() => setOpen((v) => !v)}>
            {open ? 'Show less' : 'Show all'}
          </Button>
        ) : null}
        {actions}
      </div>
      {after}
    </div>
  )
}

/** An engine-addressed card with typed provenance: awaiting chip, reply quote, answer form and Details. */
function OriginCard({ origin, session, seq, sender, ...rest }: Props & { origin: PeerOrigin; sender: Sender }) {
  const awaiting = useAwaitingAnswer(session).data?.includes(origin.id) ?? false
  // A reply's request is this session's own sent message: its text is the quote, its receipt says whether the owner answered.
  const request = useRecord(session, origin.reply_to).data
  const mine = request?.from === `session:${session}` ? request : undefined
  const quote = mine ? firstLine(mine.text) : undefined
  const owner = request?.receipt.answered_by?.startsWith('owner:') ? request.receipt.answered_by : undefined
  const [answering, setAnswering] = useScopedDisclosure('answer')
  const [details, setDetails] = useScopedDisclosure('provenance')
  return (
    <PeerCard
      {...rest}
      session={session}
      sender={sender}
      id={`peer-msg-${origin.id}`}
      tone={awaiting ? 'info' : undefined}
      full={origin.contact_mode === 'full'}
      quote={quote}
      chips={
        <>
          {owner ? (
            <Chip tone="muted" title={`answered by ${owner}`}>
              owner answer
            </Chip>
          ) : null}
          {awaiting ? <Chip tone="info">Expects a reply</Chip> : null}
        </>
      }
      actions={
        <>
          {awaiting ? (
            <Button type="button" size="xs" className={TOUCH} aria-expanded={answering} onClick={() => setAnswering((v) => !v)}>
              Answer as this chat
            </Button>
          ) : null}
          <OpenChat sender={sender} session={session} />
          <span className="flex-1" />
          <Button
            type="button"
            variant="ghost"
            size="xs"
            className={TOUCH}
            aria-expanded={details}
            aria-controls={`peer-record-${origin.id}`}
            onClick={() => setDetails((v) => !v)}
          >
            {details ? 'Hide details' : 'Details'}
          </Button>
        </>
      }
      after={
        <>
          {awaiting && answering ? <AnswerForm session={session} id={origin.id} onDone={() => setAnswering(false)} /> : null}
          {details ? <Provenance origin={origin} seq={seq} session={session} request={request} contact={rest.contact} /> : null}
        </>
      }
    />
  )
}

function OpenChat({ sender, session }: { sender: Sender; session: string }) {
  if (!sender.session || sender.session === session) return null
  return (
    <Button asChild variant="ghost" size="xs" className={TOUCH}>
      <Link to="/c/$session" params={{ session: sender.session }}>
        Open chat
      </Link>
    </Button>
  )
}

function Addressed(props: Props & { address: string }) {
  const sender = resolveSender(props.address, { ...useDirectory(props.session, isOutside(props.address) || !!props.contact), note: props.contact })
  return props.origin ? (
    <OriginCard {...props} origin={props.origin} sender={sender} />
  ) : (
    <PeerCard {...props} sender={sender} actions={<OpenChat sender={sender} session={props.session} />} />
  )
}

/** A label sender (`job j4`, `Reminder r3`, a legacy wrapper's name): no lookups beyond the session's own subtree. */
function Labelled(props: Props) {
  const children = useSessionField(props.session, 'children')
  const sender = resolveSender(props.from, { self: props.session, children, note: props.contact })
  return <PeerCard {...props} sender={sender} actions={<OpenChat sender={sender} session={props.session} />} />
}

/**
 * A peer message, subagent result or job result as a card: who sent it (name, circle, subtitle), what it answers, a
 * clamped body, then actions. A card with `origin` came from a typed peer exchange and carries its provenance and
 * controls; one parsed from a legacy wrapper (or a job/subagent/schedule note) is a label over untrusted text and gets
 * neither.
 */
export const MessagePeer = memo(function MessagePeer(props: Props) {
  const address = props.origin?.from ?? props.from
  return isAddress(address) ? <Addressed {...props} address={address} /> : <Labelled {...props} />
})
