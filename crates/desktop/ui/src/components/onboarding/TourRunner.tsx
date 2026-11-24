import 'driver.js/dist/driver.css'
import './tour.css'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { useEffect, useRef } from 'react'
import { notify as toast } from '@/lib/notify'
import { useSidebar } from '@/components/ui/sidebar'
import { phoneLinked } from '@/lib/onboarding/checklist'
import { useApi } from '@/lib/rpc'
import { useOnboarding } from '@/lib/onboarding/state'
import { startTour, stopTour } from '@/lib/onboarding/tours'
import { useQuerySupport } from '@/rpc/capabilities'
import { DEVICES } from './Checklist'
import { openLinkPhone } from './LinkPhoneDialog'

/**
 * 215: runs the tour the store asks for (`requestTour` sets `pendingTour`) and clears the request. `startTour` refuses
 * pop-outs, the island and overlays, and waits for the tour's anchors itself. Finishing the main tour (Done, not Skip)
 * offers to link a phone unless one is linked. Renders nothing; the overlay goes with it.
 */
export function TourRunner() {
  const pending = useOnboarding((s) => s.pendingTour)
  const navigate = useNavigate()
  const qc = useQueryClient()
  const api = useApi()
  const canPair = useQuerySupport(['device_pair_start'])
  const canList = useQuerySupport(['devices_list'])
  // Read at start, not a dependency: a checklist that cannot show (hidden, collapsed rail, phone) is skipped without a
  // wait. The completed flag is left out on purpose: the wizard sets it as it starts this tour, and the engine's
  // answer may land a moment later; the short anchor settle covers that.
  const { isMobile, state } = useSidebar()
  const dismissed = useOnboarding((s) => s.checklistDismissed)
  const noChecklist = useRef(false)
  noChecklist.current = dismissed || isMobile || state !== 'expanded'
  useEffect(() => stopTour, [])
  useEffect(() => {
    if (!pending) return
    useOnboarding.getState().clearPendingTour()
    const offerPhone = async () => {
      if (!canPair) return
      const devices = canList ? await qc.ensureQueryData({ queryKey: DEVICES, queryFn: () => api.devices_list() }).catch(() => undefined) : undefined
      if (phoneLinked(devices)) return
      toast('Want theuth on your phone?', {
        description: 'Reach your agents and answer the Inbox from anywhere.',
        action: { label: 'Link your phone', onClick: openLinkPhone },
      })
    }
    startTour(pending, navigate, {
      skip: noChecklist.current ? ['onboarding-checklist'] : [],
      onDone: pending === 'main' ? () => void offerPhone() : undefined,
    })
  }, [pending, navigate, qc, api, canPair, canList])
  return null
}
