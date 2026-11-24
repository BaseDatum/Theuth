import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/routines/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { lazy, Suspense, useEffect } from 'react'
import { SectionBand } from '@/components/nav/SectionBand'
import { SectionRow } from '@/components/nav/SectionRow'
import { Skeleton } from '@/components/ui/skeleton'
import { useRoutineFile, useRoutines, useSensors } from '@/components/routines/queries'
import { isPopout } from '@/lib/popout'

const RoutineGraph = lazy(() => import('@/components/routines/graph/RoutineGraph'))

/** 214 D9: `/routines/$name/graph` — the routine's graph alone, full height: the graph's pop-out window (one per routine,
 * `routineGraphPopout`). A static segment, so it outranks `/routines/$name/$section`. Live like the Overview: the list
 * refetches on every `schedule.events` event. */
export const Route = createFileRoute('/routines/$name/graph')({
  component: withCapability(Page, requirements),
})

function Page() {
  const { name } = Route.useParams()
  const routines = useRoutines()
  const file = useRoutineFile(name)
  const sensors = useSensors().data
  const info = routines.data?.find((s) => s.name === name)
  useEffect(() => {
    if (isPopout()) document.title = name
  }, [name])
  const error = routines.error ?? file.error
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="agents" />
      <SectionRow back={isPopout() ? undefined : { to: '/routines', label: 'Back to routines' }} title={name} />
      <div className="relative min-h-0 flex-1 overflow-hidden bg-background">
        {info && file.data ? (
          <Suspense fallback={<div className="size-full bg-background" />}>
            <RoutineGraph variant="window" info={info} file={file.data} sensors={sensors} chat={undefined} />
          </Suspense>
        ) : error ? (
          <p className="p-6 text-center text-sm text-danger">{error.message}</p>
        ) : routines.data && !info ? (
          <p className="p-6 text-center text-sm text-fg-tertiary">No routine named {name}.</p>
        ) : (
          <div className="flex gap-6 p-6">
            <Skeleton className="h-[120px] w-60" />
            <Skeleton className="h-[120px] w-60" />
          </div>
        )}
      </div>
    </article>
  )
}
