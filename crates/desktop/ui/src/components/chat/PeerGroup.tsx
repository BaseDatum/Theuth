import { ChevronRight } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import { formatAt } from '@/lib/time'
import { cn } from '@/lib/utils'
import type { Entry } from '@/store/sessions'
import { Message } from './Message'
import { SenderHead } from './MessagePeer'
import { firstLine, isOutside, resolveSender, sameText, useDirectory } from './peer-identity'

type Peer = Extract<Entry, { kind: 'peer' }>

/** One folded message: its time and first line; a click opens its full card in place. */
function Line({ entry, repeat, all, session }: { entry: Peer; repeat: boolean; all: boolean; session: string }) {
  const [open, setOpen] = useDisclosure(disclosureKey(entry.id, 'line'))
  const shown = open || all
  return (
    <div className="border-t border-border-subtle first:border-t-0">
      <button
        type="button"
        className="flex w-full min-w-0 items-baseline gap-2.5 py-1.5 text-left pointer-coarse:min-h-11"
        aria-expanded={shown}
        onClick={() => setOpen(!shown)}
      >
        <ChevronRight className={cn('size-3.5 shrink-0 self-center text-fg-tertiary transition-transform', shown && 'rotate-90')} />
        <span className="shrink-0 text-xs whitespace-nowrap text-fg-tertiary tabular-nums">{entry.at === undefined ? '' : formatAt(entry.at)}</span>
        <span className={cn('min-w-0 truncate', repeat && 'text-fg-tertiary')}>{repeat ? 'Same as the message above' : firstLine(entry.text)}</span>
      </button>
      {shown ? <Message entry={entry} prompts={null} session={session} /> : null}
    </div>
  )
}

/**
 * Two or more adjacent peer cards from one sender (Sky, 2026-11-10): one header, one line each (time plus first line),
 * a click opens that message's card. A message with the previous one's words (trimmed, whitespace collapsed) is
 * marked as a repeat instead of shown again. Open state is in the disclosure store (171F B13).
 */
export function PeerGroup({ run, session }: { run: Peer[]; session: string }) {
  const first = run[0] as Peer
  const address = first.origin?.from ?? first.from
  const sender = resolveSender(address, useDirectory(session, isOutside(address)))
  const [all, setAll] = useDisclosure(disclosureKey('peer-group', first.id))
  return (
    <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-2.5 text-sm">
      <SenderHead sender={sender} count={`${run.length} messages`}>
        <Button type="button" variant="ghost" size="xs" className="pointer-coarse:min-h-11" aria-expanded={all} onClick={() => setAll(!all)}>
          {all ? 'Collapse all' : 'Expand all'}
        </Button>
      </SenderHead>
      <div className="mt-1.5">
        {run.map((e, i) => (
          <Line key={e.id} entry={e} repeat={i > 0 && sameText(e.text, (run[i - 1] as Peer).text)} all={all} session={session} />
        ))}
      </div>
    </div>
  )
}
