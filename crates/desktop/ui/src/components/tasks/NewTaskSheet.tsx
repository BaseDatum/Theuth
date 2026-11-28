import { HARNESSES, ProjectCombobox, TaskModal } from './TaskModal'

export { HARNESSES, ProjectCombobox }

/**
 * Compatibility wrapper for the centered Option A `TaskModal` in create mode so `Board` and existing callers open
 * the focused task modal with zero breakage.
 */
export function NewTaskSheet({
  open,
  onOpenChange,
  project,
}: {
  open: boolean
  onOpenChange(open: boolean): void
  project: string | null
}) {
  return <TaskModal mode="create" open={open} onOpenChange={onOpenChange} project={project} />
}
