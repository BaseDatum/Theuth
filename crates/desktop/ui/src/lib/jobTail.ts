/** Rows of live output a `shell` card shows for its background job. */
export const JOB_LINES = 10

const isControl = (c: string) => {
  const n = c.codePointAt(0) ?? 0
  return n < 0x20 || (n >= 0x7f && n <= 0x9f)
}

/**
 * Append raw job output to a tail. The last element of `lines` is the line
 * in progress (possibly empty); the ones before it are complete, oldest
 * first, at most `JOB_LINES`. `\r` rewinds the current line (progress
 * bars), tabs become spaces, ANSI CSI/OSC sequences and other control
 * characters are dropped. Returns a new array; `lines` is not mutated.
 */
export function pushChunk(lines: readonly string[], chunk: string): string[] {
  const out = lines.slice(0, -1)
  let cur = lines[lines.length - 1] ?? ''
  const chars = Array.from(chunk)
  let i = 0
  while (i < chars.length) {
    const c = chars[i++] ?? ''
    switch (c) {
      case '\n':
        out.push(cur)
        if (out.length > JOB_LINES) out.shift()
        cur = ''
        break
      case '\r':
        cur = ''
        break
      case '\t':
        cur += '    '
        break
      case '\x1b': {
        const next = chars[i++]
        if (next === '[') {
          // CSI: parameters, then one final byte in `@`..`~`.
          let d = chars[i++]
          while (d !== undefined && !(d >= '@' && d <= '~')) d = chars[i++]
        } else if (next === ']') {
          // OSC: to BEL or ST.
          let prev = ''
          let d = chars[i++]
          while (d !== undefined && d !== '\x07' && !(prev === '\x1b' && d === '\\')) {
            prev = d
            d = chars[i++]
          }
        }
        break
      }
      default:
        if (!isControl(c)) cur += c
    }
  }
  out.push(cur)
  return out
}

/** The rows to draw, oldest first: complete lines and a non-empty partial one, at most `JOB_LINES`. */
export function rows(lines: readonly string[]): string[] {
  const shown = lines[lines.length - 1] === '' ? lines.slice(0, -1) : lines
  return shown.slice(-JOB_LINES)
}
