import { create } from 'zustand'
import { touch } from '@/lib/platform'
import { cn } from '@/lib/utils'
import type { SessionKind } from '@/rpc/bindings/method.agent.output'
import { useSessionField } from '@/store/sessions'

/** What decides whether the "Next:" block shows. */
export type NextStepsGate = {
  suggestions?: readonly string[]
  running: boolean
  kind?: SessionKind
  parent?: string | null
  /** The view only watches the session (no composer). */
  watching?: boolean
}

/**
 * The suggestions to show, or `null`: never while a turn runs, never on a watch-only, advisor or child session.
 * Typing, a paste or a parked skill keeps them: they go only when something is sent (the composer's submit
 * dismisses them) or a turn starts (the reducer clears them on `TurnStarted`), or a newer `Suggestions` replaces them.
 */
export function nextSteps(g: NextStepsGate): readonly string[] | null {
  if (!g.suggestions?.length || g.running || g.watching) return null
  if (g.kind === 'observed' || g.kind === 'advisor' || g.parent) return null
  return g.suggestions
}

/**
 * A picked suggestion on its way to the session's composer: the block sits above the background area, the composer
 * below it, and the send must be the composer's own (`submit`, as if typed). The composer `take`s it, so one pick is
 * sent once even when two composers show the session.
 */
type Picks = {
  by: Record<string, string>
  pick(session: string, text: string): void
  take(session: string): string | undefined
}

export const useNextPick = create<Picks>()((set, get) => ({
  by: {},
  pick: (session, text) => set((s) => ({ by: { ...s.by, [session]: text } })),
  take: (session) => {
    const text = get().by[session]
    if (text === undefined) return undefined
    set((s) => {
      const { [session]: _, ...by } = s.by
      return { by }
    })
    return text
  },
}))

/**
 * "Next:" (between the last message and the background area): the title model's next steps as plain text rows in
 * the message column. Hover or keyboard focus tints a row and shows `↵ send` (never on touch). One line each,
 * truncated, the full text as the tooltip. A pick goes to `onPick`.
 */
export function NextSteps({ items, onPick, hint = !touch() }: { items: readonly string[]; onPick: (text: string) => void; hint?: boolean }) {
  return (
    <div data-next-steps="" className="flex flex-col">
      <span className="mb-0.5 text-[11.5px] text-fg-tertiary">Next:</span>
      {items.map((item) => (
        <button
          key={item}
          type="button"
          title={item}
          data-next-step={item}
          onClick={() => onPick(item)}
          className={cn(
            'group -mx-2 flex min-w-0 items-center justify-between gap-3 rounded-[7px] px-2 py-[5px] text-left text-[13px] text-fg-secondary outline-none transition-colors',
            'hover:bg-foreground/6 hover:text-foreground focus-visible:bg-foreground/6 focus-visible:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50',
          )}
        >
          <span className="min-w-0 truncate">{item}</span>
          {hint && (
            <kbd className="shrink-0 font-mono text-[11px] text-fg-tertiary opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100">
              ↵ send
            </kbd>
          )}
        </button>
      ))}
    </div>
  )
}

/** The chat route's "Next:" block for `session`, in the message column above the background area. */
export function NextStepsSlot({ session, watching = false }: { session: string; watching?: boolean }) {
  const suggestions = useSessionField(session, 'suggestions')
  const running = useSessionField(session, 'running') ?? false
  const kind = useSessionField(session, 'kind')
  const parent = useSessionField(session, 'parent')
  const items = nextSteps({ suggestions, running, kind, parent, watching })
  if (!items) return null
  return (
    <div className="mx-auto w-full max-w-[768px] shrink-0 px-4 pb-1 pt-2">
      <NextSteps items={items} onPick={(text) => useNextPick.getState().pick(session, text)} />
    </div>
  )
}
