/** One card a harness-wrapped input renders as. */
export type Injected =
  | { kind: 'skill'; name: string; words: string }
  | { kind: 'peer'; from: string; text: string }
  | { kind: 'subagent'; name: string; text: string }
  | { kind: 'job'; id: string; command: string; exit: string; secs: string; pty: boolean; text: string }
  | { kind: 'schedule'; name: string; at: string; quiet: boolean; text: string }
  | { kind: 'nudge'; text: string }

/** Wrappers that render a card, with the attribute naming the sender, and wrappers that render nothing. */
const CARD: Record<string, string> = { subagent_result: 'name="', message: 'from="', job_result: 'id="', schedule: 'name="' }
const HIDDEN: Record<string, true> = { context_summary: true, todo: true, context_pressure: true, runtime: true, knowledge: true }

/** Value of `key` (`name="`) in an open tag, up to the closing quote. */
const attr = (open: string, key: string) => open.split(key, 2)[1]?.split('"', 1)[0]

/**
 * Classify one user input (mirrors `agent::skills::injected` and
 * `tui::model::injected`). A skill injection (`<skill name="…">…</skill>`
 * plus the user's words) is one card; an input made entirely of harness
 * wrappers (`<subagent_result>`, `<message>`, `<job_result>`, `<schedule>` blocks joined
 * by newlines, harness notes such as `<context_summary>`) is one card per
 * block: todo nudges are visible, other notes yield none. `null` for human text (or anything not
 * entirely made of wrappers).
 */
export function classify(text: string): Injected[] | null {
  let rest = text.trimStart()
  if (rest.startsWith('<skill name="')) {
    const [name, after] = rest.slice('<skill name="'.length).split('">', 2)
    const words = after?.split('</skill>', 2)[1]
    return words === undefined ? null : [{ kind: 'skill', name: name as string, words: words.trim() }]
  }
  const out: Injected[] = []
  while (rest) {
    const gt = rest.indexOf('>')
    if (!rest.startsWith('<') || gt < 0) return null
    const open = rest.slice(1, gt)
    const name = open.split(' ', 1)[0] as string
    const key = CARD[name]
    if (key === undefined && !HIDDEN[name] && open !== 'todo_continue') return null
    const close = `</${name}>`
    const end = rest.indexOf(close, gt)
    if (end < 0) return null
    if (open === 'todo_continue') {
      const body = rest.slice(gt + 1, end).trim()
        .replaceAll('&lt;', '<')
        .replaceAll('&gt;', '>')
        .replaceAll('&amp;', '&')
      out.push({ kind: 'nudge', text: body })
    } else if (key !== undefined) {
      const from = attr(open, key)
      if (from === undefined) return null
      let body = rest.slice(gt + 1, end)
      body = (body.startsWith('\n') ? body.slice(1) : body).trimEnd()
      if (name === 'job_result') {
        const command = (attr(open, 'command="') ?? '')
          .replaceAll('&quot;', '"')
          .replaceAll('&lt;', '<')
          .replaceAll('&gt;', '>')
          .replaceAll('&amp;', '&')
        out.push({
          kind: 'job',
          id: from,
          command,
          exit: attr(open, 'exit="') ?? '',
          secs: attr(open, 'secs="') ?? '',
          pty: attr(open, 'pty="') === 'true',
          text: body,
        })
      } else if (name === 'schedule') {
        out.push({ kind: 'schedule', name: from, at: attr(open, 'at="') ?? '', quiet: attr(open, 'quiet="') === 'true', text: body })
      } else if (name === 'message') {
        out.push({ kind: 'peer', from, text: body })
      } else {
        out.push({ kind: 'subagent', name: from, text: body })
      }
    }
    rest = rest.slice(end + close.length).trimStart()
  }
  return out
}
