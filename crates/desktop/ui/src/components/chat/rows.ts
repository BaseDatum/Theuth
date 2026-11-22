import { withGaps } from '@/lib/time'
import type { AgentInfo } from '@/store/ui-events'
import type { Prompt } from '@/store/approvals'
import type { Entry } from '@/store/sessions'
import { foldRuns, type Run } from './ToolRunCard'

export type Row =
  | { key: string; entry: Entry; prompts: Prompt[] | null }
  | { key: string; run: Run; prompts: Prompt[] | null }
  /** A prompt with no row of its own, appended after the last entry: `from` names the child session it belongs to. */
  | { key: string; prompt: Prompt; from?: AgentInfo }
  /** `1 h 12 min later · 15:15` between two rows at least an hour apart (74 §5). */
  | { key: string; gap: string }

/** `at` of a row: the entry's, or the first entry's of a folded run; a loose prompt has none. */
export const rowAt = (row: Row): number | undefined => ('entry' in row ? row.entry.at : 'run' in row ? row.run[0]?.at : undefined)

/** 103 F4: a decision raised by `escalate` is an `ask_user` whose one question carries the `decision:<id>` key
 * (`tasks/src/lib.rs ask_user`; the ask's own id is a plain uuid). Same test as `AskCard.tsx decisionKey`. */
export const isDecision = (prompt: Prompt): boolean =>
  prompt.type === 'AskRequested' && prompt.questions.length === 1 && prompt.questions[0]!.id.startsWith('decision:')

/**
 * Entries (consecutive tool calls folded into one run row when `collapse`, 70, a `python` call and its runtime calls always into one group; `pinned` thinking blocks never fold,
 * `foldRuns`), each with the prompts waiting on its (unfinished, same-named) tool call. 114 (Sky's D1 + 16:55 correction):
 * - a decision ask is never a chat card: the desk's `escalate` tool row is the awareness and `/attention` is where it is
 *   answered — it is dropped here;
 * - every other prompt with no such row (a subagent's prompt, 19 §3.4: `children` is the session's subtree, labelled with
 *   the child; a loose ask) is appended after the last entry, in `pending` order, so an open approval or question is
 *   never lost in the transcript after a reload.
 */
export function buildRows(
  entries: Entry[],
  pending: Prompt[],
  session: string,
  children: Record<string, AgentInfo> | undefined,
  collapse: boolean,
  pinned?: ReadonlySet<string>,
): Row[] {
  const items = foldRuns(entries, pinned, collapse)
  const rows: Row[] = items.map((item) =>
    Array.isArray(item) ? { key: (item[0] as Entry).id, run: item, prompts: null } : { key: item.id, entry: item, prompts: null },
  )
  const loose: Row[] = []
  for (const prompt of pending) {
    if (isDecision(prompt)) continue
    if (prompt.session !== session) {
      const from = children?.[prompt.session]
      if (from) loose.push({ key: prompt.id, prompt, from })
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
    else loose.push({ key: prompt.id, prompt })
  }
  return withGaps(rows.concat(loose), rowAt)
}
