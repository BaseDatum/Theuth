import { createContext, useContext } from 'react'
import type { InteractResponse, RuntimeControlOp, RuntimeState } from '@/components/decks/types'

export type InvokeActionOptions = {
  /** Deterministic idempotency key supplied by the caller (e.g. OpenUI press or HTML guest). */
  requestId?: string
  /** Explicit script revision fence; defaults to active runtime revision when omitted. */
  expectedRev?: number | null
  /** Timeout override in ms (default 30_000). */
  timeoutMs?: number
  /** Suppress host toast notifications on failure (caller renders private error inline). */
  silent?: boolean
  /** Keep result private to the caller without overwriting the shared Deck `lastResult` banner. */
  privateOnly?: boolean
  /** Re-throw transport/RPC errors instead of returning null. */
  throwOnError?: boolean
}

export type DeckInteractContextValue = {
  deckId: string
  scriptRev?: number | null
  runtimeState: RuntimeState
  stale: boolean
  busy: boolean
  pinned?: boolean
  lastResult: InteractResponse | null
  invokeAction(
    action: string,
    args: Record<string, unknown>,
    opts?: InvokeActionOptions,
  ): Promise<InteractResponse | null>
  controlRuntime(op: RuntimeControlOp): Promise<void>
}

/**
 * Deck interaction context for `ActionButton`, `StreamControls`, and `ResultBanner`.
 * Outside a Deck (`null`, e.g. ordinary chat `json-render` fences), those components are strictly inert.
 */
export const DeckInteractContext = createContext<DeckInteractContextValue | null>(null)

export function useDeckInteractContext(): DeckInteractContextValue | null {
  return useContext(DeckInteractContext)
}
