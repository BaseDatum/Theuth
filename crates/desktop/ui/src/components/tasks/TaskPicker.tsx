import { CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from '@/components/ui/command'
import type { Task } from "@/rpc/bindings/method.roster.output"
import { ownerLabel, shortId } from './helpers'
import { useTasks } from './queries'

/** Pick one open task of `project` by title (blocked by / blocks); `exclude` hides ids already chosen. */
export function TaskPicker({
  open,
  onOpenChange,
  title,
  project,
  exclude,
  onPick,
}: {
  open: boolean
  onOpenChange(open: boolean): void
  title: string
  project: string | null
  exclude: readonly string[]
  onPick(task: Task): void
}) {
  const tasks = useTasks({ owner: null, state: ['backlog', 'todo', 'doing', 'review', 'blocked'], engine: null, project })
  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title={title} description="Pick a task">
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
