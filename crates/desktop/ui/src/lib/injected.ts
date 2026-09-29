/** Mid-turn engine notes share one transcript presentation; context summaries already have their own card. */
export const ENGINE_NOTICES = {
  todo_continue: { title: 'Nudge', description: 'theuth asked the model to continue unfinished todos.' },
  rounds: { title: 'Tool rounds', description: 'theuth asked the model to wrap up within the tool-round limit.' },
  context_pressure: { title: 'Context pressure', description: 'theuth warned the model that context compaction is near.' },
  recovery: { title: 'Recovery', description: 'theuth asked the model to resume an interrupted reply.' },
  continue: { title: 'Continuation', description: 'theuth asked the model to continue a reply cut off by the output limit.' },
  steer: { title: 'Tool correction', description: 'theuth asked the model to change a repeatedly failing tool call.' },
  surface_error: { title: 'Display correction', description: 'theuth asked the model to fix content that could not be rendered.' },
  todo: { title: 'Todo reminder', description: 'theuth reminded the model of its current plan.' },
  knowledge: { title: 'Knowledge reminder', description: 'theuth suggested relevant knowledge for the model to consult.' },
  vision: { title: 'Images unseen', description: 'theuth told the model no vision model is configured, so the pending images were not looked at.' },
  nudge: { title: 'Nudge', description: 'theuth asked the desk to dispose of its task before the turn ends (70 DESK-PROTOCOL 3).' },
  journal: { title: 'Journal', description: 'theuth asked the desk to write its episode journal before the desk closes.' },
}

/** One card an engine-wrapped input renders as. */
export type Injected =
  | { kind: 'skill'; name: string; words: string }
  | { kind: 'peer'; from: string; text: string }
  | { kind: 'subagent'; name: string; text: string }
  | { kind: 'job'; id: string; command: string; exit: string; secs: string; pty: boolean; text: string }
  /** A quiet PTY job (61 §2.3): `text` is the screen. `name` is the job's label, if any. */
  | { kind: 'job_idle'; id: string; command: string; name?: string; quiet: string; secs: string; nudge: string; userTyped?: string; text: string }
  | { kind: 'schedule'; name: string; at: string; quiet: boolean; text: string }
  /** A fired `remind_me_in` timer (14 as-built): `after` is the requested seconds, `late` when re-armed past due after a restart. */
  | { kind: 'reminder'; id: string; after: string; late: boolean; text: string }
  /** A desk episode's opening block (roster 67 §6.2): the agent's charter, its journal and the operating manual. */
  | { kind: 'desk'; agent: string; charter: string; journal: string; manual: string }
  /** A scoped wake (roster 67 §7.3). `id`/`title` name the task when there is one; `state`…`body` come from the
   * `<task>` record that follows when the task was loaded; `count` when the wake was raised more than once;
   * `roster` is the fit-check block an assigned/reassigned wake carries (70 DESK-PROTOCOL). */
  | { kind: 'wake'; id?: string; title?: string; reason: string; count?: number; state?: string; owner?: string; reviewer?: string; thread?: string; body?: string; roster?: string }
  | { kind: 'engine'; notice: keyof typeof ENGINE_NOTICES; text: string }

/** Wrappers that render a card, with the attribute naming the sender, and wrappers that render nothing. */
const CARD: Record<string, string> = { subagent_result: 'name="', message: 'from="', job_result: 'id="', job_idle: 'id="', schedule: 'name="', desk: 'agent="' }
// `surface_error` (30 §8): the correction round is quiet on the desktop — the fence shows its own failure state — so the
// `<surface_error>` / `<surface_error final="true">` notes render nothing; the notice entry above stays for other clients.
const HIDDEN: Record<string, true> = { context_summary: true, runtime: true, surface_error: true }

/** Value of `key` (`name="`) in an open tag, up to the closing quote. */
const attr = (open: string, key: string) => open.split(key, 2)[1]?.split('"', 1)[0]

/** Undo the wrappers' XML attribute escape. */
const unescape = (s: string) => s.replaceAll('&quot;', '"').replaceAll('&lt;', '<').replaceAll('&gt;', '>').replaceAll('&amp;', '&')

/** Trimmed text between `<tag>` and `</tag>`; `''` when absent. */
function inner(s: string, tag: string): string {
  const a = s.indexOf(`<${tag}>`)
  const b = s.indexOf(`</${tag}>`, a)
  return a < 0 || b < 0 ? '' : s.slice(a + tag.length + 2, b).trim()
}

/** `roster::desk::wake_text`: the header line, then the task record when the task was loaded, then the `<roster>` fit check on assigned/reassigned. */
const WAKE =
  /^Wake: (?:task (\S+)(?: — (.*?))?|no task)\. Reason: ([a-z_]+)\.(?: \(Raised (\d+) times\.\))? Do not switch to another task until this wake is handled\.(?:\n\n<task ([^\n]*?)>\n([\s\S]*)\n<\/task>)?(?:\n\n<roster>\n([\s\S]*?)\n<\/roster>)?$/

function wake(text: string): Extract<Injected, { kind: 'wake' }> | null {
  const m = WAKE.exec(text)
  if (!m) return null
  const [, id, title, reason = '', count, open = '', body, roster] = m
  const w: Extract<Injected, { kind: 'wake' }> = { kind: 'wake', reason }
  if (id !== undefined) w.id = id
  if (title !== undefined) w.title = title
  if (count !== undefined) w.count = Number(count)
  if (body !== undefined) {
    w.state = attr(open, 'state="') ?? ''
    w.owner = attr(open, 'owner="') ?? ''
    const reviewer = attr(open, 'reviewer="')
    const thread = attr(open, 'thread="')
    if (reviewer !== undefined) w.reviewer = reviewer
    if (thread !== undefined) w.thread = thread
    w.body = body
  }
  if (roster !== undefined) w.roster = roster
  return w
}

/** `<reminder id="r3" after="600" set_at="…" late="false">` (14 as-built); the id names the sender. */
CARD.reminder = 'id="'

/** `45 s`, `10 min`, `1 h 30 min` for a reminder's requested delay. */
export function humanSecs(secs: string | number): string {
  const s = Number(secs)
  if (!Number.isFinite(s)) return String(secs)
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60)
  if (h > 0) return m > 0 ? `${h} h ${m} min` : `${h} h`
  return m > 0 ? `${m} min` : `${s} s`
}

/** `supervisor::broadcast`: `[broadcast <4 hex> · <n> sessions]\n<text>` — the human's one message to many sessions (62 §0.4). */
const BROADCAST = /^\[broadcast ([0-9a-f]{4}) · (\d+) sessions\](?:\n|$)/

/** The broadcast marker's id and recipient count with the human's words; `null` for any other input. */
export function parseBroadcast(text: string): { id: string; count: number; text: string } | null {
  const m = BROADCAST.exec(text.trimStart())
  return m ? { id: m[1] as string, count: Number(m[2]), text: text.trimStart().slice(m[0].length) } : null
}

/** 76 talk: `[spoken, N s] text` / `[spoken, N s, interrupted] text` — one utterance from the live talk; `interrupted` = the user cut the agent off. */
const SPOKEN = /^\[spoken, (\d+) s(, interrupted)?\]\s*/

/** The talk marker's seconds, interruption flag and transcript; `null` for any other input. */
export function parseSpoken(text: string): { secs: number; interrupted: boolean; text: string } | null {
  const m = SPOKEN.exec(text.trimStart())
  return m ? { secs: Number(m[1]), interrupted: m[2] !== undefined, text: text.trimStart().slice(m[0].length) } : null
}

/**
 * Classify one user input (mirrors `agent::skills::injected` and
 * `tui::model::injected`). A skill injection (`<skill name="…">…</skill>`
 * plus the user's words) is one card; an input made entirely of engine
 * wrappers (`<subagent_result>`, `<message>`, `<job_result>`, `<job_idle>`, `<schedule>` blocks joined
 * by newlines, engine notes such as `<context_summary>`) is one card per
 * block: mid-turn notices are visible; context summaries and runtime wrappers yield none. A desk's
 * input (`<desk>` opening, then/or a `Wake: …` line with its `<task>` record) is a card per block too.
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
    // A desk's wake (roster 67 §7.3) is the last block of its input and the only one not tag-wrapped.
    if (rest.startsWith('Wake: ')) {
      const w = wake(rest.trimEnd())
      if (w === null) return null
      out.push(w)
      break
    }
    const gt = rest.indexOf('>')
    if (!rest.startsWith('<') || gt < 0) return null
    const open = rest.slice(1, gt)
    const name = open.split(' ', 1)[0] as string
    const key = CARD[name]
    const notice = Object.hasOwn(ENGINE_NOTICES, open) ? open as keyof typeof ENGINE_NOTICES : undefined
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
      out.push({ kind: 'engine', notice, text: body })
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
      } else if (name === 'reminder') {
        out.push({ kind: 'reminder', id: from, after: attr(open, 'after="') ?? '', late: attr(open, 'late="') === 'true', text: body })
      } else if (name === 'message') {
        out.push({ kind: 'peer', from, text: body })
      } else if (name === 'desk') {
        const j = body.indexOf('</journal>')
        out.push({ kind: 'desk', agent: from, charter: inner(body, 'charter'), journal: inner(body, 'journal'), manual: j < 0 ? '' : body.slice(j + '</journal>'.length).trim() })
      } else {
        out.push({ kind: 'subagent', name: from, text: body })
      }
    }
    rest = rest.slice(end + close.length).trimStart()
  }
  return out
}
