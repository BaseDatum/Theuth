/** Mid-turn harness notes share one transcript presentation; context summaries already have their own card. */
export const HARNESS_NOTICES = {
  todo_continue: { title: 'Nudge', description: 'The harness asked the model to continue unfinished todos.' },
  rounds: { title: 'Tool rounds', description: 'The harness asked the model to wrap up within the tool-round limit.' },
  context_pressure: { title: 'Context pressure', description: 'The harness warned the model that context compaction is near.' },
  recovery: { title: 'Recovery', description: 'The harness asked the model to resume an interrupted reply.' },
  continue: { title: 'Continuation', description: 'The harness asked the model to continue a reply cut off by the output limit.' },
  steer: { title: 'Tool correction', description: 'The harness asked the model to change a repeatedly failing tool call.' },
  surface_error: { title: 'Display correction', description: 'The harness asked the model to fix content that could not be rendered.' },
  todo: { title: 'Todo reminder', description: 'The harness reminded the model of its current plan.' },
  knowledge: { title: 'Knowledge reminder', description: 'The harness suggested relevant knowledge for the model to consult.' },
  vision: { title: 'Images unseen', description: 'The harness told the model no vision model is configured, so the pending images were not looked at.' },
}

/** One card a harness-wrapped input renders as. */
export type Injected =
  | { kind: 'skill'; name: string; words: string }
  | { kind: 'peer'; from: string; text: string }
  | { kind: 'subagent'; name: string; text: string }
  | { kind: 'job'; id: string; command: string; exit: string; secs: string; pty: boolean; text: string }
  /** A quiet PTY job (61 §2.3): `text` is the screen. `name` is the job's label, if any. */
  | { kind: 'job_idle'; id: string; command: string; name?: string; quiet: string; secs: string; nudge: string; userTyped?: string; text: string }
  | { kind: 'schedule'; name: string; at: string; quiet: boolean; text: string }
  | { kind: 'harness'; notice: keyof typeof HARNESS_NOTICES; text: string }

/** Wrappers that render a card, with the attribute naming the sender, and wrappers that render nothing. */
const CARD: Record<string, string> = { subagent_result: 'name="', message: 'from="', job_result: 'id="', job_idle: 'id="', schedule: 'name="' }
// `surface_error` (30 §8): the correction round is quiet on the desktop — the fence shows its own failure state — so the
// `<surface_error>` / `<surface_error final="true">` notes render nothing; the notice entry above stays for other clients.
const HIDDEN: Record<string, true> = { context_summary: true, runtime: true, surface_error: true }

/** Value of `key` (`name="`) in an open tag, up to the closing quote. */
const attr = (open: string, key: string) => open.split(key, 2)[1]?.split('"', 1)[0]

/** Undo the wrappers' XML attribute escape. */
const unescape = (s: string) => s.replaceAll('&quot;', '"').replaceAll('&lt;', '<').replaceAll('&gt;', '>').replaceAll('&amp;', '&')

/**
 * Classify one user input (mirrors `agent::skills::injected` and
 * `tui::model::injected`). A skill injection (`<skill name="…">…</skill>`
 * plus the user's words) is one card; an input made entirely of harness
 * wrappers (`<subagent_result>`, `<message>`, `<job_result>`, `<job_idle>`, `<schedule>` blocks joined
 * by newlines, harness notes such as `<context_summary>`) is one card per
 * block: mid-turn notices are visible; context summaries and runtime wrappers yield none.
 * `null` for human text (or anything not entirely made of wrappers).
 */
export function classify(text: string): Injected[] | null {
  let rest = text.trimStart()
  if (rest.startsWith('<skill name="')) {
    // `indexOf`, not `split(sep, 2)`: JS's limit drops the remainder, and a body with `">` in it (`<input type="date">`) is common.
    const at = rest.indexOf('">')
    const end = rest.indexOf('</skill>', at)
    if (at < 0 || end < 0) return null
    return [{ kind: 'skill', name: rest.slice('<skill name="'.length, at), words: rest.slice(end + '</skill>'.length).trim() }]
  }
  const out: Injected[] = []
  while (rest) {
    const gt = rest.indexOf('>')
    if (!rest.startsWith('<') || gt < 0) return null
    const open = rest.slice(1, gt)
    const name = open.split(' ', 1)[0] as string
    const key = CARD[name]
    const notice = Object.hasOwn(HARNESS_NOTICES, open) ? open as keyof typeof HARNESS_NOTICES : undefined
    if (key === undefined && !HIDDEN[name] && notice === undefined) return null
    const close = `</${name}>`
    const end = rest.indexOf(close, gt)
    if (end < 0) return null
    // Hidden wrappers win over a same-named notice, whatever attributes the open tag carries.
    if (HIDDEN[name]) {
      rest = rest.slice(end + close.length).trimStart()
      continue
    }
    if (notice !== undefined) {
      let body = rest.slice(gt + 1, end).trim()
      // Only todo continuations escape their body at emission; other notes are already literal text.
      if (notice === 'todo_continue') body = body.replaceAll('&lt;', '<').replaceAll('&gt;', '>').replaceAll('&amp;', '&')
      out.push({ kind: 'harness', notice, text: body })
    } else if (key !== undefined) {
      const from = attr(open, key)
      if (from === undefined) return null
      let body = rest.slice(gt + 1, end)
      body = (body.startsWith('\n') ? body.slice(1) : body).trimEnd()
      if (name === 'job_result') {
        out.push({
          kind: 'job',
          id: from,
          command: unescape(attr(open, 'command="') ?? ''),
          exit: attr(open, 'exit="') ?? '',
          secs: attr(open, 'secs="') ?? '',
          pty: attr(open, 'pty="') === 'true',
          text: body,
        })
      } else if (name === 'job_idle') {
        const jobName = attr(open, 'name="')
        const userTyped = attr(open, 'user_typed="')
        out.push({
          kind: 'job_idle',
          id: from,
          command: unescape(attr(open, 'command="') ?? ''),
          ...(jobName === undefined ? {} : { name: unescape(jobName) }),
          quiet: attr(open, 'quiet="') ?? '',
          secs: attr(open, 'secs="') ?? '',
          nudge: attr(open, 'nudge="') ?? '',
          ...(userTyped === undefined ? {} : { userTyped }),
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
