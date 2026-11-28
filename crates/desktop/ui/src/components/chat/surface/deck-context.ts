import { createContext, useContext } from 'react'
import type { InteractResponse, RuntimeControlOp, RuntimeState } from '@/components/decks/types'

export type DeckInteractContextValue = {
  deckId: string
  scriptRev?: number | null
  runtimeState: RuntimeState
  stale: boolean
  busy: boolean
  lastResult: InteractResponse | null
  invokeAction(action: string, args: Record<string, unknown>): Promise<InteractResponse | null>
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
