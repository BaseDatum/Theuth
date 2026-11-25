// 219 (Copy): what a code block's copy controls put on the clipboard — the text to paste into a terminal, not the fence.
import { cellOf } from '@/components/cells/grammar'
import { notify } from '@/lib/notify'

const PROMPT = '$ '

/** Every non-blank line starts with the `$ ` prompt (a transcript-style command block). */
const prompted = (lines: readonly string[]) => {
  const text = lines.filter((l) => l.trim() !== '')
  return text.length > 0 && text.every((l) => l.startsWith(PROMPT))
}

/**
 * Text to copy for a code block: drops the `#|`/`//|` header lines (cellOf header span, so shebang and blank-line
 * rules match; a shebang stays), and strips a leading `$ ` from every line only when every non-blank line has it.
 * Secret placeholders (double-brace secret:NAME) pass through literally.
 */
export function copyText(lang: string, code: string): string {
  let lines = code.replaceAll('\r\n', '\n').replaceAll('\r', '\n').split('\n')
  const spec = cellOf(lang, code)
  if (spec) lines = [...lines.slice(0, spec.header_lines).filter((l) => l.startsWith('#!')), ...lines.slice(spec.header_lines)]
  if (lines.length > 1 && lines.at(-1) === '') lines.pop()
  return (prompted(lines) ? lines.map((l) => (l.startsWith(PROMPT) ? l.slice(PROMPT.length) : l)) : lines).join('\n')
}

/** One line, same `$ ` rule as copyText applied to that line alone. */
export function copyLine(line: string): string {
  return line.startsWith(PROMPT) ? line.slice(PROMPT.length) : line
}

/** Clipboard + status-bar notice via @/lib/notify ('Copied' / 'Copy failed'). No sonner, no toasts. */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    notify.message('Copied')
    return true
  } catch {
    notify.error('Copy failed')
    return false
  }
}
