// 215: first-run onboarding client state. The wizard's step, the checklist's dismissal, the sections visited, where
// theuth runs (224, iPhone/iPad) and the local mirror of the engine flag `onboarding.completed` live in localStorage
// `theuth-onboarding-v1`; the open wizard, the open "where" question and the pending tour are session state and never stored.
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect } from 'react'
import { create } from 'zustand'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { ConfigFlag, ConfigFlagKey } from '@/rpc/bindings/method.rpc_2dcompose.output'

export type ChecklistId = 'model' | 'chat' | 'inbox' | 'costs' | 'library' | 'phone' | 'channel'
export type TourId = 'main' | 'inbox' | 'library' | 'costs' | 'devices' | 'chat'
export type WizardStep = 'welcome' | 'model' | 'default-model' | 'first-chat'
export type RunsOn = 'device' | 'cloud'
type Visit = 'inbox' | 'costs' | 'library'

export interface OnboardingState {
  wizardOpen: boolean
  step: WizardStep
  /** Mirror of the engine flag `onboarding.completed` (or the local fallback when the engine lacks it). */
  completed: boolean
  checklistDismissed: boolean
  visited: Partial<Record<Visit, true>>
  /** Tour to start once its first anchor mounts. */
  pendingTour: TourId | null
  /** 224: the iPhone/iPad first run's answer to "Where should theuth run?"; `null` until the user picks. */
  runsOn: RunsOn | null
  /** That question on screen (session state). */
  whereOpen: boolean
  openWizard(step?: WizardStep): void
  closeWizard(): void
  setStep(s: WizardStep): void
  markVisited(k: Visit): void
  dismissChecklist(): void
  showChecklist(): void
  /** Sets `pendingTour`. */
  requestTour(id: TourId): void
  clearPendingTour(): void
  reset(): void
}

export const ONBOARDING_KEY = 'theuth-onboarding-v1'
export const WIZARD_STEPS: readonly WizardStep[] = ['welcome', 'model', 'default-model', 'first-chat']

/** The stored part of the state; the wizard's step is per run (every open names its step). */
export type StoredOnboarding = Pick<OnboardingState, 'completed' | 'checklistDismissed' | 'visited' | 'runsOn'>
export const ONBOARDING_DEFAULT: StoredOnboarding = { completed: false, checklistDismissed: false, visited: {}, runsOn: null }

/** Parse the stored JSON; garbage or missing parts fall back to the defaults, unknown visit keys are dropped. */
export function readOnboarding(raw: string | null): StoredOnboarding {
  let v: unknown = null
  try {
    v = raw === null ? null : JSON.parse(raw)
  } catch {
    v = null
  }
  if (typeof v !== 'object' || v === null) return ONBOARDING_DEFAULT
  const o = v as Record<string, unknown>
  const d = ONBOARDING_DEFAULT
  const visited: Partial<Record<Visit, true>> = {}
  if (typeof o.visited === 'object' && o.visited !== null) {
    for (const k of ['inbox', 'costs', 'library'] as const) if ((o.visited as Record<string, unknown>)[k] === true) visited[k] = true
  }
  return {
    completed: typeof o.completed === 'boolean' ? o.completed : d.completed,
    checklistDismissed: typeof o.checklistDismissed === 'boolean' ? o.checklistDismissed : d.checklistDismissed,
    visited,
    runsOn: o.runsOn === 'device' || o.runsOn === 'cloud' ? o.runsOn : d.runsOn,
  }
}

export const useOnboarding = create<OnboardingState>()((set) => ({
  // `globalThis.` so the module loads under vitest's node environment.
  ...readOnboarding(globalThis.localStorage?.getItem(ONBOARDING_KEY) ?? null),
  step: 'welcome',
  wizardOpen: false,
  pendingTour: null,
  whereOpen: false,
  openWizard: (step = 'welcome') => set({ wizardOpen: true, step }),
  closeWizard: () => set({ wizardOpen: false }),
  setStep: (step) => set({ step }),
  markVisited: (k) => set((s) => (s.visited[k] ? s : { visited: { ...s.visited, [k]: true } })),
  dismissChecklist: () => set({ checklistDismissed: true }),
  showChecklist: () => set({ checklistDismissed: false }),
  requestTour: (pendingTour) => set({ pendingTour }),
  clearPendingTour: () => set({ pendingTour: null }),
  // The engine flag is not local state: `completed` keeps mirroring it, and finishing or skipping the wizard writes it.
  // Where theuth runs is a choice, not tour progress: it stays too.
  reset: () => set((s) => ({ ...ONBOARDING_DEFAULT, step: 'welcome', visited: {}, completed: s.completed, runsOn: s.runsOn, pendingTour: null })),
}))

useOnboarding.subscribe((s, prev) => {
  if (s.completed === prev.completed && s.checklistDismissed === prev.checklistDismissed && s.visited === prev.visited && s.runsOn === prev.runsOn) return
  const stored: StoredOnboarding = { completed: s.completed, checklistDismissed: s.checklistDismissed, visited: s.visited, runsOn: s.runsOn }
  try {
    globalThis.localStorage?.setItem(ONBOARDING_KEY, JSON.stringify(stored))
  } catch {
    // Storage full or blocked: the state still holds for this run.
  }
})

const FLAG: ConfigFlagKey = 'onboarding.completed'
export const onboardingFlagQuery = ['config-flag', FLAG] as const

/**
 * The engine flag with its load state, for the boot decision: `loaded` once the engine answered, failed, or cannot
 * answer (no `config_flag_get`). An engine without the flag leaves the local mirror as the answer.
 */
export function useOnboardingFlag(): { completed: boolean; loaded: boolean; setCompleted(v: boolean): Promise<void> } {
  const supported = useQuerySupport(['config_flag_get'])
  const api = useApi()
  const qc = useQueryClient()
  const local = useOnboarding((s) => s.completed)
  const q = useQuery<ConfigFlag>({ enabled: supported, queryKey: onboardingFlagQuery, queryFn: () => api.config_flag_get(FLAG), retry: false, staleTime: Infinity })
  const remote = q.data?.enabled
  useEffect(() => {
    if (remote !== undefined && remote !== useOnboarding.getState().completed) useOnboarding.setState({ completed: remote })
  }, [remote])
  const setCompleted = useCallback(async (v: boolean) => {
    useOnboarding.setState({ completed: v })
    if (!supported || qc.getQueryState(onboardingFlagQuery)?.status === 'error') return
    try {
      qc.setQueryData(onboardingFlagQuery, await api.config_flag_set(FLAG, v))
    } catch {
      // Older engine: the local mirror is the record.
    }
  }, [api, qc, supported])
  return { completed: remote ?? local, loaded: !supported || q.isError || q.isSuccess, setCompleted }
}

export function useOnboardingCompleted(): { completed: boolean; setCompleted(v: boolean): Promise<void> } {
  const { completed, setCompleted } = useOnboardingFlag()
  return { completed, setCompleted }
}
