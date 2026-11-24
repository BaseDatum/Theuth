import { useEffect, useRef } from 'react'
import { toast } from 'sonner'
import { useModels } from '@/components/agents/queries'
import { bootDecision, onboardingHere } from '@/lib/onboarding/detect'
import { useOnboarding, useOnboardingFlag } from '@/lib/onboarding/state'
import { isPopout } from '@/lib/popout'
import { isIsland, isOverlay } from '@/lib/tauri'
import { LOCAL, hasLocal, useEngines } from '@/link/engines'
import { useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'
import { LinkPhoneDialog } from './LinkPhoneDialog'
import { TourRunner } from './TourRunner'
import { Wizard } from './Wizard'

/** Onboarding runs only in the desktop shell's main window, talking to its own engine. */
export function useOnboardingHere(): boolean {
  const active = useEngines((s) => s.active)
  return onboardingHere({ hasLocal: hasLocal(), active, local: LOCAL, popout: isPopout(), island: isIsland(), overlay: isOverlay() })
}

/** 215: the boot decision, the wizard, the tour runner and the phone dialog; nothing anywhere but the local desktop shell. */
export function OnboardingHost() {
  if (!useOnboardingHere()) return null
  return (
    <>
      <Boot />
      <Wizard />
      <LinkPhoneDialog />
      <TourRunner />
    </>
  )
}

/** Decides once per run, as soon as the flag, the model list and `info` have answered ("Trigger and persistence"). */
function Boot() {
  const flag = useOnboardingFlag()
  const canModels = useQuerySupport(['list_models'])
  const models = useModels()
  const defaultModel = useConnection((s) => s.info?.default_model)
  const decided = useRef(false)
  const { setCompleted } = flag
  const decision = bootDecision({
    here: true,
    flagLoaded: flag.loaded,
    completed: flag.completed,
    models: !canModels || models.isError ? null : models.data,
    defaultModel,
  })
  useEffect(() => {
    if (decided.current || decision === 'wait') return
    decided.current = true
    if (decision === 'wizard') {
      if (!useOnboarding.getState().wizardOpen) useOnboarding.getState().openWizard('welcome')
    } else if (decision === 'toast') {
      toast('New: a 2-minute tour of theuth', {
        id: 'onboarding-tour',
        description: 'See where your Inbox, Library, costs and settings live.',
        duration: Number.POSITIVE_INFINITY,
        action: {
          label: 'Take the tour',
          onClick: () => {
            void setCompleted(true)
            useOnboarding.getState().requestTour('main')
          },
        },
        cancel: { label: 'Dismiss', onClick: () => void setCompleted(true) },
      })
    }
  }, [decision, setCompleted])
  return null
}
