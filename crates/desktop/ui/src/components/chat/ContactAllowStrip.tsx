import { ChevronDown } from 'lucide-react'
import { FirstButtons, waitingCount, waitingLine } from '@/components/settings/Contacts'
import { MODE_LABEL, modeLines, useFirstWaiting } from '@/lib/contacts'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import type { FirstWaiting } from '@/rpc/bindings/FirstWaiting'

const who = (w: FirstWaiting) => `${w.contact}${w.machine ? ` (${w.machine})` : ''}`

function Row({ w, detail }: { w: FirstWaiting; detail: boolean }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs">
        <span className="min-w-0 flex-1 basis-48 truncate text-fg-secondary" title={waitingLine(w)}>
          {who(w)} wants to message this chat · {MODE_LABEL[w.mode]}{waitingCount(w)}
        </span>
        <FirstButtons contact={w.contact} exp={w.export} />
      </div>
      {detail && w.preview && <p className="line-clamp-3 break-words text-xs text-fg-secondary">“{w.preview}”</p>}
    </div>
  )
}

/** 186: one contact's messages queued behind the running turn: a quiet line, no buttons; open, each one's first line. */
function QueuedRow({ rows, detail }: { rows: FirstWaiting[]; detail: boolean }) {
  const first = rows[0] as FirstWaiting
  const n = rows.reduce((sum, w) => sum + w.waiting, 0)
  return (
    <div className="flex min-w-0 flex-col gap-0.5 text-xs text-fg-tertiary">
      <span className="truncate">
        {who(first)} · {n} message{n === 1 ? ' waits' : 's wait'} for this turn to finish
      </span>
      {detail &&
        rows.map((w) =>
          w.preview ? (
            <p key={w.export} className="line-clamp-2 break-words text-fg-secondary">
              “{w.preview}”
            </p>
          ) : null,
        )}
    </div>
  )
}

/**
 * 183 Phase 3: a contact's first message to this chat waits for its owner — a strip above the composer like the kernels',
 * never a transcript card. Disappears once answered (`A2aChanged` refetches `contact_first_waiting`). 186: a cautious
 * message queued behind the running turn waits here too, quietly, one row per contact, below the holds; it goes when
 * its turn starts (the engine stops listing it).
 */
export function ContactAllowStrip({ session, embedded = false }: { session: string; embedded?: boolean }) {
  const mine = (useFirstWaiting(true, true).data ?? []).filter((w) => w.session === session)
  const [open, setOpen] = useDisclosure(disclosureKey(session, 'contact-allow'))
  const held = mine.filter((w) => !w.queued)
  const queued = Object.values(Object.groupBy(mine.filter((w) => w.queued), (w) => w.contact)) as FirstWaiting[][]
  const count = held.length + queued.length
  if (count === 0) return null
  return (
    <div className={cn('w-full shrink-0', !embedded && 'mx-auto px-4 md:max-w-[768px]')} role="region" aria-label="Contacts waiting">
      <div className="flex flex-col gap-1 rounded-t-xl border border-b-0 border-border bg-layer-1 px-3 py-1.5">
        <div className="flex items-center gap-2">
          <button type="button" className="flex shrink-0 items-center text-fg-secondary pointer-coarse:min-h-12" aria-expanded={open} aria-label="Details" onClick={() => setOpen(!open)}>
            <ChevronDown className={cn('size-3.5 transition-transform', open && 'rotate-180')} aria-hidden />
          </button>
          <div className="min-w-0 flex-1">
            {count > 1 ? (
              <span className="text-xs text-fg-secondary">{count} contacts waiting</span>
            ) : held[0] ? (
              <Row w={held[0]} detail={open} />
            ) : (
              <QueuedRow rows={queued[0] as FirstWaiting[]} detail={open} />
            )}
          </div>
        </div>
        {count > 1 && open && (
          <>
            {held.map((w) => (
              <Row key={`${w.contact}:${w.export}`} w={w} detail />
            ))}
            {queued.map((rows) => (
              <QueuedRow key={(rows[0] as FirstWaiting).contact} rows={rows} detail />
            ))}
          </>
        )}
        {open &&
          modeLines(held.map((w) => w.mode)).map((l) => (
            <p key={l} className="text-xs text-fg-tertiary">
              {l}
            </p>
          ))}
      </div>
    </div>
  )
}
