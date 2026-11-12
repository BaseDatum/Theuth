import { ChevronDown } from 'lucide-react'
import { FirstButtons, waitingCount, waitingLine } from '@/components/settings/Contacts'
import { useFirstWaiting } from '@/lib/contacts'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import type { FirstWaiting } from '@/rpc/bindings/FirstWaiting'

const who = (w: FirstWaiting) => `${w.contact}${w.machine ? ` (${w.machine})` : ''}`

function Row({ w, detail }: { w: FirstWaiting; detail: boolean }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs">
        <span className="min-w-0 flex-1 basis-48 truncate text-fg-secondary" title={waitingLine(w)}>
          {who(w)} wants to message this chat{waitingCount(w)}
        </span>
        <FirstButtons contact={w.contact} exp={w.export} elsewhere={w.elsewhere} />
      </div>
      {detail && w.preview && <p className="line-clamp-3 break-words text-xs text-fg-secondary">“{w.preview}”</p>}
    </div>
  )
}

/**
 * 183 Phase 3: a contact's first message to this chat waits for its owner — a strip above the composer like the kernels',
 * never a transcript card. Disappears once answered (`A2aChanged` refetches `contact_first_waiting`). 191: the list covers every
 * machine of the mesh, and `A2aChanged` also fires when another machine's hold row arrives or goes.
 */
export function ContactAllowStrip({ session, embedded = false }: { session: string; embedded?: boolean }) {
  const held = (useFirstWaiting().data ?? []).filter((w) => w.session === session)
  const [open, setOpen] = useDisclosure(disclosureKey(session, 'contact-allow'))
  if (held.length === 0) return null
  return (
    <div className={cn('w-full shrink-0', !embedded && 'mx-auto px-4 md:max-w-[768px]')} role="region" aria-label="Contacts waiting">
      <div className="flex flex-col gap-1 rounded-t-xl border border-b-0 border-border bg-layer-1 px-3 py-1.5">
        <div className="flex items-center gap-2">
          <button type="button" className="flex shrink-0 items-center text-fg-secondary pointer-coarse:min-h-12" aria-expanded={open} aria-label="Details" onClick={() => setOpen(!open)}>
            <ChevronDown className={cn('size-3.5 transition-transform', open && 'rotate-180')} aria-hidden />
          </button>
          <div className="min-w-0 flex-1">
            {held.length > 1 ? <span className="text-xs text-fg-secondary">{held.length} contacts waiting</span> : <Row w={held[0] as FirstWaiting} detail={open} />}
          </div>
        </div>
        {held.length > 1 && open && held.map((w) => <Row key={`${w.contact}:${w.export}`} w={w} detail />)}
      </div>
    </div>
  )
}
