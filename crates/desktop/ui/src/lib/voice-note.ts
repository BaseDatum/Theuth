// 45 §4.4: the `[voice note, N s]` marker is the whole "this was spoken" convention;
// mirrors `voice::note_text` / `voice::is_note` on the engine.

/** The user line for a transcript: `[voice note, 12 s] …`; at least one second. */
export const noteText = (text: string, ms: number): string =>
  `[voice note, ${Math.max(1, Math.ceil(ms / 1000))} s] ${text}`

/** Split a user line into its marker and transcript; `null` when it was typed. */
export function parseNote(text: string): { secs: number; body: string } | null {
  const m = /^\s*\[voice note, (\d+) s\]\s*/.exec(text)
  return m ? { secs: Number(m[1]), body: text.slice(m[0].length) } : null
}
