import { ChevronRight, Wrench } from 'lucide-react'
import { codeDetails, resultLine } from '@/lib/describe'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import { ToolSpinner } from './Indicators'
import type { Prompt } from '@/store/approvals'
import type { Entry } from '@/store/sessions'
import { isBoardCreate } from './BoardCard'
import { Attachments } from './Attachments'
import { Message, PromptCard } from './Message'
import { Source } from './MessageTool'
import { PeerGroup } from './PeerGroup'
import { toolKey, useDisclosed } from './disclosure'
import { labelKind } from './peer-identity'

type Tool = Extract<Entry, { kind: 'tool' }>

/** A run: tool rows and job results, possibly with reasoning rows between them, starting and ending with one of those; or 2+ adjacent peer rows from one sender. */
export type Run = Entry[]

/** Deliverable tools (Sky, 2026-10-06): "Show tool cards are never folded in with the collapsed sequential tools. They
 * are always shown in the transcript; they need to be easy for the user to find." A board create (71 §2.11) and the
 * full-width artifact cards `MessageTool` draws. `routine_draft` is named ahead of its tool so that wave needs no change here.
 * `ask_user` (Sky, 2026-10-07): "answered questions shouldn't be folded in collapsed tool cards, they should always be
 * visible, like show tool calls." `desktop_handoff` is also always visible: its reason tells the owner what they must
 * do at the desktop. `secret_request` and approvals still use their prompt cards.
 * `send_message` (Sky, 2026-11-10): every send is its own card with its delivery track (`SendCard`). `show_web` (205): its
 * card reopens the web view tile; `show_pty`/`show_note` (211) likewise their terminal and note tiles, `show_file`/
 * `show_diff` (212) their file and diff tiles.
 * `browser_show_element` (209 §4.10): its card is the form the user fills, so it is never folded away. */
const DELIVERABLE_TOOLS: Record<string, true> = { show: true, show_web: true, show_pty: true, show_note: true, show_file: true, show_diff: true, app: true, agent_draft: true, task_draft: true, routine_draft: true, ask_user: true, desktop_handoff: true, send_message: true, browser_show_element: true }
export function isDeliverable(tool: Tool): boolean {
  return DELIVERABLE_TOOLS[tool.name] === true || isBoardCreate(tool)
}

type Peer = Extract<Entry, { kind: 'peer' }>
const foldsPeer = (e: Entry | undefined): e is Peer => e?.kind === 'peer' && !e.context?.removed
/** A job's result (`job j4`, `j4 · cmd · exit 0`): read from the sender, like `MessagePeer` does, never from the body. */
const isJob = (e: Entry | undefined): e is Peer => foldsPeer(e) && labelKind(e.origin?.from ?? e.from) === 'job'
/** A row a tool run is made of: a tool call that is no deliverable and carries no restart note, or a job result. */
const foldsTool = (e: Entry | undefined): boolean => isJob(e) || (e?.kind === 'tool' && !isDeliverable(e) && !e.restart && !isPythonCall(e))

/** A `python` call (without a restart note) heads its own group: the call, then the runtime calls its cell made (call ids `<id>.<n>`). */
const isPythonCall = (e: Entry | undefined): e is Tool => e?.kind === 'tool' && e.name === 'python' && !e.restart
/** A group is a `Run` that starts with a python call; the first entry of any other run is a job result or a tool that is not `python`. */
export const isPythonGroup = (run: Run): boolean => isPythonCall(run[0])
/** A finished cell that called nothing and did not fail: a `ran code` card on its own. */
const ranCodeOnly = (item: Entry | Run): item is Run => {
  const call = Array.isArray(item) && item.length === 1 ? item[0] : undefined
  return isPythonCall(call) && !call.pending && !!call.result && !call.result.isError
}
/** Two or more `ran code` cells folded into one card (`ran code × N`), reasoning between them riding along. */
export const isCodeRun = (run: Run): boolean => isPythonCall(run[0]) && run.filter(isPythonCall).length > 1

/** Sky 2026-11-22: consecutive `ran code` cells collapse into one run; unpinned reasoning between two of them rides
 * along, a trailing one stays outside. A cell with calls, a failed or running cell, or any other row ends the run. */
function foldCodeOnly(items: (Entry | Run)[], pinned: ReadonlySet<string>): (Entry | Run)[] {
  const out: (Entry | Run)[] = []
  let i = 0
  while (i < items.length) {
    const item = items[i] as Entry | Run
    if (!ranCodeOnly(item)) {
      out.push(item)
      i++
      continue
    }
    const run: Entry[] = [item[0] as Entry]
    const between: Entry[] = []
    let last = i
    for (let j = i + 1; j < items.length; j++) {
      const next = items[j] as Entry | Run
      if (!Array.isArray(next) && next.kind === 'reasoning' && !pinned.has(next.id)) between.push(next)
      else if (ranCodeOnly(next)) {
        run.push(...between, next[0] as Entry)
        between.length = 0
        last = j
      } else break
    }
    out.push(last > i ? run : item)
    i = last + 1
  }
  return out
}

/** The python call at `at` with the calls its cell made, which directly follow it. Deliverables the cell made (`show`, `ask_user`, …) are not
 * in the group: they stay plain entries after it, never folded. `end` is the index after the last call consumed. */
function pythonGroup(entries: Entry[], at: number): { group: Run; after: Entry[]; end: number } {
  const call = entries[at] as Tool
  const group: Run = [call]
  const after: Entry[] = []
  let end = at + 1
  while (end < entries.length) {
    const e = entries[end]
    if (e?.kind !== 'tool' || !e.callId.startsWith(`${call.callId}.`)) break
    ;(isDeliverable(e) ? after : group).push(e)
    end++
  }
  return { group, after, end }
}

const NONE: ReadonlySet<string> = new Set()

/** Parallel cells (two `python` calls in one message) interleave their calls: `[py1, py2, py1.1, py2.1]`. Move every
 * call a cell made to just after that cell, wherever it arrived, so `pythonGroup` finds it; a call whose cell is not in
 * `entries` stays where it is. Order within a cell is kept. */
function adoptCellCalls(entries: Entry[]): Entry[] {
  const cells = new Set<string>()
  for (const e of entries) if (isPythonCall(e)) cells.add(e.callId)
  if (cells.size < 2) return entries
  const calls = new Map<string, Entry[]>()
  const rest: Entry[] = []
  for (const e of entries) {
    const dot = e.kind === 'tool' ? e.callId.lastIndexOf('.') : -1
    const cell = dot > 0 ? (e as Tool).callId.slice(0, dot) : ''
    if (cell && cells.has(cell)) {
      const list = calls.get(cell)
      if (list) list.push(e)
      else calls.set(cell, [e])
    } else rest.push(e)
  }
  const out: Entry[] = []
  for (const e of rest) {
    out.push(e)
    if (isPythonCall(e)) out.push(...(calls.get(e.callId) ?? []))
  }
  return out
}

/** Entries with every maximal run of two or more tool cards (reasoning between them rides along; any other row,
 * including assistant text, ends the run) folded into one `Run`. A lone tool call stays a plain entry. A deliverable
 * (`isDeliverable`) is never in a run: it splits one, `[a, b] show [c, d]` → `[[a, b], show, [c, d]]`. So does a
 * `pinned` thinking block (`useToolRuns.pinned`, one the user opened while it streamed): the next tool call must not
 * fold the block the reader is watching into a collapsed card. 183 §12.4: a call carrying a restart note splits one too,
 * so its EngineRestartCard stays in view. Sky 2026-11-10: two or more adjacent peer cards from one sender fold into one
 * run too (`PeerGroup`); a row removed from context stays its own tombstone. Sky 2026-11-10: a job result (`isJob`) is a
 * tool card here: it folds into the tool run around it and never into a `PeerGroup`.
 * A `python` call is never part of such a run: it and the calls its cell made are one group (`PythonGroup`), whether or not
 * `fold` is on (the preference only governs the generic runs). */
export function foldRuns(given: Entry[], pinned: ReadonlySet<string> = NONE, fold = true): (Entry | Run)[] {
  const entries = adoptCellCalls(given)
  const out: (Entry | Run)[] = []
  let i = 0
  while (i < entries.length) {
    const e = entries[i] as Entry
    if (isPythonCall(e)) {
      const { group, after, end } = pythonGroup(entries, i)
      out.push(group, ...after)
      i = end
      continue
    }
    if (!fold) {
      out.push(e)
      i++
      continue
    }
    if (foldsTool(e)) {
      let end = i + 1
      let last = i
      while (end < entries.length && (foldsTool(entries[end]) || (entries[end]?.kind === 'reasoning' && !pinned.has(entries[end]!.id)))) {
        if (entries[end]?.kind !== 'reasoning') last = end
        end++
      }
      out.push(last > i ? entries.slice(i, last + 1) : e)
      i = last + 1
      continue
    }
    if (foldsPeer(e)) {
      // Folded by the authenticated endpoint, else the label.
      const from = e.origin?.from ?? e.from
      let end = i + 1
      while (foldsPeer(entries[end]) && ((entries[end] as Peer).origin?.from ?? (entries[end] as Peer).from) === from) end++
      out.push(end - i > 1 ? entries.slice(i, end) : e)
      i = end
      continue
    }
    out.push(e)
    i++
  }
  return fold ? foldCodeOnly(out, pinned) : out
}

export type Summary = {
  /** Header line: `running \`name\` · 3 of 5` live, `5 tools · 1 job output · 2.1 s · 1 failed (name)` once every call has a result. */
  text: string
  running: boolean
  /** Red card (Sky, 2026-11-09): more of the completed calls failed than succeeded; a tie is not red. The header still names every failure. */
  failed: boolean
  /** The last completed call's `✓/✗ first line`, when any call completed. */
  hint: string | null
}

export function summarize(run: Run): Summary {
  const tools = run.filter((e): e is Tool => e.kind === 'tool')
  const current = tools.findIndex((t) => !t.result)
  const done = tools.filter((t) => t.result)
  const failed = done.filter((t) => t.result?.isError)
  const mostlyFailed = failed.length > done.length - failed.length
  const last = done[done.length - 1]
  const hint = last?.result ? `${last.result.isError ? '✗' : '✓'} ${resultLine(last.result.output)}` : null
  if (current >= 0) {
    return { text: `running \`${tools[current]?.name}\` · ${current + 1} of ${tools.length}`, running: true, failed: mostlyFailed, hint }
  }
  const jobs = run.filter(isJob).length
  const parts = [tools.length ? `${tools.length} tool${tools.length === 1 ? '' : 's'}` : '', jobs ? `${jobs} job output${jobs === 1 ? '' : 's'}` : ''].filter(Boolean)
  const start = tools[0]?.startedAt
  const end = tools[tools.length - 1]?.finishedAt
  // `2.1 s` under a minute, `3 min` from there.
  if (start !== undefined && end !== undefined) parts.push(end - start < 60_000 ? `${((end - start) / 1000).toFixed(1)} s` : `${Math.round((end - start) / 60_000)} min`)
  if (failed.length) parts.push(`${failed.length} failed (${failed.map((t) => t.name).join(', ')})`)
  return { text: parts.join(' · '), running: false, failed: mostlyFailed, hint }
}

/** Header of a code run: `ran code × 3 · 1.2 s` (the cells' own durations added up). */
export function summarizeCells(run: Run): Summary {
  const cells = run.filter(isPythonCall)
  const ms = cells.reduce((sum, c) => sum + (c.startedAt !== undefined && c.finishedAt !== undefined ? c.finishedAt - c.startedAt : 0), 0)
  const parts = [`ran code × ${cells.length}`]
  if (ms > 0) parts.push(ms < 60_000 ? `${(ms / 1000).toFixed(1)} s` : `${Math.round(ms / 60_000)} min`)
  return { text: parts.join(' · '), running: false, failed: false, hint: null }
}

export type GroupSummary = Summary & { /** Calls the cell made: the cards under the header. */ calls: number }

/** Header of a python group: `3 tools` (the calls its cell made, deliverables shown beside the group not counted), `running · 3 tools` while the cell runs, `ran code` for a cell that called nothing,
 * then the duration and the failed calls. The word "python" never appears. A cell that errors itself is red and shows its first error line (`hint`). */
export function summarizeGroup(group: Run): GroupSummary {
  const call = group[0] as Tool
  const calls = group.slice(1).filter((e): e is Tool => e.kind === 'tool')
  const done = calls.filter((t) => t.result)
  const failed = done.filter((t) => t.result?.isError)
  const running = !call.interrupted && (call.pending || !call.result)
  const last = done[done.length - 1]
  const lastLine = last?.result ? `${last.result.isError ? '✗' : '✓'} ${resultLine(last.result.output)}` : null
  const crashed = call.result?.isError === true
  const hint = crashed && call.result ? `✗ ${resultLine(call.result.output)}` : lastLine
  const count = calls.length ? `${calls.length} tool${calls.length === 1 ? '' : 's'}` : running ? '' : 'ran code'
  const parts = [running ? ['running', count].filter(Boolean).join(' · ') : count]
  if (!running && call.startedAt !== undefined && call.finishedAt !== undefined) {
    const ms = call.finishedAt - call.startedAt
    parts.push(ms < 60_000 ? `${(ms / 1000).toFixed(1)} s` : `${Math.round(ms / 60_000)} min`)
  }
  if (failed.length) parts.push(`${failed.length} failed (${failed.map((t) => t.name).join(', ')})`)
  if (crashed) parts.push('code failed')
  return { text: parts.join(' · '), running, failed: crashed || failed.length > done.length - failed.length, hint, calls: calls.length }
}

/** A folded run: consecutive peer cards from one sender (`PeerGroup`), a python call with its runtime calls (`PythonGroup`), or tool calls and job results (`ToolRun`). */
export function ToolRunCard({ run, prompts, session }: { run: Run; prompts: Prompt[] | null; session: string }) {
  if (isCodeRun(run)) return <CodeRun run={run} prompts={prompts} session={session} />
  if (isPythonGroup(run)) return <PythonGroup group={run} prompts={prompts} session={session} />
  return run[0]?.kind === 'peer' && !isJob(run[0]) ? <PeerGroup run={run as Peer[]} session={session} /> : <ToolRun run={run} prompts={prompts} session={session} />
}

/** One card for a run of consecutive tool calls (70 TOOL-COLLAPSE): collapsed by default to a live status line, expanded
 * to the same cards the transcript shows ungrouped. Open state lives in the disclosure store keyed by the first call id
 * (a job result's entry id when the run starts with one), so it survives the settled reload (new entry ids, same call
 * ids); until the user toggles the run it is open whenever a member's own card is (`toolKey`, a job card's `peer`), so
 * the next call folding an expanded tool card into a run keeps it on screen (171F B13).
 * Prompts waiting on a call in the run render under the header whether or not it is open: an approval must never hide
 * inside a collapsed run. */
function ToolRun({ run, prompts, session }: { run: Run; prompts: Prompt[] | null; session: string }) {
  const first = run[0] as Entry
  const member = useDisclosed((open) =>
    run.some((e) => (e.kind === 'tool' ? open[toolKey(e.callId)] : e.kind === 'peer' && open[disclosureKey(e.id, 'peer')]) === true),
  )
  const [open, setOpen] = useDisclosure(disclosureKey('run', first.kind === 'tool' ? first.callId : first.id), member)
  const { text, running, failed, hint } = summarize(run)
  return (
    <>
      <div className={cn('my-2 min-w-0 rounded-xl border bg-layer-1 px-4 py-3 text-sm', failed ? 'border-danger-border' : 'border-border-subtle')}>
        <button type="button" className="flex w-full min-w-0 flex-col gap-1 text-left" aria-expanded={open} onClick={() => setOpen(!open)}>
          <div className={cn('flex items-center gap-2 font-medium', failed && 'text-danger')}>
            <ChevronRight className={cn('size-4 shrink-0 transition-transform', open && 'rotate-90')} />
            <Wrench className="size-3.5 shrink-0 text-fg-secondary" />
            <span className="min-w-0 truncate">{text}</span>
            {running && <ToolSpinner />}
          </div>
          {hint && !open && (
            <div className={cn('w-full truncate pl-6 font-mono text-xs', hint.startsWith('✗') ? 'text-danger' : 'text-success')}>{hint}</div>
          )}
        </button>
      </div>
      {open && run.map((e) => <Message key={e.id} entry={e} prompts={null} session={session} />)}
      {prompts?.map((p) => <PromptCard key={p.id} prompt={p} />)}
    </>
  )
}

/** Consecutive cells that called nothing (`isCodeRun`): one collapsed `ran code × N` card; expanded, each cell's own
 * `ran code` card (its source one click further) and the reasoning between them. Open state: `cells|<first call id>`. */
function CodeRun({ run, prompts, session }: { run: Run; prompts: Prompt[] | null; session: string }) {
  const first = run[0] as Tool
  const [open, setOpen] = useDisclosure(disclosureKey('cells', first.callId))
  const { text } = summarizeCells(run)
  return (
    <>
      <div className="my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm">
        <button type="button" className="flex w-full min-w-0 items-center gap-2 text-left font-medium" aria-expanded={open} onClick={() => setOpen(!open)}>
          <ChevronRight className={cn('size-4 shrink-0 transition-transform', open && 'rotate-90')} />
          <Wrench className="size-3.5 shrink-0 text-fg-secondary" />
          <span className="min-w-0 truncate">{text}</span>
        </button>
      </div>
      {open &&
        run.map((e) =>
          isPythonCall(e) ? <PythonGroup key={e.id} group={[e]} prompts={null} session={session} /> : <Message key={e.id} entry={e} prompts={null} session={session} />,
        )}
      {prompts?.map((p) => <PromptCard key={p.id} prompt={p} />)}
    </>
  )
}

/** One card for a `python` call and the runtime calls its cell made, in three modes: collapsed (`> N tools`), expanded
 * (`v N tools`: a collapsed `> code` row, then the calls' own cards below), code expanded (`v code`: the source, the
 * cell's output when it failed or the source is open). Open state is in the disclosure store (`run|<call id>`, `code|<call id>`).
 * Until the user toggles the group it is open whenever a member's own card is, like `ToolRun`. A failed call or a cell that errors keeps the
 * header red and, collapsed, its first line visible; prompts waiting on a call render under the header either way. */
function PythonGroup({ group, prompts, session }: { group: Run; prompts: Prompt[] | null; session: string }) {
  const call = group[0] as Tool
  const calls = group.slice(1)
  const member = useDisclosed((open) => calls.some((e) => e.kind === 'tool' && open[toolKey(e.callId)] === true))
  const [open, setOpen] = useDisclosure(disclosureKey('run', call.callId), member)
  const [codeOpen, setCodeOpen] = useDisclosure(disclosureKey('code', call.callId))
  const { text, running, failed, hint } = summarizeGroup(group)
  const source = codeDetails(call.name, call.args)
  const { result } = call
  return (
    <>
      <div className={cn('my-2 min-w-0 rounded-xl border bg-layer-1 px-4 py-3 text-sm', failed ? 'border-danger-border' : 'border-border-subtle')}>
        <button type="button" className="flex w-full min-w-0 flex-col gap-1 text-left" aria-expanded={open} onClick={() => setOpen(!open)}>
          <div className={cn('flex items-center gap-2 font-medium', failed && 'text-danger')}>
            <ChevronRight className={cn('size-4 shrink-0 transition-transform', open && 'rotate-90')} />
            <Wrench className="size-3.5 shrink-0 text-fg-secondary" />
            <span className="min-w-0 truncate">{text}</span>
            {running && <ToolSpinner />}
          </div>
          {hint && !open && (
            <div className={cn('w-full truncate pl-6 font-mono text-xs', hint.startsWith('✗') ? 'text-danger' : 'text-success')}>{hint}</div>
          )}
        </button>
        {open && source && (
          <div className="mt-2 pl-6">
            <button type="button" className="flex w-full items-center gap-2 text-left font-medium" aria-expanded={codeOpen} onClick={() => setCodeOpen(!codeOpen)}>
              <ChevronRight className={cn('size-4 shrink-0 transition-transform', codeOpen && 'rotate-90')} />
              <span>code</span>
            </button>
            {codeOpen && <Source code={source.code} language={source.language} />}
          </div>
        )}
        {open && result && (result.isError || codeOpen) && result.output && (
          <pre className={cn('mt-2 ml-6 max-h-56 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap', result.isError && 'text-danger')}>
            {result.output}
          </pre>
        )}
        {result && <Attachments files={result.files} occurrences={call.context?.attachments} session={session} active={call.context?.active} className="mt-2" />}
      </div>
      {open && calls.map((e) => <Message key={e.id} entry={e} prompts={null} session={session} />)}
      {prompts?.map((p) => <PromptCard key={p.id} prompt={p} />)}
    </>
  )
}
