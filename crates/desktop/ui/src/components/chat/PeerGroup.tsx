import { ChevronRight } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { disclosureKey, useDisclosure, useToolRuns } from '@/lib/tool-runs'
import { formatAt } from '@/lib/time'
import { cn } from '@/lib/utils'
import type { Entry } from '@/store/sessions'
import { Message } from './Message'
import { SenderHead, capLabel, useAwaitingAnswer } from './MessagePeer'
import { useDisclosed } from './disclosure'
import { type Sender, firstLine, isOutside, resolveSender, sameText, useDirectory } from './peer-identity'

type Peer = Extract<Entry, { kind: 'peer' }>

const lineKey = (id: string) => disclosureKey(id, 'line')

/** One folded message: its time and first line; a click opens its full card in place. */
function Line({ entry, repeat, session }: { entry: Peer; repeat: boolean; session: string }) {
  const [open, setOpen] = useDisclosure(lineKey(entry.id))
  return (
    <div className="border-t border-border-subtle first:border-t-0">
      <button
        type="button"
        className="flex w-full min-w-0 items-baseline gap-2.5 py-1.5 text-left pointer-coarse:min-h-11"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <ChevronRight className={cn('size-3.5 shrink-0 self-center text-fg-tertiary transition-transform', open && 'rotate-90')} />
        <span className="shrink-0 text-xs whitespace-nowrap text-fg-tertiary tabular-nums">{entry.at === undefined ? '' : formatAt(entry.at)}</span>
        <span className={cn('min-w-0 truncate', repeat && 'text-fg-tertiary')}>{repeat ? 'Same as the message above' : firstLine(entry.text)}</span>
      </button>
      {open ? <Message entry={entry} prompts={null} session={session} /> : null}
    </div>
  )
}

/** The folded card: one header, one line per message. Expand all opens every line; each line still toggles alone. */
function Folded({ lines, sender, session }: { lines: { entry: Peer; repeat: boolean }[]; sender: Sender; session: string }) {
  const all = useDisclosed((open) => lines.every((l) => open[lineKey(l.entry.id)] === true))
  const setAll = (next: boolean) =>
    useToolRuns.setState((s) => ({ disclosed: { ...s.disclosed, ...Object.fromEntries(lines.map((l) => [lineKey(l.entry.id), next])) } }))
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-2.5 text-sm">
      <SenderHead sender={sender} count={`${lines.length} messages`} tag={lines[0]?.entry.contact && capLabel(lines[0].entry.contact.cap)}>
        <Button type="button" variant="ghost" size="xs" className="pointer-coarse:min-h-11" aria-expanded={all} onClick={() => setAll(!all)}>
          {all ? 'Collapse all' : 'Expand all'}
        </Button>
      </SenderHead>
      <div className="mt-1.5">
        {lines.map((l) => (
          <Line key={l.entry.id} entry={l.entry} repeat={l.repeat} session={session} />
        ))}
      </div>
    </div>
  )
}

/**
 * Two or more adjacent peer cards from one sender (Sky, 2026-11-10): one header, one line each (time plus first line),
 * a click opens that message's card. A message with the previous one's words (trimmed, whitespace collapsed) is
 * marked as a repeat instead of shown again. A message this chat still owes an answer to is never folded: it splits
 * the group and shows as its full card, so "Expects a reply" and "Answer as this chat" stay in view. Open state is in
 * the disclosure store (171F B13).
 */
export function PeerGroup({ run, session }: { run: Peer[]; session: string }) {
  const first = run[0] as Peer
  const address = first.origin?.from ?? first.from
  const sender = resolveSender(address, { ...useDirectory(session, isOutside(address) || !!first.contact), note: first.contact })
  const awaiting = useAwaitingAnswer(session).data
  // Maximal stretches of folded lines between the awaited messages; a stretch of one is a plain card too.
  const parts: { entry: Peer; repeat: boolean }[][] = []
  let open = true
  run.forEach((entry, i) => {
    const owed = !!entry.origin && !!awaiting?.includes(entry.origin.id)
    const line = { entry, repeat: i > 0 && sameText(entry.text, (run[i - 1] as Peer).text) }
    if (owed || !open || parts.length === 0) parts.push([line])
    else (parts[parts.length - 1] as typeof parts[number]).push(line)
    open = !owed
  })
  return (
    <>
      {parts.map((lines) => {
        const head = (lines[0] as { entry: Peer }).entry
        return lines.length === 1 ? (
          <Message key={head.id} entry={head} prompts={null} session={session} />
        ) : (
          <Folded key={head.id} lines={lines} sender={sender} session={session} />
        )
      })}
    </>
  )
}
