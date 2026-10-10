// 124 §5: the form field over the model catalog — an outline trigger plus the shared `ModelPicker`,
// replacing the Radix `Select` of 388 rows. `none` is the "Engine default" / "Off" row; it reports `null`.
// 148: `effort` passes through to the picker's effort levels; the trigger then reads `name · effort`.
// 148 R2: on touch the picker opens in `ModelDialog`'s top-of-band dialog (127 §6), not a trigger-anchored popover
// the keyboard would cover (the search input focuses on open); desktop keeps the popover.
// 2026-10-10: the desktop popover is `modal`; inside the Settings dialog, the dialog's scroll lock otherwise cancels wheel events over the portaled list.

import { ChevronDown } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { touch } from '@/lib/platform'
import { cn } from '@/lib/utils'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { dialogPlacement, useFavorites } from './ModelMenu'
import { ModelPicker } from './ModelPicker'

const NONE = '__none__'

/** The trigger's text: the model's name (else its id, else `none`'s label), with ` · <effort>` when one is set. */
export function triggerLabel(current: ModelInfo | undefined, value: string | null | undefined, none: { label: string } | undefined, effort: string | null | undefined): string {
  const name = current?.name ?? value ?? none?.label ?? 'Model'
  return effort ? `${name} · ${effort}` : name
}

/** Where the picker opens: fingers get the top-of-band dialog (`dialogPlacement`), a mouse the trigger-anchored popover. */
export const pickerSurface = (touch: boolean): 'dialog' | 'popover' => (touch ? 'dialog' : 'popover')

export function ModelCombobox({
  models,
  value,
  onChange,
  effort,
  none,
  disabled,
  id,
  className,
  size,
  'aria-label': ariaLabel,
}: {
  models: ModelInfo[]
  /** The id on file; one missing from `models` stays selectable under the picker's `other` provider. */
  value: string | null | undefined
  onChange(id: string | null): void
  /** The current model's effort levels (`ModelPicker.effort`); `onSelect(null)` clears. */
  effort?: { levels: string[]; current: string | null | undefined; onSelect(level: string | null): void }
  none?: { label: string }
  disabled?: boolean
  id?: string
  className?: string
  size?: 'sm' | 'default'
  'aria-label'?: string
}) {
  const [open, setOpen] = useState(false)
  const current = value ? models.find((m) => m.id === value) : undefined
  const { favorites, toggleFavorite } = useFavorites()
  const trigger = (
    <Button id={id} variant="outline" size={size} disabled={disabled} aria-label={ariaLabel} className={cn('justify-between font-normal', className)}>
      <span className="truncate">{triggerLabel(current, value, none, effort?.current)}</span>
      <ChevronDown className="size-4 opacity-70" />
    </Button>
  )
  const picker = (className?: string, listClassName?: string) => (
    <ModelPicker
      models={models}
      model={value ?? undefined}
      extra={value && !current ? [value] : undefined}
      effort={effort && { ...effort, onSelect: (level) => (setOpen(false), effort.onSelect(level)) }}
      none={none && { value: NONE, label: none.label }}
      favorites={favorites}
      onToggleFavorite={(id) => void toggleFavorite(id)}
      className={className}
      listClassName={listClassName}
      onSelect={(id) => {
        setOpen(false)
        onChange(id === NONE ? null : id)
      }}
    />
  )
  if (pickerSurface(touch()) === 'dialog') {
    const place = dialogPlacement(true)
    return (
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogTrigger asChild>{trigger}</DialogTrigger>
        <DialogContent className={cn('max-w-sm overflow-hidden p-0', place.content)}>
          <DialogHeader className="sr-only">
            <DialogTitle>Model</DialogTitle>
            <DialogDescription>Pick the model and effort</DialogDescription>
          </DialogHeader>
          {picker(cn('[&_[cmdk-input]]:h-12 **:data-[slot=command-input-wrapper]:h-12', place.picker), place.list)}
        </DialogContent>
      </Dialog>
    )
  }
  // `modal`: the popover's own scroll lock supersedes a parent Dialog's (react-remove-scroll keeps a stack; only the top lock
  // acts), so the portaled list scrolls with the wheel inside the Settings dialog instead of having its wheel events cancelled.
  return (
    <Popover open={open} onOpenChange={setOpen} modal>
      <PopoverTrigger asChild>{trigger}</PopoverTrigger>
      <PopoverContent align="start" className="w-80 p-0">
        {picker(undefined, 'max-h-[min(24rem,calc(var(--visible-band)-6rem))]')}
      </PopoverContent>
    </Popover>
  )
}
