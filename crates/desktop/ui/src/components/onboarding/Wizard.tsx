import './wizard.css'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { useRef, useState } from 'react'
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
/** The numbered setup steps; Welcome has no number. */
const SETUP: readonly WizardStep[] = WIZARD_STEPS.filter((s) => s !== 'welcome')

/** A first send's ack is one round trip, as the composer's (`Composer.tsx` `deadline`): longer means a dead socket. */
const SEND_TIMEOUT_MS = 15_000
async function deadline<T>(p: Promise<T>): Promise<T> {
  const late = Promise.withResolvers<never>()
  const timer = window.setTimeout(() => late.reject(new Error('Not sent: no reply from the engine')), SEND_TIMEOUT_MS)
  try {
    return await Promise.race([p, late.promise])
  } finally {
    clearTimeout(timer)
  }
}

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
  // The chat a failed first send created, kept for the retry; the same prompt reuses its input id, which the engine
  // dedupes, so a send that did land is not run twice.
  const created = useRef<{ id: string; text: string; input: string } | null>(null)
  const start = async (text: string) => {
    const id = created.current?.id ?? (await api.new_session())
    const input = created.current?.text === text ? created.current.input : crypto.randomUUID()
    created.current = { id, text, input }
    const { loadHistory, markSent, unmarkSent } = useSessions.getState()
    // Seed before sending, as the composer does: the chat shows the message at once and the route does not refetch
    // an empty history over the first turn. A failed send takes the message back out.
    loadHistory(id, { ...FRESH, model: info?.default_model ?? '', effort: info?.default_effort ?? null }, 'replace')
    markSent(id, text, [])
    try {
      await deadline(api.send_input(id, text, [], null, input))
    } catch (e) {
      unmarkSent(id, text)
      throw e
    }
    created.current = null
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
  const index = SETUP.indexOf(step)
  return (
    <Dialog open={open} onOpenChange={(o) => !o && finish()}>
      <DialogContent
        showCloseButton={false}
        onPointerDownOutside={(e) => e.preventDefault()}
        // Focus lands on the step's primary action, or (while it is disabled) on the dialog itself: never on Skip setup,
        // the first control in reading order.
        onOpenAutoFocus={(e) => {
          const content = e.currentTarget as HTMLElement | null
          e.preventDefault()
          ;(content?.querySelector<HTMLElement>('[data-primary] button:not(:disabled)') ?? content)?.focus()
        }}
        className="onb-pop flex h-[min(720px,calc(100dvh-2rem))] w-[min(1080px,calc(100vw-2rem))] max-w-none flex-col gap-0 overflow-hidden rounded-2xl border-border-subtle bg-layer-1 p-0 shadow-2xl sm:max-w-none"
      >
        <div className="flex shrink-0 items-center justify-between px-6 pt-5 pb-3 sm:px-12 sm:pt-7">
          <ol aria-label={index < 0 ? STEP_LABEL[step] : `Step ${index + 1} of ${SETUP.length}: ${STEP_LABEL[step]}`} className="flex items-center gap-1.5">
            {SETUP.map((s, i) => (
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
        <div key={step} className="onb-rise flex min-h-0 flex-1 flex-col">
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
