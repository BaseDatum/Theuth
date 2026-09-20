import { createFileRoute } from '@tanstack/react-router'
import { useEffect } from 'react'
import { Island } from '@/components/meeting/Island'

/** 03 §0.1: the island window's only route (memory history in that webview; `/island?fixture=meeting` in a browser replays the dev feed). */
export const Route = createFileRoute('/island')({
  validateSearch: (s: Record<string, unknown>): { fixture?: string } =>
    typeof s.fixture === 'string' ? { fixture: s.fixture } : {},
  component: IslandRoute,
})

function IslandRoute() {
  const { fixture } = Route.useSearch()
  useEffect(() => {
    if (!import.meta.env.DEV || (fixture !== 'meeting' && !import.meta.env.VITE_MEETING_FIXTURE)) return
    let stop = () => {}
    void import('@/dev/meeting-fixture').then((m) => (stop = m.play()))
    return () => stop()
  }, [fixture])
  return <Island />
}
