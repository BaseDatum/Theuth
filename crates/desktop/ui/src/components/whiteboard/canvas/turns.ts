// A session card's body (design §3.5 "last N turns, read-only"): the newest user/assistant messages, oldest first.
// Pure, over either the live session store's entries or a `load_session` history page.

export type Turn = { role: 'user' | 'assistant'; text: string }

/** Characters kept per message (a card shows a few lines of each). */
export const TURN_CHARS = 280

/** Rows shaped `{ kind | role, text }`: user and assistant rows with text, the newest `n`, each cut to TURN_CHARS. */
export function lastTurns(rows: readonly unknown[], n = 6): Turn[] {
  const out: Turn[] = []
  for (let i = rows.length - 1; i >= 0 && out.length < n; i--) {
    const r = rows[i]
    if (!r || typeof r !== 'object') continue
    const role = 'kind' in r ? r.kind : 'role' in r ? r.role : undefined
    const text = 'text' in r && typeof r.text === 'string' ? r.text.trim() : ''
    if ((role !== 'user' && role !== 'assistant') || !text) continue
    out.push({ role, text: text.length > TURN_CHARS ? `${text.slice(0, TURN_CHARS - 1)}…` : text })
  }
  return out.reverse()
}
