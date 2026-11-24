import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { useState } from 'react'
import { FRESH } from '@/components/chat/slash'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent } from '@/components/ui/dialog'
import { WIZARD_STEPS, type WizardStep, useOnboarding, useOnboardingCompleted } from '@/lib/onboarding/state'
import { queryKeys, useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { useSessions } from '@/store/sessions'
import { ConnectModel } from './steps/ConnectModel'
import { DefaultModel } from './steps/DefaultModel'
import { FirstChat } from './steps/FirstChat'
import { Welcome } from './steps/Welcome'

const STEP_LABEL: Record<WizardStep, string> = { welcome: 'Welcome', model: 'Connect a model', 'default-model': 'Pick your default model', 'first-chat': 'Your first chat' }

/**
 * 215 layer 1: the welcome and setup wizard, a near-full-window dialog. Esc, Skip and finishing all mark onboarding
 * done (the engine flag); a first chat closes it and asks for the main tour once the chat view mounts.
 */
export function Wizard() {
  const open = useOnboarding((s) => s.wizardOpen)
  const step = useOnboarding((s) => s.step)
  const { setStep, closeWizard, requestTour } = useOnboarding.getState()
  const { setCompleted } = useOnboardingCompleted()
  const api = useApi()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const info = useConnection((s) => s.info)
  // The provider connected in step 2 leads the recommendations in step 3.
  const [prefer, setPrefer] = useState<string | null>(null)
  const finish = () => {
    closeWizard()
    void setCompleted(true)
  }
  const start = async (text: string) => {
    const id = await api.new_session()
    const { loadHistory, markSent } = useSessions.getState()
    // Seed before sending, as the composer does: the chat shows the message at once and the route does not refetch
    // an empty history over the first turn.
    loadHistory(id, { ...FRESH, model: info?.default_model ?? '', effort: info?.default_effort ?? null }, 'replace')
    markSent(id, text, [])
    await api.send_input(id, text, [], null, crypto.randomUUID())
    void qc.invalidateQueries({ queryKey: queryKeys.sessions })
    finish()
    requestTour('main')
    await navigate({ to: '/c/$session', params: { session: id } })
  }
  const own = () => {
    finish()
    requestTour('main')
    void navigate({ to: '/new' }).then(() => requestAnimationFrame(() => window.dispatchEvent(new Event('theuth:focus-composer'))))
  }
  const index = WIZARD_STEPS.indexOf(step)
  return (
    <Dialog open={open} onOpenChange={(o) => !o && finish()}>
      <DialogContent
        showCloseButton={false}
        onPointerDownOutside={(e) => e.preventDefault()}
        className="flex h-[min(720px,calc(100dvh-2rem))] w-[min(920px,calc(100vw-2rem))] max-w-none flex-col gap-0 overflow-hidden rounded-2xl border-border-subtle bg-layer-1 p-0 shadow-2xl sm:max-w-none"
      >
        <div className="flex shrink-0 items-center justify-between px-6 pt-5 pb-3 sm:px-12 sm:pt-7">
          <ol aria-label={`Step ${index + 1} of ${WIZARD_STEPS.length}: ${STEP_LABEL[step]}`} className="flex items-center gap-1.5">
            {WIZARD_STEPS.map((s, i) => (
              <li
                key={s}
                aria-hidden
                className={`h-1.5 rounded-full transition-[width,background-color] duration-300 motion-reduce:transition-none ${i === index ? 'w-6 bg-fg-accent' : i < index ? 'w-1.5 bg-fg-accent/50' : 'w-1.5 bg-border-strong'}`}
              />
            ))}
          </ol>
          {step !== 'welcome' && (
            <Button variant="ghost" size="sm" onClick={finish} className="text-fg-tertiary">
              Skip setup
            </Button>
          )}
        </div>
        <div key={step} className="flex min-h-0 flex-1 flex-col animate-in fade-in-0 slide-in-from-bottom-2 duration-300 motion-reduce:animate-none">
          {step === 'welcome' ? (
            <Welcome onNext={() => setStep('model')} onSkip={finish} />
          ) : step === 'model' ? (
            <ConnectModel onConnected={setPrefer} onBack={() => setStep('welcome')} onNext={() => setStep('default-model')} />
          ) : step === 'default-model' ? (
            <DefaultModel prefer={prefer} onBack={() => setStep('model')} onNext={() => setStep('first-chat')} />
          ) : (
            <FirstChat onStart={start} onOwn={own} onBack={() => setStep('default-model')} />
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
