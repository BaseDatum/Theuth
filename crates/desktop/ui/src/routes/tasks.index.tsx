import { createFileRoute } from '@tanstack/react-router'
import { Board } from '@/components/tasks/Board'
import { STATES } from '@/components/tasks/helpers'
import type { TaskState } from '@/rpc/bindings/TaskState'

/** Board filters live in the URL so a board survives reload and can be linked. `project: ''` = all projects. */
export type BoardSearch = { project?: string; agent?: string; engine?: string; state?: TaskState; cancelled?: boolean }

export const Route = createFileRoute('/tasks/')({
  validateSearch: (s: Record<string, unknown>): BoardSearch => ({
    ...(typeof s.project === 'string' && { project: s.project }),
    ...(typeof s.agent === 'string' && s.agent !== '' && { agent: s.agent }),
    ...(typeof s.engine === 'string' && s.engine !== '' && { engine: s.engine }),
    ...(typeof s.state === 'string' && (STATES as readonly string[]).includes(s.state) && { state: s.state as TaskState }),
    ...(s.cancelled === true && { cancelled: true }),
  }),
  component: Board,
})
