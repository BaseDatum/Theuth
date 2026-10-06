import { createFileRoute, redirect } from '@tanstack/react-router'
import { Board } from '@/components/tasks/Board'
import { type BoardSearch, boardEntryRedirect, parseBoardSearch, recallBoard } from '@/components/tasks/boardMemory'

export type { BoardSearch }

export const Route = createFileRoute('/tasks/')({
  validateSearch: parseBoardSearch,
  // 118: a bare `/tasks` (the band's Tasks tab, the task page's back, the navigate after cancelling a task) is entered
  // with the filters this device last had. An explicit search (AgentDetail, the Hud) and in-place filter edits on the
  // board (`cause: 'stay'`, including clearing the last filter) are untouched; hover preloads never redirect.
  beforeLoad: ({ cause, search }) => {
    const to = boardEntryRedirect(cause, search, recallBoard())
    if (to) throw redirect({ to: '/tasks', search: to, replace: true })
  },
  component: Board,
})
