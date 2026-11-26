// "+ Add → App view…": the common app views (task graph first, then the view catalog), a project for the task graph,
// or any app view path. Picks a path `viewCardPath` accepts; the shell places the `view` card.

import { ChevronLeft } from 'lucide-react'
import { useState } from 'react'
import { useBoardProjects } from '@/components/tasks/queries'
import { notify } from '@/lib/notify'
import { availableViews } from '@/lib/view-catalog'
import { taskGraphPath, viewCardPath, viewCardTitle } from './view-card'

type Step = 'views' | 'project' | 'path'

const row = 'wb-press flex min-h-11 w-full items-center gap-2 rounded-xl px-3 text-left text-sm'

/** The catalog's views a card can show, the task graph first (it takes a project), the task board next. */
function commonViews(): { path: string; title: string; description: string }[] {
  const views = availableViews().filter((v) => v.path !== '/tasks/graph' && viewCardPath(v.path) !== null && !v.path.startsWith('/whiteboards'))
  const rank = (p: string) => (p === '/tasks' ? 0 : p === '/agents' ? 1 : 2)
  return views.sort((a, b) => rank(a.path) - rank(b.path))
}

function Projects({ onPick }: { onPick(path: string): void }) {
  const projects = useBoardProjects()
  return (
    <ul className="flex flex-col overflow-y-auto">
      <li>
        <button type="button" className={row} onClick={() => onPick(taskGraphPath(null))}>
          <span className="min-w-0 flex-1 truncate">All projects</span>
        </button>
      </li>
      {projects.map((p) => (
        <li key={p}>
          <button type="button" className={row} onClick={() => onPick(taskGraphPath(p))}>
            <span className="min-w-0 flex-1 truncate">{p}</span>
          </button>
        </li>
      ))}
    </ul>
  )
}

export function ViewPicker({ onPick, onDone }: { onPick(path: string, title: string): void; onDone(): void }) {
  const [step, setStep] = useState<Step>('views')
  const [value, setValue] = useState('')
  const pick = (path: string) => {
    onPick(path, viewCardTitle(path))
    onDone()
  }
  const title = step === 'project' ? 'Task graph — which project?' : step === 'path' ? 'Any app view' : 'Add an app view'
  return (
    <div className="absolute inset-0 z-50 grid place-items-center bg-black/20" onPointerDown={(e) => e.target === e.currentTarget && onDone()}>
      <div role="dialog" aria-label={title} className="wb-glass flex max-h-[70%] w-[min(360px,calc(100vw-32px))] flex-col gap-2 rounded-2xl p-3" onKeyDown={(e) => e.key === 'Escape' && onDone()}>
        <h2 className="flex items-center gap-1 px-1 text-sm font-semibold">
          {step !== 'views' && (
            <button type="button" aria-label="Back" className="grid size-7 place-items-center rounded-full" onClick={() => setStep('views')}>
              <ChevronLeft className="size-4" />
            </button>
          )}
          {title}
        </h2>
        {step === 'views' && (
          <ul className="flex flex-col overflow-y-auto">
            <li>
              <button type="button" className={row} onClick={() => setStep('project')}>
                <span className="min-w-0 flex-1 truncate">Task graph</span>
                <span className="wb-muted text-xs">pick a project</span>
              </button>
            </li>
            {commonViews().map((v) => (
              <li key={v.path}>
                <button type="button" className={row} title={v.description} onClick={() => pick(v.path)}>
                  <span className="min-w-0 flex-1 truncate">{v.path === '/tasks' ? 'Task board' : v.title}</span>
                  <span className="wb-muted text-xs">{v.path}</span>
                </button>
              </li>
            ))}
            <li>
              <button type="button" className={row} onClick={() => setStep('path')}>
                <span className="min-w-0 flex-1 truncate">Other path…</span>
              </button>
            </li>
          </ul>
        )}
        {step === 'project' && <Projects onPick={pick} />}
        {step === 'path' && (
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              const path = viewCardPath(value)
              if (!path) return notify.warning('That is not an app view path (try /tasks or /agents/<name>)')
              pick(path)
            }}
          >
            {/* biome-ignore lint/a11y/noAutofocus: the step exists to take this one value */}
            <input autoFocus value={value} onChange={(e) => setValue(e.target.value)} placeholder="/tasks/graph?project=…" className="wb-well min-w-0 flex-1 rounded-lg px-3 py-2 text-sm outline-none" />
            <button type="submit" className="rounded-lg bg-fg-accent px-3 text-sm font-semibold text-primary-foreground hover:bg-fg-accent-hover">
              Add
            </button>
          </form>
        )}
      </div>
    </div>
  )
}
