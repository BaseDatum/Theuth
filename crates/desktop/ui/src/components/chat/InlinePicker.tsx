import { Check, type LucideIcon } from 'lucide-react'
import { type RefObject, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Command, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { Sheet, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { useIsMobile } from '@/hooks/use-mobile'
import { useKeyboardInset } from '@/lib/mobile'
import { cn } from '@/lib/utils'

export type InlinePickerItem = {
  /** Stable unique identifier (canonical skill name or saved prompt id). */
  id: string
  /** Primary display label. */
  label: string
  /** Secondary description or text preview. */
  detail?: string
  /** Optional leading icon. */
  icon?: LucideIcon
  /** Optional trailing checkmark (e.g. already-parked skill). */
  checked?: boolean
}

/**
 * Quiet inline autocomplete popup shared by `$` (skills) and `&` (prompts).
 * Copies `AttachPicker`'s quiet-mode chrome and keyboard contract: Popover anchored to the composer box
 * on desktop (textarea keeps focus; capture-phase window keydown owns ↑ ↓ ⏎ ⇥ Esc, leaving Shift+Enter
 * and every other key to the textarea), bottom Sheet on mobile.
 */
export function InlinePicker({
  open,
  label,
  heading,
  items,
  anchor,
  onPick,
  onClose,
}: {
  open: boolean
  label: string
  heading: string
  items: readonly InlinePickerItem[]
  anchor: RefObject<HTMLElement | null>
  onPick(id: string): void
  onClose(): void
}) {
  const isMobile = useIsMobile()
  const inset = useKeyboardInset()
  const [highlighted, setHighlighted] = useState('')
  const effectiveOpen = open && items.length > 0

  const current = items.find((r) => r.id === highlighted) ?? items[0]
  useEffect(() => {
    if (!effectiveOpen) {
      setHighlighted('')
      return
    }
    if (current && current.id !== highlighted) setHighlighted(current.id)
  }, [effectiveOpen, current, highlighted])

  const move = (dir: 1 | -1) => {
    if (items.length === 0) return
    const i = items.findIndex((r) => r.id === current?.id)
    const next = items[(i + dir + items.length) % items.length]
    if (next) setHighlighted(next.id)
  }
  const pickCurrent = () => {
    if (current) onPick(current.id)
  }
  const quietRef = useRef({ move, pickCurrent, onClose })
  quietRef.current = { move, pickCurrent, onClose }

  useEffect(() => {
    if (!effectiveOpen) return
    const on = (e: globalThis.KeyboardEvent) => {
      if (e.isComposing) return
      const { move, pickCurrent, onClose } = quietRef.current
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') move(e.key === 'ArrowDown' ? 1 : -1)
      else if (e.key === 'Enter' || e.key === 'Tab') {
        if (e.key === 'Enter' && e.shiftKey) return
        pickCurrent()
      } else if (e.key === 'Escape') onClose()
      else return
      e.preventDefault()
      e.stopPropagation()
    }
    window.addEventListener('keydown', on, true)
    return () => window.removeEventListener('keydown', on, true)
  }, [effectiveOpen])

  const command = (
    <Command
      label={label}
      shouldFilter={false}
      loop
      disablePointerSelection={isMobile}
      value={current?.id ?? ''}
      onValueChange={(v) => {
        const match = items.find((r) => r.id === v) ?? items.find((r) => r.id.toLowerCase() === v.toLowerCase())
        if (match) setHighlighted(match.id)
      }}
      className="min-h-0 flex-1 bg-transparent"
    >
      <CommandList className="min-h-0 max-h-none flex-1 overscroll-contain">
        <CommandGroup heading={heading} className="pt-1">
          {items.map((item) => {
            const Icon = item.icon
            return (
              <CommandItem
                key={item.id}
                value={item.id}
                onSelect={() => onPick(item.id)}
                className="min-h-10 items-center gap-2.5 py-1.5 pointer-coarse:min-h-11"
              >
                {Icon && <Icon className="size-4 shrink-0 text-fg-secondary" aria-hidden />}
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">{item.label}</div>
                  {item.detail && <div className="truncate text-xs text-fg-tertiary">{item.detail}</div>}
                </div>
                {item.checked && <Check className="ml-auto size-3.5 shrink-0 text-fg-secondary" aria-hidden />}
              </CommandItem>
            )
          })}
        </CommandGroup>
      </CommandList>
    </Command>
  )

  if (isMobile) {
    return (
      <Sheet open={effectiveOpen} onOpenChange={(o) => !o && onClose()}>
        <SheetContent side="bottom" showCloseButton={false} className="max-h-[75vh] gap-0 p-0 motion-reduce:animate-none">
          <SheetTitle className="sr-only">{label}</SheetTitle>
          <SheetDescription className="sr-only">Tap an item to insert it into the composer.</SheetDescription>
          <div className="mx-auto mt-2 h-1 w-10 shrink-0 rounded-full bg-border" aria-hidden />
          {command}
          <div className="shrink-0 border-t p-3" style={{ paddingBottom: `calc(0.75rem + ${inset}px)` }}>
            <Button variant="outline" className="min-h-11 w-full" onClick={onClose}>
              Close
            </Button>
          </div>
        </SheetContent>
      </Sheet>
    )
  }

  return (
    <Popover open={effectiveOpen} onOpenChange={(o) => !o && onClose()}>
      <PopoverAnchor virtualRef={anchor} />
      <PopoverContent
        side="top"
        align="start"
        sideOffset={8}
        collisionPadding={8}
        onOpenAutoFocus={(e) => e.preventDefault()}
        onCloseAutoFocus={(e) => e.preventDefault()}
        onFocusOutside={(e) => e.preventDefault()}
        className={cn('flex max-h-[min(320px,calc(100dvh-6rem))] w-[min(420px,calc(100vw-2rem))] flex-col p-0')}
      >
        {command}
        <div className="flex min-h-8 shrink-0 items-center border-t px-3 py-1 text-xs text-fg-tertiary">
          <span className="truncate">⏎ / ⇥ select · ↑↓ navigate · Esc dismiss</span>
        </div>
      </PopoverContent>
    </Popover>
  )
}
