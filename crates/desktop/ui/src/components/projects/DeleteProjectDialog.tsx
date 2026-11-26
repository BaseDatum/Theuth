import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import type { ProjectDeleted } from '@/rpc/bindings/method.agent.output'
import { type ProjectCounts, itemsText, useDeleteProject } from './queries'

/**
 * 223: `Delete project` — drops the project and detaches its items: chats, tasks, notes, notebooks and meetings stay,
 * without the label. Schedule files keep their `project:` key.
 */
export function DeleteProjectDialog({
  name,
  counts,
  open,
  onOpenChange,
  onDeleted,
}: {
  name: string
  counts: ProjectCounts
  open: boolean
  onOpenChange(open: boolean): void
  onDeleted?(d: ProjectDeleted): void
}) {
  const del = useDeleteProject()
  const items = itemsText(counts)
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete {name}?</AlertDialogTitle>
          <AlertDialogDescription>
            Deletes the project only; nothing in it is deleted. {items ? `Its ${items} stay` : 'Its chats, tasks and notes stay'}, detached from
            the project, along with any notebooks and meetings that carry it. Routines keep their project setting.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Keep</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={del.isPending}
            onClick={() => del.mutate(name, { onSuccess: (d) => onDeleted?.(d) })}
          >
            Delete project
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
