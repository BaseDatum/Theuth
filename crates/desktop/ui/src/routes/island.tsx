import { createFileRoute } from '@tanstack/react-router'
import { useEffect } from 'react'
import { Island } from '@/components/meeting/Island'
import { LiveBadge, useLiveBadge } from '@/components/public-share/LiveBadge'
import { useCompanionAlive } from '@/lib/companion'
import { VoicePanel } from '@/components/voice/VoicePanel'
import { islandHide, pointRefuse } from '@/lib/tauri'
import { useVoiceKey } from '@/lib/voice-key'
import { live, useMeeting } from '@/store/meeting'

/** 03 §0.1: the island window's only route (memory history in that webview; `/island?fixture=meeting` in a browser replays the dev feed).
 * 96 §0.3: the same window is the voice panel while the voice key is live; the meeting island otherwise.
 * 210 §7.4 item 6: with no meeting on it, a share active and the main window hidden, the `● LIVE N · Stop` badge. */
export const Route = createFileRoute('/island')({
  validateSearch: (s: Record<string, unknown>): { fixture?: string } =>
    typeof s.fixture === 'string' ? { fixture: s.fixture } : {},
  component: IslandRoute,
})

function IslandRoute() {
  const { fixture } = Route.useSearch()
  const voice = useVoiceKey((s) => s.phase !== 'idle')
  const meeting = useMeeting((s) => live(s.state))
  // The states `Island` shows itself in (it hides otherwise).
  const onIsland = useMeeting((s) => live(s.state) || s.state === 'Starting' || s.state === 'Stopping')
  const badge = useLiveBadge(onIsland)
  // 149 D15: nothing to draw while the companion owns the island and the voice key.
  const companion = useCompanionAlive()
  useEffect(() => {
    if (companion) void islandHide()
  }, [companion])
  // A recording meeting leases the `Utterer`: the shell answers the next press with `point:refused` instead of `Start`.
  useEffect(() => {
    void pointRefuse(meeting ? 'meeting' : null)
  }, [meeting])
  useEffect(() => {
    if (!import.meta.env.DEV || (fixture !== 'meeting' && !import.meta.env.VITE_MEETING_FIXTURE)) return
    // The import resolves after StrictMode's first cleanup: a replay started then would run twice.
    let stop: (() => void) | null = () => {}
    void import('@/dev/meeting-fixture').then((m) => {
      if (stop) stop = m.play()
    })
    return () => {
      stop?.()
      stop = null
    }
  }, [fixture])
  if (companion) return null
  return voice ? <VoicePanel /> : badge !== null ? <LiveBadge count={badge} /> : <Island />
}
