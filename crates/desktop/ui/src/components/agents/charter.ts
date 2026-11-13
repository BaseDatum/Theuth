import type { AttachRef } from '@/lib/attach'
import type { Binding } from "@/rpc/bindings/method.agent.output"

/** The charter of agent `name` as a document binding — `DocId::for_binding` on the engine makes the id deterministic. */
export const charterBinding = (name: string): Binding => ({ agent_charter: name })

/** The chat panel's key and the session title: `Charter · <name>`. */
export const charterTitle = (name: string): string => `Charter · ${name}`

/**
 * 66 §3.3: the charter as the chat's pinned reference. There is no per-session system prompt, so the standing
 * context — whose charter, what the agent is, how to treat the text — rides in the marker's `meta`, which every
 * message carries. Kept to one line without `]` (the marker grammar's terminator).
 */
export function charterRef(
  id: string,
  a: { name: string; title: string; reports_to: string; capabilities: string | null },
): AttachRef {
  const caps = a.capabilities?.trim() ? `; hand off to it when: ${a.capabilities.trim()}` : ''
  const meta = `the charter of agent ${a.name} (${a.title.trim() || 'untitled'}, reports to ${a.reports_to}${caps}). You are helping write it: edit it with the doc tools and keep the user's words`
  return { kind: 'note', id, title: charterTitle(a.name), meta: meta.replace(/\s+/g, ' ').replace(/\[/g, '(').replace(/]/g, ')').slice(0, 400) }
}
