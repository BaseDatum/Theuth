import { withCapability } from '@/rpc/capabilities'
import { panelRequirements as requirements } from '@/components/meeting/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { ControlCenter } from '@/components/meeting/ControlCenter'
import { PoppedMeetingNote } from '@/components/meeting/Note'
import { POPOUT } from '@/lib/popout'
import { useScope } from '@/store/engine-scope'

/** 53 §4.3: the control center while recording; the manifest after. Popped with `?pane=note`: the meeting's note alone. */
export const Route = createFileRoute('/meetings/$id')({ component: withCapability(Page, requirements) })

function Page() {
  const { id } = Route.useParams()
  const { connection } = useScope()
  // 156 D7, dev only: `?sim=meeting-live|meeting-review|meeting-done` answers this meeting from `dev/meeting-page-fixture.ts`.
  // The page renders once the fixture's `Api` is in place, so its first `meeting_get` never reaches the engine.
  const sim = import.meta.env.DEV && /^meeting-(live|review|done)$/.test(new URLSearchParams(location.search).get('sim') ?? '')
  const [ready, setReady] = useState(!sim)
  useEffect(() => {
    if (!sim) return
    let stop: (() => void) | undefined
    let gone = false
    // Dynamic on purpose: the fixture is a dev module that must stay out of the production bundle (like `lib/talk-sim.ts`).
    void import('@/dev/meeting-page-fixture').then((m) => {
      const mode = m.meetingPageSim()
      if (gone || !mode) return
      stop = m.install(connection, mode)
      setReady(true)
    })
    return () => {
      gone = true
      stop?.()
    }
  }, [connection, sim])
  if (!ready) return null
  return POPOUT?.kind === 'meeting-note' ? <PoppedMeetingNote id={id} /> : <ControlCenter id={id} />
}
