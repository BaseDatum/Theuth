// 171F B13: every transcript disclosure keeps its open state in `useToolRuns.disclosed` (`useDisclosure`), never in
// the component, so no remount can close what the reader expanded: a lone tool folding into a run, a row crossing the
// transcript's two arrays or leaving the virtual window, a reply's stream closing, the settled reload. The ratchet in
// `components/disclosure.test.ts` keeps component-local open state out of the transcript.

import { createContext, type ReactNode, useContext, useId, useSyncExternalStore } from 'react'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { disclosureKey, useDisclosure, useToolRuns } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'

/** Where a disclosure sits: `Message` provides its entry id, a fence inside a reply adds its offset (`FenceScope`). */
export const DisclosureScope = createContext<string | null>(null)

/** The reply's markdown, for `FenceScope`; `MessageAssistant` provides it. Its own context, so a text delta re-renders
 * the scopes only (their children are unchanged elements) and never the fences' other `FenceContext` readers. */
export const FenceText = createContext<string | null>(null)

/** The disclosure of tool call `callId`: a lone tool card's details, and the run that later folds that card in opens with it. */
export const toolKey = (callId: string) => disclosureKey('tool', callId)

/** The store key of `part` under the enclosing `DisclosureScope`; outside any scope (an artifact viewer, a settings
 * page) under this mount, as local as the `useState` it replaces. */
export function useScopeKey(part: string): string {
  const scope = useContext(DisclosureScope)
  const local = useId()
  return disclosureKey(scope ?? local, part)
}

/** `useDisclosure` for `part` under the enclosing scope. */
export function useScopedDisclosure(part: string, initial = false) {
  return useDisclosure(useScopeKey(part), initial)
}

/** A primitive derived from the open map, read like `useDisclosure` (the live state is the server snapshot too). */
export function useDisclosed<T extends string | number | boolean>(pick: (open: Record<string, boolean>) => T): T {
  const read = () => pick(useToolRuns.getState().disclosed)
  return useSyncExternalStore(useToolRuns.subscribe, read, read)
}

/**
 * Scopes the disclosures of the fence whose body is `code` by its offset in the reply (`FenceText`): stable while the
 * reply streams (text only grows) and across remounts. Trimmed first: streamdown closes an open fence with a newline
 * the reply does not have yet. A body still not found verbatim (a fence indented inside a list item) falls back to its
 * first 80 characters.
 */
export function FenceScope({ code, children }: { code: string; children: ReactNode }) {
  const text = useContext(FenceText)
  const body = code.trimEnd()
  const at = text?.indexOf(body) ?? -1
  return <DisclosureScope value={useScopeKey(disclosureKey('fence', at >= 0 ? at : body.slice(0, 80)))}>{children}</DisclosureScope>
}

/** A transcript card's fold (what an HTML details element used to be): `trigger` toggles `children`; open state under `part` in the enclosing scope.
 * The root carries `data-state`, so a `group/x` class on it styles the chevron with `group-data-[state=open]/x:`. */
export function Details({ part, className, trigger, triggerClassName, children }: { part: string; className?: string; trigger: ReactNode; triggerClassName?: string; children: ReactNode }) {
  const [open, setOpen] = useScopedDisclosure(part)
  return (
    <Collapsible open={open} onOpenChange={setOpen} className={className}>
      <CollapsibleTrigger className={cn('w-full cursor-pointer text-left', triggerClassName)}>{trigger}</CollapsibleTrigger>
      <CollapsibleContent>{children}</CollapsibleContent>
    </Collapsible>
  )
}
