// 124 §5: the form field over the model catalog — an outline trigger plus the shared `ModelPicker`,
// replacing the Radix `Select` of 388 rows. `none` is the "Engine default" / "Off" row; it reports `null`.

import { ChevronDown } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import type { ModelInfo } from '@/rpc/bindings/ModelInfo'
import { useFavorites } from './ModelMenu'
import { ModelPicker } from './ModelPicker'

const NONE = '__none__'

export function ModelCombobox({
  models,
  value,
  onChange,
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
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button id={id} variant="outline" size={size} disabled={disabled} aria-label={ariaLabel} className={cn('justify-between font-normal', className)}>
          <span className="truncate">{current?.name ?? value ?? none?.label ?? 'Model'}</span>
          <ChevronDown className="size-4 opacity-70" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-80 p-0">
        <ModelPicker
          models={models}
          model={value ?? undefined}
          extra={value && !current ? [value] : undefined}
          none={none && { value: NONE, label: none.label }}
          favorites={favorites}
          onToggleFavorite={(id) => void toggleFavorite(id)}
          listClassName="max-h-[min(24rem,calc(var(--visible-band)-6rem))]"
          onSelect={(id) => {
            setOpen(false)
            onChange(id === NONE ? null : id)
          }}
        />
      </PopoverContent>
    </Popover>
  )
}
