// A finished item's full final response. The engine stores only the first line (`summary`, ≤ 160 chars), so the
// pane reads the session's last page and takes the assistant row that line came from. History rows are text only:
// tool calls and reasoning are rows of their own, never part of an assistant row's text.
import type { HistoryEntry } from '@/rpc/bindings/method.agent.output'

/** The first non-empty line, with a heading's `#`s stripped, as the engine's `parse_finished` takes it. */
export function firstLine(text: string): string {
  for (const line of text.split('\n')) {
    const t = line.trim().replace(/^#+/, '').trim()
    if (t) return t
  }
  return ''
}

/** Everything after the first non-empty line, trimmed; the trailing `## Next` section stays (it is part of the text). */
export function afterFirstLine(text: string): string {
  const lines = text.split('\n')
  const i = lines.findIndex((l) => l.trim() !== '')
  return i < 0 ? '' : lines.slice(i + 1).join('\n').trim()
}

/** The summary may be clipped with `…`: compare on the part before it. */
const stem = (s: string) => s.replace(/\u2026$/, '').trim()

/**
 * The final response a `finished` item stands for: the newest assistant row whose first line starts with the item's
 * summary, else the newest assistant row. `null` when the page holds no assistant text.
 */
export function finalResponse(history: readonly HistoryEntry[], summary: string | null): string | null {
  const rows = history.filter((h): h is Extract<HistoryEntry, { role: 'assistant' }> => h.role === 'assistant' && h.text.trim() !== '')
  if (rows.length === 0) return null
  const want = summary ? stem(summary) : ''
  if (want) {
    for (let i = rows.length - 1; i >= 0; i--) {
      if (firstLine(rows[i].text).startsWith(want)) return rows[i].text
    }
  }
  return rows[rows.length - 1].text
}
