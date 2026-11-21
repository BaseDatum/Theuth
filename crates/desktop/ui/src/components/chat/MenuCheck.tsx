import { Check, Minus } from 'lucide-react'
import { cn } from '@/lib/utils'

/** One tri-state checkbox row of the composer's tools popover (`McpMenu`): an MCP server or a builtin category for
 * this chat. `live` false greys the row and leaves the box empty; `title` then says why. */
export function MenuCheck({
  label,
  state,
  live,
  busy,
  title,
  count,
  onToggle,
}: {
  label: string
  state: 'on' | 'off' | 'mixed'
  live: boolean
  busy: boolean
  title: string | undefined
  count: string
  onToggle: () => void
}) {
  return (
    <button
      type="button"
      role="menuitemcheckbox"
      aria-checked={state === 'mixed' ? 'mixed' : state === 'on'}
      disabled={!live || busy}
      title={title}
      onClick={onToggle}
      className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent disabled:pointer-events-none disabled:opacity-50 pointer-coarse:min-h-11"
    >
      <span
        aria-hidden="true"
        className={cn(
          'flex size-4 shrink-0 items-center justify-center rounded-[4px] border border-border',
          live && state !== 'off' && 'border-fg-accent bg-fg-accent text-white',
        )}
      >
        {live && state === 'on' && <Check className="size-3" />}
        {live && state === 'mixed' && <Minus className="size-3" />}
      </span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <span className="shrink-0 text-xs text-fg-tertiary">{count}</span>
    </button>
  )
}
