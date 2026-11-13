import { Link } from '@tanstack/react-router'
import { X } from 'lucide-react'
import { useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import type { Task } from "@/rpc/bindings/method.roster.output"
import { TaskPicker } from './TaskPicker'
import { shortId } from './helpers'

/**
 * Editable chips of related tasks (69: *Blocked by* and *Blocks* side by side): each chip opens the other task, has
 * a remove button, and "Add task…" opens the picker scoped to `project`. `exclude` hides ids that may not be picked.
 */
export function TaskChips({
  title,
  tasks,
  project,
  exclude = [],
  disabled,
  onAdd,
  onRemove,
}: {
  title: string
  tasks: readonly Task[]
  project: string | null
  exclude?: readonly string[]
  disabled?: boolean
  onAdd(t: Task): void
  onRemove(t: Task): void
}) {
  const [picking, setPicking] = useState(false)
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {tasks.map((b) => (
        <Badge key={b.id} variant="secondary" className="max-w-full gap-1 pr-1">
          <Link to="/tasks/$id" params={{ id: b.id }} className="truncate hover:underline">
            {b.title}
          </Link>
          <span className="text-fg-tertiary">{shortId(b.id)}</span>
          <button type="button" aria-label={`Remove ${b.title}`} className="rounded-full hover:bg-layer-2" disabled={disabled} onClick={() => onRemove(b)}>
            <X className="size-3" />
          </button>
        </Badge>
      ))}
      <Button type="button" variant="outline" size="sm" disabled={disabled} onClick={() => setPicking(true)}>
        Add task…
      </Button>
      <TaskPicker
        open={picking}
        onOpenChange={setPicking}
        title={title}
        project={project}
        exclude={[...exclude, ...tasks.map((b) => b.id)]}
        onPick={onAdd}
      />
    </div>
  )
}
