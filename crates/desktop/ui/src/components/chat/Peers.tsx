import { useQuerySupport } from '@/rpc/capabilities'
import { useInfiniteQuery, useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Copy, RefreshCw } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { relative } from '@/components/routines/helpers'
import { Empty, Row, Section, errorText, useAction } from '@/components/settings/shared'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { routed } from '@/lib/owner'
import { queryKeys, useApi } from '@/lib/rpc'
import type { PeerInfo, PeerRecord } from "@/rpc/bindings/method.agent.output"
import { AttachFiles, answerPeer } from './ContactFiles'
import { isOutside } from './peer-identity'

const TOUCH = 'pointer-coarse:min-h-11'

/**
 * 50 §5.1/§5.3 for the open chat: the permitted directory `list_peers` returns
 * (the same filter the model sees: name, kind, position, liveness, address —
 * never another root's title or cwd) and the requests a human may resolve by
 * exact id, each with inspect and answer controls bound to that id.
 */
export function Peers({ session, focus }: { session: string; focus: string | null }) {
  return (
    <>
      <Directory session={session} />
      <Requests session={session} focus={focus} />
    </>
  )
}

/** 200 (Contract): the line under a page whose contact machines did not all answer in time (the model reads the same). */
export const partialLine = (partial: readonly string[]) =>
  `partial: no answer in time from ${partial.join(', ')}; their sessions may be missing from this page (repeat this call to retry)`

/** A contact's own `peer:<contact>` row (not one of its shares or sessions): its title is the engine's hint
 * (`full access · more: …`, `+N more: …`, `did not answer: …`). */
const contactRow = (p: PeerInfo) => p.kind === 'a2a' && p.tags[0] === 'contact' && !p.name.includes('/')

/** 200: contact session rows (`peer:<contact>/session:<uuid>`) list under their contact with its machine, never with Open:
 * a contact's session is messaged, never read here. The `partial` line stands under the list whatever page is loaded. */
function Directory({ session }: { session: string }) {
  const canQuery1 = useQuerySupport(["list_peers"])
  const api = useApi()
  const navigate = useNavigate()
  const [q, setQ] = useState('')
  const query = q.trim() || null
  const peers = useInfiniteQuery({ enabled: canQuery1,
    queryKey: queryKeys.peers(session, query),
    queryFn: ({ pageParam }) => api.list_peers(session, query, pageParam, null),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.cursor,
  })
  const rows = peers.data?.pages.flatMap((p) => p.peers) ?? []
  // An engine without 200 sends no `partial`.
  const partial = [...new Set(peers.data?.pages.flatMap((p) => p.partial ?? []) ?? [])]
  return (
    <Section
      title="Peers"
      actions={
        <Input
          type="search"
          aria-label="Filter peers by name or address"
          placeholder="Filter"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          className={`w-44 ${TOUCH}`}
        />
      }
    >
      {peers.isError && (
        <Row label={<span className="text-destructive">{errorText(peers.error)}</span>}>
          <Button variant="outline" size="sm" className={TOUCH} onClick={() => void peers.refetch()}>
            <RefreshCw /> Retry
          </Button>
        </Row>
      )}
      {peers.isPending && <Empty>Loading…</Empty>}
      {peers.isSuccess && rows.length === 0 && (
        <Empty>{query ? 'No peer matches.' : 'No peer this chat may address right now.'}</Empty>
      )}
      {rows.map((p) => {
        // Only a canonical `session:<uuid>` address opens natively; remote/ACP/node peers have no chat here.
        const id = p.address.startsWith('session:') ? p.address.slice('session:'.length) : null
        return (
          <Row
            key={p.address}
            label={
              <span className="flex flex-wrap items-center gap-2">
                {p.name}
                <Badge variant="outline">{p.kind}</Badge>
                <Badge variant="outline">{p.relationship}</Badge>
                <Badge variant={p.state === 'offline' ? 'destructive' : 'secondary'}>{p.state}</Badge>
                {contactRow(p) && p.title?.startsWith('full access') && <Badge variant="outline">full access</Badge>}
              </span>
            }
            description={
              <>
                <span className="break-all font-mono text-xs">{p.address}</span>
                <span className="block">
                  {[p.engine, p.send ? ['send', p.reply && 'reply', p.stream && 'stream'].filter(Boolean).join(' · ') : 'does not accept messages'].filter(Boolean).join(' · ')}
                </span>
                {contactRow(p) && p.title && <span className="block">{p.title}</span>}
              </>
            }
          >
            {id && (
              <Button
                variant="outline"
                size="sm"
                className={TOUCH}
                onClick={() => void navigate({ to: '/c/$session', params: { session: id } })}
              >
                Open
              </Button>
            )}
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Copy address of ${p.name}`}
              onClick={() => navigator.clipboard.writeText(p.address).then(() => toast.message('Copied'), (e: unknown) => toast.error(errorText(e)))}
            >
              <Copy />
            </Button>
          </Row>
        )
      })}
      {partial.length > 0 && (
        <Row label={<span role="status" className="text-warning">{partialLine(partial)}</span>}>
          <Button variant="outline" size="sm" className={TOUCH} onClick={() => void peers.refetch()}>
            <RefreshCw /> Retry
          </Button>
        </Row>
      )}
      {peers.hasNextPage && (
        <Button
          variant="ghost"
          size="sm"
          className={`mt-2 ${TOUCH}`}
          disabled={peers.isFetchingNextPage}
          onClick={() => void peers.fetchNextPage()}
        >
          {peers.isFetchingNextPage ? 'Loading…' : 'More peers'}
        </Button>
      )}
    </Section>
  )
}

/** How the owner may act on one record: `answer` for the recipient, `resolve` for a blocked sender, else read-only. */
type Side = 'answer' | 'resolve' | null

function Requests({ session, focus }: { session: string; focus: string | null }) {
  const canQuery2 = useQuerySupport(["list_peer_messages"])
  const api = useApi()
  const [pendingOnly, setPendingOnly] = useState(true)
  const page = useInfiniteQuery({ enabled: canQuery2,
    queryKey: queryKeys.peerMessages(session, pendingOnly, 'infinite'),
    queryFn: ({ pageParam }) => api.list_peer_messages(session, pageParam, null, pendingOnly),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.cursor,
  })
  const pages = page.data?.pages ?? []
  // Exact ids the engine says a human may resolve now, gathered over the loaded pages.
  const answer = pages.flatMap((p) => p.awaiting_answer)
  const reply = pages.flatMap((p) => p.awaiting_reply)
  const records = pages.flatMap((p) => p.messages)
  const [open, setOpen] = useState<string | null>(focus)
  const side = (id: string): Side => (answer.includes(id) ? 'answer' : reply.includes(id) ? 'resolve' : null)

  return (
    <Section
      title="Peer requests"
      actions={
        <label className={`flex items-center gap-2 text-[13px] text-fg-secondary ${TOUCH}`}>
          Pending only
          <Switch size="sm" checked={pendingOnly} onCheckedChange={setPendingOnly} aria-label="Show pending requests only" />
        </label>
      }
    >
      {page.isError && (
        <Row label={<span className="text-destructive">{errorText(page.error)}</span>}>
          <Button variant="outline" size="sm" className={TOUCH} onClick={() => void page.refetch()}>
            <RefreshCw /> Retry
          </Button>
        </Row>
      )}
      {page.isPending && <Empty>Loading…</Empty>}
      {page.isSuccess && records.length === 0 && (
        <Empty>
          {pendingOnly
            ? 'No request is waiting on this chat or its peers. Pending waits do not survive a node restart; their durable outcome is in the full list.'
            : 'No peer exchange recorded for this chat.'}
        </Empty>
      )}
      {focus && page.isSuccess && !records.some((r) => r.receipt.id === focus) && (
        <Focused session={session} id={focus} />
      )}
      {records.map((r) => (
        <Request
          key={r.receipt.id}
          session={session}
          record={r}
          side={side(r.receipt.id)}
          open={open === r.receipt.id}
          focused={focus === r.receipt.id}
          onToggle={() => setOpen((v) => (v === r.receipt.id ? null : r.receipt.id))}
        />
      ))}
      {page.hasNextPage && (
        <Button
          variant="ghost"
          size="sm"
          className={`mt-2 ${TOUCH}`}
          disabled={page.isFetchingNextPage}
          onClick={() => void page.fetchNextPage()}
        >
          {page.isFetchingNextPage ? 'Loading…' : 'Older requests'}
        </Button>
      )}
    </Section>
  )
}

/** A deep-linked id that the loaded pages do not hold: the durable record alone, without an answer box (nothing pending). */
function Focused({ session, id }: { session: string; id: string }) {
  const canQuery3 = useQuerySupport(["peer_message"])
  const api = useApi()
  const record = useQuery({ enabled: canQuery3, queryKey: queryKeys.peerMessage(session, id), queryFn: () => api.peer_message(session, id) })
  if (record.isPending) return <Empty>Looking up {id}…</Empty>
  if (record.isError)
    return (
      <Row label={<span className="text-destructive">{id}</span>} description={errorText(record.error)} />
    )
  return <Request session={session} record={record.data} side={null} open focused onToggle={() => {}} />
}

function Request({
  session,
  record: r,
  side,
  open,
  focused,
  onToggle,
}: {
  session: string
  record: PeerRecord
  side: Side
  open: boolean
  focused: boolean
  onToggle: () => void
}) {
  const canQuery4 = useQuerySupport(["peer_message"])
  const api = useApi()
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (focused) ref.current?.scrollIntoView({ block: 'center' })
  }, [focused])
  const id = r.receipt.id
  // The full durable record on inspect; the list row already is this session's view, so the same id re-reads only when opened.
  const detail = useQuery({
    queryKey: queryKeys.peerMessage(session, id),
    queryFn: () => api.peer_message(session, id),
    enabled: canQuery4 && (open),
  })
  const [text, setText] = useState('')
  // 199 D15: an answer to a contact may carry files, picked on the chat's machine only.
  const [files, setFiles] = useState<string[]>([])
  const attach = side === 'answer' && isOutside(r.from)
  const send = useAction((t: string) => routed(session, () => answerPeer(api, session, id, t, attach ? files : [])), {
    keys: [['peer-messages', session], queryKeys.peerMessage(session, id)],
    onSuccess: (receipt) => {
      if (!receipt) return // parked behind the chat's OwnerOfflineCard: nothing was answered
      setText('')
      setFiles([])
      toast(`Answered ${id.slice(0, 8)} · ${receipt.state}`)
    },
  })
  const ready = !!text.trim() || (attach && files.length > 0)
  const d = detail.data ?? r
  const terminal = d.receipt.state === 'replied' || d.receipt.state === 'suppressed' || d.receipt.state === 'failed'

  return (
    <div
      ref={ref}
      className={`border-b border-border-subtle py-3 last:border-0 ${focused ? 'rounded-md bg-layer-3/40 px-2' : ''}`}
    >
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2 text-sm">
            {side === 'answer' && <Badge>Waiting for this chat's answer</Badge>}
            {side === 'resolve' && <Badge>This chat is waiting for a reply</Badge>}
            <Badge variant={d.receipt.state === 'failed' || d.receipt.state === 'suppressed' ? 'destructive' : 'outline'}>
              {d.receipt.state}
            </Badge>
            <span className="text-fg-secondary">{relative(r.created_at)}</span>
          </div>
          <div className="mt-1 break-all font-mono text-xs text-fg-secondary">
            {r.from} → {r.receipt.to}
          </div>
          <p className={`mt-1 whitespace-pre-wrap break-words text-sm ${open ? '' : 'line-clamp-3'}`}>{r.text}</p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button variant="outline" size="sm" className={TOUCH} aria-expanded={open} onClick={onToggle}>
            {open ? 'Hide' : 'Inspect'}
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Copy request id"
            onClick={() => navigator.clipboard.writeText(id).then(() => toast.message('Copied'), (e: unknown) => toast.error(errorText(e)))}
          >
            <Copy />
          </Button>
        </div>
      </div>
      {open && (
        <dl className="mt-2 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 break-all font-mono text-xs text-fg-secondary">
          <dt>id</dt>
          <dd>{id}</dd>
          {d.receipt.reply_to && (
            <>
              <dt>reply_to</dt>
              <dd>{d.receipt.reply_to}</dd>
            </>
          )}
          {d.request_scope && (
            <>
              <dt>scope</dt>
              <dd>{d.request_scope}</dd>
            </>
          )}
          {d.receipt.task_id && (
            <>
              <dt>task</dt>
              <dd>{d.receipt.task_id}</dd>
            </>
          )}
          {d.receipt.answered_by && (
            <>
              <dt>answered by</dt>
              <dd>{d.receipt.answered_by}</dd>
            </>
          )}
          {d.receipt.reply && (
            <>
              <dt>reply</dt>
              <dd className="whitespace-pre-wrap font-sans text-sm text-foreground">{d.receipt.reply}</dd>
            </>
          )}
          {d.receipt.error && (
            <>
              <dt>error</dt>
              <dd className="text-destructive">{d.receipt.error}</dd>
            </>
          )}
          {detail.isError && (
            <>
              <dt>lookup</dt>
              <dd className="text-destructive">{errorText(detail.error)}</dd>
            </>
          )}
        </dl>
      )}
      {side && !terminal && (
        <form
          className="mt-2 flex flex-col gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (ready) send.mutate(text.trim())
          }}
        >
          <Textarea
            aria-label={side === 'answer' ? `Answer request ${id} on behalf of this chat` : `Resolve this chat's wait on ${id}`}
            placeholder={
              side === 'answer'
                ? 'Answer sent to the sender as a message from this chat, recorded as answered by you.'
                : "Text this chat's blocked send_message returns; the recipient is not impersonated."
            }
            value={text}
            disabled={send.isPending}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) e.currentTarget.form?.requestSubmit()
            }}
          />
          <div className="flex flex-wrap items-center justify-end gap-2">
            {attach ? (
              <div className="mr-auto min-w-0">
                <AttachFiles session={session} files={files} onChange={setFiles} disabled={send.isPending} />
              </div>
            ) : null}
            <Button type="submit" size="sm" className={TOUCH} disabled={send.isPending || !ready} aria-busy={send.isPending || undefined}>
              {side === 'answer' ? 'Answer as this chat' : 'Resolve wait'}
            </Button>
          </div>
        </form>
      )}
    </div>
  )
}
