import type { SessionKind } from '@/rpc/bindings/method.agent.output'
import { cn } from '@/lib/utils'

/** What decides whether the "What's next?" chips show: the session's slice fields and the composer's own state. */
export type NextStepsGate = {
  suggestions?: readonly string[]
  running: boolean
  kind?: SessionKind
  parent?: string | null
  /** The box holds typed text, held pastes or parked skills: the user is writing their own message. */
  drafting: boolean
  /** A send is in flight, or the box takes no input (the talk view). */
  blocked: boolean
}

/**
 * The suggestions to show, or `null`: never while a turn runs, never on a watch-only (observed) or child session,
 * never once the user has started a message of their own or a send is in flight.
 */
export function nextSteps(g: NextStepsGate): readonly string[] | null {
  if (!g.suggestions?.length || g.running || g.drafting || g.blocked) return null
  if (g.kind === 'observed' || g.kind === 'advisor' || g.parent) return null
  return g.suggestions
}

/**
 * "What's next?" (after the last turn, right above the input): the title model's next steps as chips. A pick is
 * sent exactly as if typed and sent from the composer (`onPick` is the composer's submit). Long ones truncate; the
 * full text is the tooltip. Tap targets are at least 36 px tall so they work on a phone.
 */
export function NextSteps({ items, onPick, compact = false }: { items: readonly string[]; onPick: (text: string) => void; compact?: boolean }) {
  return (
    <div data-next-steps="" className="mb-2 flex flex-col gap-1.5 px-1">
      <span className="text-xs text-fg-tertiary">What's next?</span>
      <div className="flex flex-wrap gap-2">
        {items.map((item) => (
          <button
            key={item}
            type="button"
            title={item}
            data-next-step={item}
            onClick={() => onPick(item)}
            className={cn(
              'min-h-9 max-w-full truncate rounded-full border border-border-subtle bg-layer-2 px-3 text-left text-sm text-fg-secondary transition-colors',
              'hover:bg-layer-3 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
              compact && 'min-h-8 text-xs',
            )}
          >
            {item}
          </button>
        ))}
      </div>
    </div>
  )
}
