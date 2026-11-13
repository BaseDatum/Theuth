import { THEUTH } from '@/components/tasks/helpers'
import type { Api } from '@/rpc/bindings/client'
import type { AgentSummary } from "@/rpc/bindings/method.roster.output"

/** Proposed agents (67 §6.5) — the Hiring inbox's rows, oldest name first. */
export function proposed(agents: readonly AgentSummary[] | undefined): AgentSummary[] {
  return (agents ?? []).filter((a) => a.status === 'proposed' || a.file.proposed_by !== null).sort((x, y) => x.file.name.localeCompare(y.file.name))
}

/**
 * The Hiring inbox's two verbs over the RPCs that exist: approve = `agent_approve`; reject = `agent_remove` with the
 * note (trimmed; empty = none) for the hire task. Pure over `api` so the wiring is testable without React.
 */
export function hiring(api: Pick<Api, 'agent_approve' | 'agent_remove'>) {
  return {
    approve: (name: string) => api.agent_approve(name),
    reject: (name: string, note: string) => api.agent_remove(name, note.trim() || null),
  }
}

/** Names an agent may report to: the root plus every other roster entry. */
export function managerNames(agents: readonly AgentSummary[] | undefined, self: string): string[] {
  return [THEUTH, ...(agents ?? []).map((a) => a.file.name).filter((n) => n !== THEUTH && n !== self).sort()]
}
