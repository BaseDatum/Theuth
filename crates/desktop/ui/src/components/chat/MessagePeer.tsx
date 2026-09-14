import { ChevronRight } from 'lucide-react'
import { memo, useState } from 'react'
import { Streamdown } from 'streamdown'

/** `⟵ from` card for a peer message, subagent result or job result; the first line until expanded. */
export const MessagePeer = memo(function MessagePeer({ from, text }: { from: string; text: string }) {
  const [open, setOpen] = useState(false)
  const first = text.split('\n', 1)[0] ?? ''
  const more = text.length > first.length
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
        <Streamdown mode="static" className="mt-2 text-sm">
          {text}
        </Streamdown>
      ) : (
        <div className="mt-1 truncate text-fg-secondary">{first}</div>
      )}
    </div>
  )
})
