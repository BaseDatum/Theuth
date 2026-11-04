import type { AdvisorSeverity } from '@/rpc/bindings/AdvisorSeverity'

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
  /** 152 D5: `loop_::budget_note` — `<budget spent cap currency>`, the only notice with attributes. */
  budget: { title: 'Budget spent', description: "theuth told the model the session's money budget is spent and the turn ended." },
}

/** One card an engine-wrapped input renders as. */
export type Injected =
  | { kind: 'skill'; name: string; words: string }
  | { kind: 'peer'; from: string; text: string }
  | { kind: 'subagent'; name: string; text: string }
  | { kind: 'job'; id: string; command: string; exit: string; secs: string; pty: boolean; text: string }
  /** A quiet PTY job (61 §2.3): `text` is the screen. `name` is the job's label, if any. 155 §6.5: `why` it fired. */
  | { kind: 'job_idle'; id: string; command: string; name?: string; quiet: string; secs: string; nudge: string; userTyped?: string; why?: string; text: string }
  | { kind: 'schedule'; name: string; at: string; quiet: boolean; text: string }
  /** A fired `remind_me_in` timer (14 as-built): `after` is the requested seconds, `late` when re-armed past due after a restart. */
  | { kind: 'reminder'; id: string; after: string; late: boolean; text: string }
  /** A desk episode's opening block (roster 67 §6.2): the agent's charter, its journal and the operating manual. */
  | { kind: 'desk'; agent: string; charter: string; journal: string; manual: string }
  /** A scoped wake (roster 67 §7.3). `task`/`title` name the task when there is one; `state`…`body` come from the
   * `<task>` record that follows when the task was loaded; `count` when the wake was raised more than once;
   * `roster` is the fit-check block an assigned/reassigned wake carries (70 DESK-PROTOCOL). */
  | { kind: 'wake'; task?: string; title?: string; reason: string; count?: number; state?: string; owner?: string; reviewer?: string; thread?: string; body?: string; roster?: string }
  /** 125 §5.4: one `<advisory severity="…">` element of an advisor row — a card, never a `you` bubble. */
  | { kind: 'advisory'; severity: AdvisorSeverity; text: string }
  | { kind: 'engine'; notice: keyof typeof ENGINE_NOTICES; text: string }
  /** 152 D1: `<resume_handoff>` (146 D10/D11). `restored`: the harness kept its context; else a fresh process got the
   * handoff's sections. `clock` is `HH:MM:SS` as written (`at` is the Entry's ms); `text` is the whole inner text, always kept. */
  | { kind: 'resume'; restored: boolean; clock: string; working?: string; previous?: string; sections: ResumeSection[]; omitted?: string[]; text: string }
  /** 152 D2: `<child_lost name session at why>` in a parent — a child not respawned after a restart (146 D4). */
  | { kind: 'lost'; name: string; session: string; clock: string; why: string; text: string }
  /** 151 R1 (D7): `<autoresearch slug event>` — a study's start / result / failure card delivered into its setup chat
   * when no messaging channel renders the session; `text` is the card markdown (a `json-render` fence or a line). */
  | { kind: 'autoresearch'; slug: string; event: string; text: string }
  /** 171 §9.1: `<cell_result run=… name=…|cell=… label=… message=… exit secs …>` — a user cell's run the agent read; one quiet line. */
  | { kind: 'cell_result'; run: string; name?: string; label?: string; cell?: string; message?: string; exit: string; secs: string; text: string }

export type CellResultInput = Extract<Injected, { kind: 'cell_result' }>

export type ResumeSection = { title: string; text: string; count?: number }

/** Wrappers that render a card, with the attribute naming the sender, and wrappers that render nothing. */
const CARD: Record<string, string> = { subagent_result: 'name="', message: 'from="', job_result: 'id="', job_idle: 'id="', schedule: 'name="', desk: 'agent="', advisory: 'severity="', child_lost: 'name="', autoresearch: 'slug="' }
// 171 §9.1 (L2): keyed on `run="`, the first attribute, which every cell result carries (unnamed cells have `cell="<ordinal>"`).
CARD.cell_result = 'run="'
// `surface_error` (30 §8): the correction round is quiet on the desktop — the fence shows its own failure state — so the
// `<surface_error>` / `<surface_error final="true">` notes render nothing; the notice entry above stays for other clients.
// 144 §5.3: `<notebook id title>` is the bound chat's volatile context block — a harness note the context line already
// summarises (`in context: N sources · M notes`), so it renders nothing, like `<runtime>`.
const HIDDEN: Record<string, true> = { context_summary: true, runtime: true, surface_error: true, notebook: true }
// 171 §9.2: the `<cell_error>` correction round is quiet on the desktop, like `surface_error`; the cell shows its invalid state.
HIDDEN.cell_error = true

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
  if (id !== undefined) w.task = id
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

/** `handoff::render_restored`'s header: the time and, when there was one, the interrupted prompt's excerpt. */
const RESTORED = /^The theuth engine restarted at (\d{2}:\d{2}:\d{2})(?: while you were working on: "([\s\S]*?)")?\. Your session with this harness has been restored/
/** `handoff::render`'s header: the time and the harness session that could not be loaded. */
const FRESH = /^The theuth engine restarted at (\d{2}:\d{2}:\d{2}) and your previous harness session(?: (\S+))? could not be loaded,[^\n]*\n/

/** `handoff::render`'s sections in order: the line-start head of each and how its body is quoted. */
const SECTIONS: { title: string; head: RegExp; quoted?: true; count?: true }[] = [
  { title: 'Original brief', head: /^Your original brief: "/m, quoted: true },
  { title: 'Interrupted prompt', head: /^The prompt you were interrupted in: "/m, quoted: true },
  { title: 'Last report', head: /^What you had reported last: "/m, quoted: true },
  { title: 'Uncommitted work', head: /^Uncommitted work(?: in [^\n]*)? \(git status --porcelain, (\d+) lines\):\n/m, count: true },
  { title: 'Diff', head: /^Diff since the last commit \(git diff --stat, (\d+) lines\):\n/m, count: true },
  { title: 'Queued messages', head: /^Messages queued for you while you were away \((\d+)\):\n/m, count: true },
  { title: 'Open todos', head: /^Open todos \((\d+)\):\n/m, count: true },
]
const OMITTED = /^\(omitted for the handoff budget: ([^\n]*)\)\n?/m
const FOOTER = /^The full earlier transcript is available with session_read\(/m

/**
 * 152 D1: a `<resume_handoff>` body as a card. Defensive: whatever does not match stays as text — a malformed note is
 * still a `resume` card with the remainder in a final `Note` section, never a raw bubble, never a lost word.
 */
function resume(text: string): Extract<Injected, { kind: 'resume' }> {
  const restored = RESTORED.exec(text)
  if (restored) {
    const [, clock, working] = restored
    return { kind: 'resume', restored: true, clock: clock as string, ...(working === undefined ? {} : { working }), sections: [], text }
  }
  const fresh = FRESH.exec(text)
  const out: Extract<Injected, { kind: 'resume' }> = { kind: 'resume', restored: false, clock: fresh?.[1] ?? '', sections: [], text }
  if (fresh?.[2] !== undefined) out.previous = fresh[2]
  // Section heads in document order; the body of one runs to the next head (or the omitted line / footer). The header
  // sentences, any unparsed lead and the footer end up in the `Note` section: every character stays readable.
  let rest = fresh ? text.slice(fresh[0].length) : text
  const note: string[] = fresh ? [fresh[0].trim()] : []
  const footer = FOOTER.exec(rest)
  const tail = footer ? rest.slice(footer.index).trim() : ''
  rest = footer ? rest.slice(0, footer.index) : rest
  const omitted = OMITTED.exec(rest)
  if (omitted) {
    out.omitted = (omitted[1] as string).split(', ')
    rest = rest.slice(0, omitted.index) + rest.slice(omitted.index + omitted[0].length)
  }
  const heads = SECTIONS.flatMap((s) => {
    const m = s.head.exec(rest)
    return m ? [{ s, at: m.index, len: m[0].length, count: m[1] === undefined ? undefined : Number(m[1]) }] : []
  }).sort((a, b) => a.at - b.at)
  const lead = rest.slice(0, heads[0]?.at ?? rest.length).trim()
  if (lead) note.push(lead)
  if (tail) note.push(tail)
  heads.forEach((h, i) => {
    const end = heads[i + 1]?.at ?? rest.length
    let body = rest.slice(h.at + h.len, end).trimEnd()
    // A quoted section ends with its closing quote unless the budget cut it (`… [N chars cut]`).
    if (h.s.quoted && body.endsWith('"')) body = body.slice(0, -1)
    out.sections.push({ title: h.s.title, text: body, ...(h.count === undefined ? {} : { count: h.count }) })
  })
  if (note.length) out.sections.push({ title: 'Note', text: note.join('\n\n') })
  return out
}

/** One line of an endless parent's view (137 §7.2) as a native child's opener carries it. */
export type ParentViewLine = { id: number; n: number; text: string; earlier: boolean }

const VIEW_LINE = /^(\d+)\+(\d+)\|([\s\S]*)$/
const EARLIER = '(earlier history, not summarized; recall searches it)'

/**
 * 152 D3: the `<chat>\n…\n</chat>` view an endless parent hands a native child before its brief (137 R1-D1), reloaded
 * as one string with the brief (`text_of` joins the parts). Only the exact shape matches — every non-empty line inside
 * is `id+n|text` — so a human typing `<chat>` keeps a normal bubble. `rest` is everything after `</chat>`.
 */
export function splitParentView(text: string): { lines: ParentViewLine[]; bytes: number; rest: string } | null {
  const t = text.trimStart()
  if (!t.startsWith('<chat>\n')) return null
  const end = t.indexOf('</chat>')
  if (end < 0) return null
  const body = t.slice('<chat>\n'.length, end)
  const lines: ParentViewLine[] = []
  for (const raw of body.split('\n')) {
    if (raw === '') continue
    const m = VIEW_LINE.exec(raw)
    if (!m) return null
    const [, id, n, line] = m as unknown as [string, string, string, string]
    lines.push({ id: Number(id), n: Number(n), text: line, earlier: id === '0' && line === EARLIER })
  }
  return { lines, bytes: new TextEncoder().encode(body).length, rest: t.slice(end + '</chat>'.length) }
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
    // A notice is its bare tag (a human's `<rounds source="user">` stays text); `<budget spent cap currency>` is the one
    // engine notice that carries attributes (152 D5).
    const notice = Object.hasOwn(ENGINE_NOTICES, open) ? open as keyof typeof ENGINE_NOTICES : name === 'budget' ? name : undefined
    if (key === undefined && !HIDDEN[name] && notice === undefined && name !== 'resume_handoff') return null
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
    } else if (name === 'resume_handoff') {
      out.push(resume(rest.slice(gt + 1, end).trim()))
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
        const why = attr(open, 'why="')
        out.push({
          kind: 'job_idle',
          id: from,
          command: unescape(attr(open, 'command="') ?? ''),
          ...(jobName === undefined ? {} : { name: unescape(jobName) }),
          quiet: attr(open, 'quiet="') ?? '',
          secs: attr(open, 'secs="') ?? '',
          nudge: attr(open, 'nudge="') ?? '',
          ...(userTyped === undefined ? {} : { userTyped }),
          ...(why === undefined || why === '' ? {} : { why: unescape(why) }),
          text: body,
        })
      } else if (name === 'schedule') {
        out.push({ kind: 'schedule', name: from, at: attr(open, 'at="') ?? '', quiet: attr(open, 'quiet="') === 'true', text: body })
      } else if (name === 'reminder') {
        out.push({ kind: 'reminder', id: from, after: attr(open, 'after="') ?? '', late: attr(open, 'late="') === 'true', text: body })
      } else if (name === 'message') {
        out.push({ kind: 'peer', from, text: body })
      } else if (name === 'advisory') {
        out.push({ kind: 'advisory', severity: from === 'concern' || from === 'blocker' ? from : 'nit', text: unescape(body.trim()) })
      } else if (name === 'child_lost') {
        out.push({ kind: 'lost', name: unescape(from), session: attr(open, 'session="') ?? '', clock: attr(open, 'at="') ?? '', why: unescape(attr(open, 'why="') ?? ''), text: body.trim() })
      } else if (name === 'autoresearch') {
        out.push({ kind: 'autoresearch', slug: unescape(from), event: unescape(attr(open, 'event="') ?? ''), text: body.trim() })
      } else if (name === 'cell_result') {
        out.push(cellResult(open, from, body))
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

/** One `<cell_result>` card from its open tag (sans `<`), its `run` and its trimmed body. */
function cellResult(open: string, run: string, text: string): CellResultInput {
  const cellName = attr(open, ' name="')
  const label = attr(open, ' label="')
  const ordinal = attr(open, ' cell="')
  const entry = attr(open, ' message="')
  return {
    kind: 'cell_result',
    run,
    ...(cellName === undefined ? {} : { name: unescape(cellName) }),
    ...(label === undefined ? {} : { label: unescape(label) }),
    ...(ordinal === undefined ? {} : { cell: ordinal }),
    ...(entry === undefined ? {} : { message: entry }),
    exit: attr(open, ' exit="') ?? '',
    secs: attr(open, ' secs="') ?? '',
    text,
  }
}

/**
 * 171F B4: a turn's text is its parked cell results joined with the user's words (`loop_.rs` `texts.join("\n")`). Peel
 * ONLY the leading `<cell_result>` blocks; `rest` is everything after them (left-trimmed), or `text` itself when none lead.
 * A block typed mid-message and every other wrapper stay in `rest`.
 */
export function splitLeading(text: string): { cells: CellResultInput[]; rest: string } {
  const cells: CellResultInput[] = []
  const close = '</cell_result>'
  let rest = text
  for (;;) {
    const s = rest.trimStart()
    if (!s.startsWith('<cell_result ')) break
    const gt = s.indexOf('>')
    const end = gt < 0 ? -1 : s.indexOf(close, gt)
    const run = end < 0 ? undefined : attr(s.slice(1, gt), CARD.cell_result as string)
    if (run === undefined) break
    const body = s.slice(gt + 1, end)
    cells.push(cellResult(s.slice(1, gt), run, (body.startsWith('\n') ? body.slice(1) : body).trimEnd()))
    rest = s.slice(end + close.length)
  }
  return { cells, rest: cells.length ? rest.trimStart() : text }
}
