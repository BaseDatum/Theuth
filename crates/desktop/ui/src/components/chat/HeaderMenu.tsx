import { ChevronDown, type LucideIcon } from 'lucide-react'
import { type ComponentProps, createContext, isValidElement, type ReactElement, type ReactNode, type RefObject, useContext, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { panelGeometry } from '@/lib/header'
import { cn } from '@/lib/utils'

/** Closes the chevron menu; a no-op outside it, so row components can call it unconditionally. */
const Close = createContext<() => void>(() => {})
export const useHeaderMenuClose = () => useContext(Close)

/**
 * One line of the chevron menu: icon · label · detail. With `onSelect` it is a button that closes the menu first (a
 * dialog it opens lives in the header, not in the menu, so it survives); without one, an indicator only, unless a
 * caller spreads a Radix trigger on it (`asChild`), when the nested menu takes over the click. `icon` is a LucideIcon,
 * or a ready element (a live indicator carrying its own state) that must size itself to `size-4`.
 */
export function MenuRow({
  icon: Icon,
  label,
  detail,
  onSelect,
  className,
  ...props
}: Omit<ComponentProps<'button'>, 'onSelect'> & {
  icon?: LucideIcon | ReactElement
  label: ReactNode
  detail?: ReactNode
  onSelect?(): void
}) {
  const close = useHeaderMenuClose()
  const body = (
    <>
      {isValidElement(Icon) ? Icon : Icon ? <Icon className="size-4 shrink-0 text-fg-secondary" /> : <span className="size-4 shrink-0" />}
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
 * The compact header's chevron and its floating panel, just under the bar and centred on the pane: `panelGeometry`
 * sets its left edge and width from the pane's rect, so it stays inside the pane and the viewport wherever the pane
 * sits; Radix only portals, dismisses and manages focus (no collision logic, no CSS-variable width: Firefox and
 * Chromium then agree). Non-modal Popover: it never touches `body{pointer-events}`, so a Sheet opened from a row
 * (ArtifactPanel) neither fights it nor leaves the body dead.
 */
export function HeaderMenu({
  header,
  badge,
  children,
  open: openProp,
  onOpenChange,
}: {
  /** The `<header>`: the panel hangs under it; its parent, the pane, bounds the panel. */
  header: RefObject<HTMLElement | null>
  /** A warning dot on the chevron (pending approvals live behind it when the bar is compact). */
  badge?: boolean
  children: ReactNode
  /** Controlled open state (198: the native title pill opens the panel); uncontrolled when absent. */
  open?: boolean
  onOpenChange?(open: boolean): void
}) {
  const [own, setOwn] = useState(false)
  const open = openProp ?? own
  const setOpen = onOpenChange ?? setOwn
  const button = useRef<HTMLButtonElement>(null)
  // The header's box is the anchor. A virtual ref, not `PopoverTrigger`: Radix would register the trigger as anchor
  // first and drop it a render later, by which time that button node has been replaced (0 × 0 rect, panel at the
  // viewport's left edge). The chevron is a plain toggle instead; `onInteractOutside` keeps its click from counting
  // as a dismissal, so the toggle closes as well as opens.
  const anchor = useRef({ getBoundingClientRect: () => header.current?.getBoundingClientRect() ?? new DOMRect() })
  // Re-measure while open when the pane resizes (split drag, window resize).
  const [, bump] = useState(0)
  useEffect(() => {
    const pane = header.current?.parentElement
    if (!open || !pane) return
    const ro = new ResizeObserver(() => bump((n) => n + 1))
    ro.observe(pane)
    return () => ro.disconnect()
  }, [open, header])
  const pane = header.current?.parentElement?.getBoundingClientRect()
  const geo = pane ? panelGeometry(pane.left, pane.right, window.innerWidth) : { left: 0, width: 0 }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverAnchor virtualRef={anchor} />
      <Button
        ref={button}
        variant="ghost"
        size="icon"
        className="relative rounded-lg text-fg-secondary"
        aria-label={open ? 'Hide header controls' : 'More header controls'}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <ChevronDown className={cn('size-5 transition-transform duration-200', open && 'rotate-180')} />
        {badge && <span className="absolute top-1.5 right-1.5 size-2 rounded-full bg-warning" />}
      </Button>
      <PopoverContent
        side="bottom"
        sideOffset={2}
        align="start"
        alignOffset={geo.left - (pane?.left ?? 0)}
        avoidCollisions={false}
        style={{ width: geo.width }}
        className="flex max-h-[70vh] flex-col gap-0.5 overflow-y-auto p-1.5"
        onInteractOutside={(e) => {
          if (button.current?.contains(e.target as Node)) e.preventDefault()
        }}
        onCloseAutoFocus={(e) => {
          e.preventDefault()
          button.current?.focus()
        }}
      >
        <Close.Provider value={() => setOpen(false)}>{children}</Close.Provider>
      </PopoverContent>
    </Popover>
  )
}
