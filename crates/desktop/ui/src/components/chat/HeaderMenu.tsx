import { ChevronDown, type LucideIcon } from 'lucide-react'
import { type ComponentProps, createContext, type ReactNode, type RefObject, useContext, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'

/** Closes the chevron menu; a no-op outside it, so row components can call it unconditionally. */
const Close = createContext<() => void>(() => {})
export const useHeaderMenuClose = () => useContext(Close)

/**
 * One line of the chevron menu: icon · label · detail. With `onSelect` it is a button that closes the menu first (a
 * dialog it opens lives in the header, not in the menu, so it survives); without one, an indicator only, unless a
 * caller spreads a Radix trigger on it (`asChild`), when the nested menu takes over the click.
 */
export function MenuRow({
  icon: Icon,
  label,
  detail,
  onSelect,
  className,
  ...props
}: Omit<ComponentProps<'button'>, 'onSelect'> & {
  icon?: LucideIcon
  label: ReactNode
  detail?: ReactNode
  onSelect?(): void
}) {
  const close = useHeaderMenuClose()
  const body = (
    <>
      {Icon ? <Icon className="size-4 shrink-0 text-fg-secondary" /> : <span className="size-4 shrink-0" />}
      <span className="shrink-0 text-left">{label}</span>
      {detail !== undefined && detail !== null && (
        <span className="ml-auto min-w-0 truncate text-xs text-fg-tertiary tabular-nums" title={typeof detail === 'string' ? detail : undefined}>
          {detail}
        </span>
      )}
    </>
  )
  const cls = cn('flex h-10 w-full items-center gap-3 rounded-md px-2 text-sm', className)
  // Indicator only when nothing drives it: no `onSelect`, and no Radix trigger props (`onPointerDown`, `ref`, aria) spread in.
  if (onSelect === undefined && Object.keys(props).length === 0) return <div className={cls}>{body}</div>
  return (
    <button
      type="button"
      {...props}
      className={cn(cls, 'hover:bg-accent focus-visible:bg-accent focus-visible:outline-hidden')}
      onClick={(e) => {
        props.onClick?.(e)
        if (onSelect) {
          close()
          onSelect()
        }
      }}
    >
      {body}
    </button>
  )
}

/**
 * The compact header's chevron and its floating panel: centred in the pane (the header's box is the anchor, its width
 * the panel's ceiling), just under the bar, kept inside the pane. Non-modal Popover: it never touches
 * `body{pointer-events}`, so a Sheet opened from a row (ArtifactPanel) neither fights it nor leaves the body dead.
 */
export function HeaderMenu({
  header,
  badge,
  children,
}: {
  /** The `<header>` (position: relative): the anchor span covers it, and its parent, the pane, bounds the panel. */
  header: RefObject<HTMLElement | null>
  /** A warning dot on the chevron (pending approvals live behind it when the bar is compact). */
  badge?: boolean
  children: ReactNode
}) {
  const [open, setOpen] = useState(false)
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="relative rounded-lg text-fg-secondary"
          aria-label={open ? 'Hide header controls' : 'More header controls'}
        >
          <ChevronDown className={cn('size-5 transition-transform duration-200', open && 'rotate-180')} />
          {badge && <span className="absolute top-1.5 right-1.5 size-2 rounded-full bg-warning" />}
        </Button>
      </PopoverTrigger>
      {/* After the trigger on purpose: Radix registers the trigger as anchor first and only drops it a render later,
          by which time that button node has been replaced; the later ref callback wins, so this one must be it. */}
      <PopoverAnchor asChild>
        <span className="pointer-events-none absolute inset-0" />
      </PopoverAnchor>
      <PopoverContent
        align="center"
        side="bottom"
        sideOffset={2}
        collisionBoundary={header.current?.parentElement ?? undefined}
        collisionPadding={8}
        className="flex max-h-[70vh] w-[min(20rem,calc(var(--radix-popover-trigger-width)-1rem))] flex-col gap-0.5 overflow-y-auto p-1.5"
      >
        <Close.Provider value={() => setOpen(false)}>{children}</Close.Provider>
      </PopoverContent>
    </Popover>
  )
}
