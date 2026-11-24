// 215: when the getting-started guides (sidebar checklist, Library overview strip, the tour's checklist step) show.
// Light on purpose: pages that render the Library strip import this, not the wizard or the checklist card.

import { useSidebar } from '@/components/ui/sidebar'
import { onboardingHere } from '@/lib/onboarding/detect'
import { useOnboarding, useOnboardingCompleted } from '@/lib/onboarding/state'
import { isPopout } from '@/lib/popout'
import { isIsland, isOverlay } from '@/lib/tauri'
import { LOCAL, hasLocal, useEngines } from '@/link/engines'

/**
 * The desktop shell's main window talking to its own engine (`onboardingHere`, as `OnboardingHost` decides it).
 * Without a local engine nothing else is asked, so a page rendered with no window (a server render, a test) is fine.
 */
export function useGuidesHere(): boolean {
  const active = useEngines((s) => s.active)
  if (!hasLocal()) return false
  return onboardingHere({ hasLocal: true, active, local: LOCAL, popout: isPopout(), island: isIsland(), overlay: isOverlay() })
}

/**
 * The guides are up: onboarding finished or skipped (the engine flag), the wizard closed, the checklist not hidden, and
 * the expanded sidebar on a desktop. Call only where `useGuidesHere()` holds.
 */
export function useChecklistShown(): boolean {
  const { isMobile, state } = useSidebar()
  const dismissed = useOnboarding((s) => s.checklistDismissed)
  const wizardOpen = useOnboarding((s) => s.wizardOpen)
  const { completed } = useOnboardingCompleted()
  return completed && !wizardOpen && !dismissed && !isMobile && state === 'expanded'
}
