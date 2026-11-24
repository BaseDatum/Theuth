import 'driver.js/dist/driver.css'
import './tour.css'
import { useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { useOnboarding } from '@/lib/onboarding/state'
import { startTour } from '@/lib/onboarding/tours'

/**
 * 215: runs the tour the store asks for (`requestTour` sets `pendingTour`) and clears the request. `startTour` refuses
 * pop-outs, the island and overlays, and waits for the tour's anchors itself. Renders nothing.
 */
export function TourRunner() {
  const pending = useOnboarding((s) => s.pendingTour)
  const navigate = useNavigate()
  useEffect(() => {
    if (!pending) return
    const { clearPendingTour, checklistDismissed } = useOnboarding.getState()
    clearPendingTour()
    // A hidden checklist will not appear: skip its step now instead of waiting out the anchor timeout.
    startTour(pending, navigate, { skip: checklistDismissed ? ['onboarding-checklist'] : [] })
  }, [pending, navigate])
  return null
}
