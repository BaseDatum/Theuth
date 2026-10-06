import { withGaps } from '@/lib/time'
import type { AgentInfo } from '@/rpc/extra'
import type { Prompt } from '@/store/approvals'
import type { Entry } from '@/store/sessions'
import { foldRuns, type Run } from './ToolRunCard'

export type Row =
  | { key: string; entry: Entry; prompts: Prompt[] | null }
  | { key: string; run: Run; prompts: Prompt[] | null }
  /** A prompt with no row of its own: `from` names the child session it belongs to; `at` is its arrival (114). */
  | { key: string; prompt: Prompt; from?: AgentInfo; at?: number }
  /** `1 h 12 min later · 15:15` between two rows at least an hour apart (74 §5). */
  | { key: string; gap: string }

/** `at` of a row: the entry's, the first entry's of a folded run, or a loose prompt's arrival. */
export const rowAt = (row: Row): number | undefined =>
  'entry' in row ? row.entry.at : 'run' in row ? row.run[0]?.at : 'prompt' in row ? row.at : undefined

/** 103 F4: a decision raised by `escalate` is an `ask_user` whose one question carries the `decision:<id>` key
 * (`tasks/src/lib.rs ask_user`; the ask's own id is a plain uuid). Same test as `AskCard.tsx decisionKey`. */
export const isDecision = (prompt: Prompt): boolean =>
  prompt.type === 'AskRequested' && prompt.questions.length === 1 && prompt.questions[0]!.id.startsWith('decision:')

/**
 * Entries (consecutive tool calls folded into one run row when `collapse`, 70), each with the prompts waiting on its
 * (unfinished, same-named) tool call. 114 (Sky's D1):
 * - a decision ask is never a chat card: the desk's `escalate` tool row is the awareness and `/attention` is where it is
 *   answered — it is dropped here;
 * - every other prompt with no such row (a subagent's prompt, 19 §3.4: `children` is the session's subtree, labelled with
 *   the child; a loose ask) is a card at its arrival time `at` — after the last row at or before it — never pinned to
 *   the bottom; one without an arrival (first seen through the engine's snapshot) comes last.
 */
export function buildRows(
  entries: Entry[],
  pending: Prompt[],
  at: Map<string, number>,
  session: string,
  children: Record<string, AgentInfo> | undefined,
  collapse: boolean,
): Row[] {
  const items = collapse ? foldRuns(entries) : entries
  const rows: Row[] = items.map((item) =>
    Array.isArray(item) ? { key: (item[0] as Entry).id, run: item, prompts: null } : { key: item.id, entry: item, prompts: null },
  )
  const loose: Extract<Row, { prompt: Prompt }>[] = []
  for (const prompt of pending) {
    if (isDecision(prompt)) continue
    if (prompt.session !== session) {
      const from = children?.[prompt.session]
      if (from) loose.push({ key: prompt.id, prompt, from, at: at.get(prompt.id) })
      continue
    }
    const tool =
      prompt.type === 'ApprovalRequested' ? prompt.tool : prompt.type === 'AskRequested' ? 'ask_user' : 'secret_request'
    const waits = (e: Entry) => e.kind === 'tool' && e.name === tool && !e.result
    let i = items.length - 1
    while (i >= 0) {
      const item = items[i] as Entry | Run
      if (Array.isArray(item) ? item.some(waits) : waits(item)) break
      i--
    }
    const row = rows[i]
    if (row && 'prompts' in row) row.prompts = [...(row.prompts ?? []), prompt]
    else loose.push({ key: prompt.id, prompt, at: at.get(prompt.id) })
  }
  // Stable: two prompts at the same instant (or both without one) keep their store order.
  loose.sort((a, b) => (a.at ?? Number.POSITIVE_INFINITY) - (b.at ?? Number.POSITIVE_INFINITY))
  const out: Row[] = []
  let next = 0
  for (const row of rows) {
    const t = rowAt(row)
    if (t !== undefined) while (next < loose.length && (loose[next]!.at ?? Number.POSITIVE_INFINITY) < t) out.push(loose[next++]!)
    out.push(row)
  }
  for (; next < loose.length; next++) out.push(loose[next]!)
  return withGaps(out, rowAt)
}
