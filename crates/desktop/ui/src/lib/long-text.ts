/** 171F B6: a user message's size, when it is long enough to fold: more than 12 lines or more than 1200 characters
 * (code points, a CRLF counting as one line break); `null` otherwise. */
export function longText(text: string): { lines: number; chars: number } | null {
  const t = text.replace(/\r\n?/g, '\n')
  const lines = t ? t.split('\n').length : 0
  const chars = [...t].length
  return lines > 12 || chars > 1200 ? { lines, chars } : null
}

/** The folded message's toggle: `Show all N lines`, or `Show all · N characters` for one long line. */
export const showAll = ({ lines, chars }: { lines: number; chars: number }): string =>
  lines > 1 ? `Show all ${lines} lines` : `Show all · ${chars} characters`
