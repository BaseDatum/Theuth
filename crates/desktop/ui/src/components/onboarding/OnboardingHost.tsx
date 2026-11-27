import { useEffect, useRef } from 'react'
import { CloudNodeWizard } from '@/components/cloud/CloudNodeWizard'
import { CLOUD_REQUIRED } from '@/components/cloud/queries'
import { notify as toast } from '@/lib/notify'
import { useModels } from '@/components/agents/queries'
import { bootDecision, onboardingHere } from '@/lib/onboarding/detect'
import { useOnboarding, useOnboardingFlag } from '@/lib/onboarding/state'
import { isIos } from '@/lib/platform'
import { isPopout } from '@/lib/popout'
import { isIsland, isOverlay } from '@/lib/tauri'
import { LOCAL, hasLocal, useEngines } from '@/link/engines'
import { useQuerySupport, useSupported } from '@/rpc/capabilities'
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
      <WhereToRun />
      <Wizard />
      <LinkPhoneDialog />
      <TourRunner />
    </>
  )
}

/**
 * Acts once per run, as soon as the flag, the model list and `info` have answered ("Trigger and persistence"). Only an
 * acted-on decision latches: a failed `list_models` waits for its refetch (focus, reconnect) instead of deciding nothing.
 */
function Boot() {
  const flag = useOnboardingFlag()
  const canModels = useQuerySupport(['list_models'])
  const models = useModels()
  const defaultModel = useConnection((s) => s.info?.default_model)
  const cloud = useSupported(CLOUD_REQUIRED)
  const decided = useRef(false)
  const { setCompleted } = flag
  const decision = bootDecision({
    here: true,
    flagLoaded: flag.loaded,
    completed: flag.completed,
    models: !canModels ? null : models.isError ? undefined : models.data,
    defaultModel,
  })
  useEffect(() => {
    if (decided.current || (decision !== 'wizard' && decision !== 'toast')) return
    decided.current = true
    if (decision === 'wizard') {
      const s = useOnboarding.getState()
      // 224: the iPhone/iPad asks where theuth runs first, once, where its engine can create cloud nodes; "Just this
      // iPhone" goes on to the wizard below.
      if (s.wizardOpen || s.whereOpen) return
      if (isIos() && cloud && s.runsOn === null) useOnboarding.setState({ whereOpen: true })
      else s.openWizard('welcome')
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
  }, [decision, setCompleted, cloud])
  return null
}

/**
 * 224: the iPhone/iPad first run's "Where should theuth run?" (the cloud wizard's first-run entry), over the shell.
 * "Just this iPhone" is remembered and opens today's setup at Welcome; a created node is remembered and the wizard
 * itself goes on to Settings › Providers ("Connect a model"). Closing it without a pick opens Welcome and asks again
 * next launch.
 */
function WhereToRun() {
  const open = useOnboarding((s) => s.whereOpen)
  if (!open) return null
  const welcome = () => {
    useOnboarding.setState({ whereOpen: false })
    useOnboarding.getState().openWizard('welcome')
  }
  return (
    <div className="fixed inset-0 z-40 overflow-y-auto bg-background">
      <CloudNodeWizard
        entry="first-run"
        onClose={welcome}
        onDone={(node) => {
          useOnboarding.setState({ runsOn: node ? 'cloud' : 'device' })
          if (node) useOnboarding.setState({ whereOpen: false })
          else welcome()
        }}
      />
    </div>
  )
}
