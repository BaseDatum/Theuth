import { Paperclip, Plus } from 'lucide-react'
import { type DragEvent, useRef, useState } from 'react'
import { AttachChip } from '@/components/chat/AttachChip'
import { AttachPicker } from '@/components/chat/AttachPicker'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { type AttachRef, addRefs, droppedRef, key } from '@/lib/attach'
import { cn } from '@/lib/utils'

/**
 * A task's attachments (70 TASK-ATTACHMENTS): the composer's chips and `+` picker (design 66) over a plain list.
 * `onChange` gets the whole new list — the create sheet keeps it in state, the detail page sends it as
 * `task_update {attachments}` (replace whole). Sidebar/notes rows drag onto it like onto a composer.
 */
export function AttachmentsEditor({
  value,
  onChange,
  disabled,
  className,
}: {
  value: readonly AttachRef[]
  onChange(next: AttachRef[]): void
  disabled?: boolean
  className?: string
}) {
  const box = useRef<HTMLDivElement>(null)
  const [picking, setPicking] = useState(false)
  const [over, setOver] = useState(false)
  const onDrop = (e: DragEvent) => {
    setOver(false)
    const ref = droppedRef(e.dataTransfer)
    if (!ref) return
    e.preventDefault()
    if (!disabled) onChange(addRefs(value, [ref]))
  }
  return (
    <div
      ref={box}
      className={cn('flex flex-wrap items-center gap-1.5 rounded-md', over && 'ring-2 ring-primary/40', className)}
      onDragOver={(e) => {
        e.preventDefault()
        setOver(true)
      }}
      onDragLeave={() => setOver(false)}
      onDrop={onDrop}
    >
      <AttachmentChips items={value} onRemove={disabled ? undefined : (r) => onChange(value.filter((x) => key(x) !== key(r)))} />
      <Button type="button" size="sm" variant="ghost" className="h-7 px-2 text-xs" disabled={disabled} onClick={() => setPicking(true)} aria-label="Attach">
        <Plus /> Attach…
      </Button>
      {value.length === 0 && <span className="text-xs text-fg-tertiary">or drop a note, chat, artifact or meeting here</span>}
      <AttachPicker session={null} open={picking} anchor={box} onClose={() => setPicking(false)} onAttach={(refs) => onChange(addRefs(value, refs))} />
    </div>
  )
}

/** The chips alone (read-only unless `onRemove`): what the detail page and the create sheet show. */
export function AttachmentChips({ items, onRemove }: { items: readonly AttachRef[]; onRemove?: (r: AttachRef) => void }) {
  return (
    <>
      {items.map((r) => (
        <AttachChip key={key(r)} item={r} onRemove={onRemove && (() => onRemove(r))} />
      ))}
    </>
  )
}

/** The board card's paperclip: `📎 N`, nothing when empty. */
export function AttachmentCount({ items }: { items: readonly AttachRef[] }) {
  if (items.length === 0) return null
  return (
    <Badge variant="outline" className="text-fg-secondary" aria-label={`${items.length} attachment${items.length === 1 ? '' : 's'}`}>
      <Paperclip /> {items.length}
    </Badge>
  )
}
