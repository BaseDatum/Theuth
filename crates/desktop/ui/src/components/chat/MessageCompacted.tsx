import { memo, useState } from 'react'
import { Streamdown } from 'streamdown'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'

/** Centred `── context compacted (epoch n · style) ──` divider; click shows the summary when history carries one. */
export const MessageCompacted = memo(function MessageCompacted({
  epoch,
  style,
  summary,
}: {
  epoch: number
  style: CompactStyle
  summary?: string
}) {
  const [open, setOpen] = useState(false)
  const label = style === 'clear' ? `context cleared (epoch ${epoch})` : `context compacted (epoch ${epoch} · ${style})`
  return (
    <div className="my-3 text-sm text-fg-tertiary">
      <button
        type="button"
        className="flex w-full items-center gap-3 disabled:cursor-default"
        disabled={!summary}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <span className="h-px flex-1 bg-border-strong" />
        <span>
          {label}
          {summary && !open && ' · click for summary'}
        </span>
        <span className="h-px flex-1 bg-border-strong" />
      </button>
      {open && summary && (
        <Streamdown mode="static" className="mt-2 rounded-xl bg-layer-2 p-4 text-sm text-fg-secondary">
          {summary}
        </Streamdown>
      )}
    </div>
  )
})
