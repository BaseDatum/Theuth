import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { ChevronRight, Info } from 'lucide-react'
import { memo, useState } from 'react'
import { Streamdown } from 'streamdown'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { queryKeys, useApi } from '@/lib/rpc'
import type { PeerOrigin } from '@/rpc/bindings/PeerOrigin'
import type { PeerRecord } from '@/rpc/bindings/PeerRecord'

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
function useAwaitingAnswer(session: string) {
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
 * Typed provenance of a peer card (50 §9): the exchange id, sender, reply target and A2A scope the harness
 * authenticated, the durable record on demand, and the answer affordance for a request this session still owes
 * a reply to. Every fact here is a `PeerOrigin` field or a `peer_message`/`list_peer_messages` reply, never
 * parsed from the body.
 */
function Provenance({ origin, seq, session }: { origin: PeerOrigin; seq?: number; session: string }) {
  const [open, setOpen] = useState(false)
  const awaiting = useAwaitingAnswer(session).data?.includes(origin.id) ?? false
  const record = useRecord(session, origin.id, open)
  // A reply's request is this session's own sent message; its receipt says whether the owner, not the peer, answered.
  const request = useRecord(session, origin.reply_to)
  const owner = request.data?.receipt.answered_by?.startsWith('owner:') ? request.data.receipt.answered_by : undefined
  const rec = record.data
  return (
    <div className="mt-2 flex flex-col gap-2 text-xs">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="ghost"
          size="xs"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          aria-controls={`peer-record-${origin.id}`}
          className="pointer-coarse:min-h-11"
        >
          <Info /> {open ? 'Hide record' : 'Inspect record'}
        </Button>
        <span className="select-all font-mono text-fg-secondary" title="exchange id">
          {origin.id}
        </span>
        {owner !== undefined ? (
          <Badge variant="outline" title={`answered by ${owner}`}>
            owner answer
          </Badge>
        ) : null}
        {awaiting ? (
          <>
            <Badge variant="secondary">awaiting your answer</Badge>
            <Button asChild variant="outline" size="xs" className="pointer-coarse:min-h-11">
              <Link to="/settings/$tab" params={{ tab: 'communication' }} search={{ session, peer: origin.id }}>
                Answer as owner
              </Link>
            </Button>
          </>
        ) : null}
      </div>
      {open ? (
        <div id={`peer-record-${origin.id}`} className="rounded-lg bg-layer-2 p-3">
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
            <Row label="from" value={origin.from} />
            <Row label="id" value={origin.id} />
            <Row label="reply to" value={origin.reply_to} />
            <Row label="scope" value={origin.scope} />
            <Row label="row" value={seq} />
            <Row label="to" value={rec?.receipt.to} />
            <Row label="state" value={rec?.receipt.state} />
            <Row label="answered by" value={answeredBy(rec)} />
            <Row label="task" value={rec?.receipt.task_id} />
            <Row label="error" value={rec?.receipt.error} />
            <Row label="accepted" value={rec?.created_at} />
            {origin.reply_to ? <Row label="request answered by" value={answeredBy(request.data)} /> : null}
          </dl>
          {record.isPending ? <p className="mt-2 text-fg-secondary">Loading record…</p> : null}
          {record.error ? (
            <p role="alert" className="mt-2 text-destructive">
              {record.error instanceof Error ? record.error.message : String(record.error)}
            </p>
          ) : null}
          <Button asChild variant="link" size="xs" className="mt-1 px-0">
            <Link to="/settings/$tab" params={{ tab: 'communication' }} search={{ session, peer: origin.id }}>
              Open in Communication
            </Link>
          </Button>
        </div>
      ) : null}
    </div>
  )
}

/**
 * `⟵ from` card for a peer message, subagent result or job result; the first line until expanded. A card with
 * `origin` came from a typed peer exchange and carries its provenance and controls; one parsed from a legacy
 * wrapper (or a job/subagent/schedule note) is a label over untrusted text and gets neither.
 */
export const MessagePeer = memo(function MessagePeer({
  from,
  text,
  origin,
  seq,
  session,
}: {
  from: string
  text: string
  origin?: PeerOrigin
  seq?: number
  session: string
}) {
  const [open, setOpen] = useState(false)
  const first = text.split('\n', 1)[0] ?? ''
  const more = text.length > first.length
  // A job result is raw command output (14 §3): markdown would fold its lines into one paragraph.
  const raw = from.startsWith('job ')
  return (
    <div className="my-2 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
      <button
        type="button"
        className="group flex w-full items-center gap-2 text-left font-medium text-fg-accent"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <ChevronRight className={more ? `size-4 transition-transform ${open ? 'rotate-90' : ''}` : 'invisible size-4'} />
        <span className="truncate">⟵ {from}</span>
      </button>
      {open ? (
        raw ? (
          <pre className="mt-2 max-h-56 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">
            {text}
          </pre>
        ) : (
          <Streamdown mode="static" className="mt-2 text-sm">
            {text}
          </Streamdown>
        )
      ) : (
        <div className={`mt-1 truncate text-fg-secondary ${raw ? 'font-mono text-xs' : ''}`}>{first}</div>
      )}
      {origin ? <Provenance origin={origin} seq={seq} session={session} /> : null}
    </div>
  )
})
