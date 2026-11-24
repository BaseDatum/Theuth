// 215 "Trigger and persistence": what the shell does about onboarding once it has booted.

/** `wait` until the inputs are known; `none` nothing modal; `wizard` open it at Welcome; `toast` the upgrading user's tour offer. */
export type BootDecision = 'wait' | 'none' | 'wizard' | 'toast'

export type BootInput = {
  /** Desktop shell on its own local engine, in the main window (never pop-outs, island, overlay, phones, web clients). */
  here: boolean
  /** The engine flag (or its local fallback) has an answer. */
  flagLoaded: boolean
  completed: boolean
  /** `list_models()`: `undefined` while loading, `null` when unavailable or failed. */
  models: readonly { id: string }[] | null | undefined
  /** `info().default_model`: `undefined` until `info` is known. */
  defaultModel: string | undefined
}

export function bootDecision(i: BootInput): BootDecision {
  if (!i.here) return 'none'
  if (!i.flagLoaded) return 'wait'
  if (i.completed) return 'none'
  if (i.models === undefined || i.defaultModel === undefined) return 'wait'
  // No model list to judge by: degrade to nothing rather than guess.
  if (i.models === null) return 'none'
  // An empty list, or a default whose provider is not configured (its model is not offered), means setup is due. No
  // default at all with models on offer is an engine that picks per chat: that user is set up.
  if (i.models.length === 0 || (i.defaultModel && !i.models.some((m) => m.id === i.defaultModel))) return 'wizard'
  return 'toast'
}

/** Where onboarding may run: the desktop shell talking to its own engine. */
export function onboardingHere(env: { hasLocal: boolean; active: string; local: string; popout: boolean; island: boolean; overlay: boolean }): boolean {
  return env.hasLocal && env.active === env.local && !env.popout && !env.island && !env.overlay
}
