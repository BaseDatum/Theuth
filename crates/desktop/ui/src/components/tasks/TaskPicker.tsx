import { CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import type { Task } from '@/rpc/bindings/Task'
import { ownerLabel, shortId } from './helpers'
import { useTasks } from './queries'

/** Pick one open task of `project` by title (the New task sheet's "blocked by"); `exclude` hides ids already chosen. */
export function TaskPicker({
  open,
  onOpenChange,
  project,
  exclude,
  onPick,
}: {
  open: boolean
  onOpenChange(open: boolean): void
  project: string | null
  exclude: readonly string[]
  onPick(task: Task): void
}) {
  const tasks = useTasks({ owner: null, state: ['todo', 'doing', 'review', 'blocked'], engine: null, project })
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Blocked by" description="Pick the task this one waits on">
      <CommandInput placeholder="Search tasks by title…" autoFocus />
      <CommandList>
        <CommandEmpty>{tasks.isPending ? 'Loading…' : tasks.isError ? tasks.error.message : 'No matching task'}</CommandEmpty>
        <CommandGroup>
          {tasks.data
            ?.filter((t) => !exclude.includes(t.id))
            .map((t) => (
              <CommandItem
                key={t.id}
                value={`${t.title} ${t.id}`}
                onSelect={() => {
                  onPick(t)
                  onOpenChange(false)
                }}
              >
                <span className="flex-1 truncate">{t.title}</span>
                <span className="text-xs text-fg-tertiary">
                  {t.state} · {ownerLabel(t.owner)} · {shortId(t.id)}
                </span>
              </CommandItem>
            ))}
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  )
}
