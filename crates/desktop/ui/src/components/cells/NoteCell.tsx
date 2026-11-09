// Owner: U2
import type { BlockNoteEditor } from '@blocknote/core'
import { useEditorState } from '@blocknote/react'
import { useQuery } from '@tanstack/react-query'
import { ChevronDown, Play } from 'lucide-react'
import { type CSSProperties, type MouseEvent, type ReactNode, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { machineName, inMesh } from '@/components/notes/machine'
import { useMesh } from '@/components/tasks/queries'
import { LOCAL, LOCAL_NAME, useEngines } from '@/link/engines'
import { isApple } from '@/lib/chord'
import { useApi } from '@/lib/rpc'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import type { Api } from '@/rpc/api'
import { RpcError } from '@/rpc/client'
import type { CellLatest } from '@/rpc/bindings/CellLatest'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRunMode } from '@/rpc/bindings/CellRunMode'
import type { CellScope } from '@/rpc/bindings/CellScope'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { DocId } from '@/rpc/bindings/DocId'
import type { NoteKey } from '@/rpc/bindings/NoteKey'
import { Cell, CellExtras, Chip, Notice } from './Cell'
import { CodeTokens } from './CodeTokens'
import { AnsiText } from './Outputs'
import { digest } from './grammar'
import { CONFIRM_ROOM, LineConfirm } from './LineConfirm'
import { type Ask, type AskEvent, askSkipped, askStep, cellIdentity, lineGuard, MARK_LOOK, noteHover, useLineRun } from './line-run'
import { historyAfter } from './open-as-note'
import { headerFolded, lineDiff, lineStart, memberName, modeOf, pythonValue, withPython } from './note-edit'
import { NoteHost } from './note-context'
import { clock, errorText, lastPass, mintId, press, refKey, refresh, register, scopeKey, scopeOf, setNotice, useCellRecord, useCells, useLatestRow } from './store'
import { commandUnits, unitLabel, unitMode, unitOf } from './units'
import { disarmedReason, interpreterText, labelOf, RUN_ANYWAY, titleOf, viewOf } from './view'
import './note-cells.css'

export type NoteCellProps = {
  doc: DocId
  /** `Name(name)` when the header names the cell, else `Block(id)`. */
  key_: NoteKey
  cell: CellSpec
  /** The block's whole text (header + code): what the press digests (§3.4). */
  code: string
  /** BlockNote's own code content (`contentRef`), §4.2. */
  editor: ReactNode
  editable: boolean
  // 171:U2-ADD — the BlockNote block and editor: run-line/run-selection read its selection, the python menu edits it.
  block: string
  blocknote: BlockNoteEditor<any, any, any>
}

/** Every mounted note cell's ref and text on screen (by `refKey`), for the header's Run all (§11.5). */
const mounted = new Map<string, { ref: CellRef; text: string }>()

/** The note's mounted cells top to bottom (document = DOM order) with the text on screen. */
export function mountedCells(doc: DocId): { ref: CellRef; text: string }[] {
  const keys = new Set([...document.querySelectorAll<HTMLElement>('[data-note-cell][data-ref-key]')].map((el) => el.dataset.refKey ?? ''))
  return [...keys].flatMap((k) => {
    const hit = mounted.get(k)
    return hit?.ref.type === 'note' && hit.ref.doc === doc ? [hit] : []
  })
}

/**
 * One press of a note cell (§15.2 `cell_run`): the digest of the text on screen as `seen`, a client-minted id retried
 * on transport failures, no edited text (the note IS the text), `member_ack` = the member's pub8 when the engine
 * reported a member edit (invariant 6, "Run Alice's edit"), and `confirmed` = the press past a warning (Run anyway,
 * a danger cell's second ►).
 */
export async function pressNote(api: Pick<Api, 'cell_run'>, ref: CellRef, text: string, mode: CellRunMode, memberAck: string | null, confirmed = false) {
  const seen = await digest(text)
  return press(mintId('r_'), (id) => api.cell_run(ref, seen, mode, id, null, memberAck, confirmed))
}

/**
 * 171F4 N1: one re-read of the note's record per settled edit, `ms` after the last change (each change restarts the
 * wait): the engine's stale mark ("ran an earlier version") and its validation follow the text, never per keystroke.
 */
export function rereadAfterEdit(scope: CellScope, ms = 800) {
  let stop = () => {}
  return {
    edit() {
      stop()
      const t = setTimeout(() => void refresh(scope), ms)
      stop = () => clearTimeout(t)
    },
    stop: () => stop(),
  }
}

/** `just now`, `4 min ago`, `3 h ago`, `2 d ago`. */
function ageText(ms: number, now = Date.now()): string {
  const m = Math.floor(Math.max(0, now - ms) / 60_000)
  return m < 1 ? 'just now' : m < 60 ? `${m} min ago` : m < 1440 ? `${Math.floor(m / 60)} h ago` : `${Math.floor(m / 1440)} d ago`
}

/** §6.5, §12.3: a result another engine ran, from `cell_latest`: its text, `ran on xeus · 4 min ago · exit 0`, the expect verdict. Nothing to attach to, stop or open. */
function ForeignRun({ row, machine }: { row: CellLatest; machine: string }) {
  const end = row.exit !== null ? `exit ${row.exit}` : row.state.replaceAll('_', ' ')
  return (
    <div className="flex flex-col gap-1 border-t border-border-subtle px-3 py-2">
      {row.text && <AnsiText text={row.text} className="max-h-[28rem] overflow-auto text-xs whitespace-pre" />}
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-secondary">
        <span className="tabular-nums">
          <span className="text-fg-primary">ran on {machine}</span> · {ageText(Number(row.ended_ms))} · {end}
          {row.expect && ` · ${row.expect}`}
        </span>
      </div>
    </div>
  )
}

/** §12.3: the press `MemberAck("?")` refused (nobody can say who last edited the text): the member-ack confirm. */
export function UnsureRun({ server, onRun, onCancel }: { server: string; onRun(): void; onCancel(): void }) {
  const cancel = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    // Inline, not modal (a modal loops ProseMirror): focus moves in, and back to whatever had it (the cell's run button) on close.
    // Cancel, never Run: the confirm follows the user's own Run press, so a second Enter must not run unread text.
    const from = document.activeElement
    cancel.current?.focus()
    return () => {
      if (from instanceof HTMLElement && from.isConnected) from.focus()
    }
  }, [])
  return (
    <Notice tone="warning">
      <div
        role="dialog"
        aria-label="Run this cell?"
        className="flex flex-col gap-1.5"
        onKeyDown={(e) => e.key === 'Escape' && (e.stopPropagation(), onCancel())}
      >
        <b>Run this cell?</b>
        <span>This note is shared and {server} can't be reached, so theuth can't tell who last edited this cell. Read it before you run it.</span>
        <span className="flex gap-1.5">
          <button type="button" className="underline underline-offset-2 pointer-coarse:min-h-12" onClick={onRun}>
            Run
          </button>
          <button ref={cancel} type="button" className="underline underline-offset-2 pointer-coarse:min-h-12" onClick={onCancel}>
            Cancel
          </button>
        </span>
      </div>
    </Notice>
  )
}

const decode = (b64: string) => new TextDecoder().decode(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)))

type PmSelection = BlockNoteEditor<any, any, any>['prosemirrorState']['selection']

/** Start of block `id`'s code text when `$pos` is inside it, else `null`. */
function codeStart($pos: PmSelection['$from'], id: string): number | null {
  for (let d = $pos.depth; d > 0; d--) if ($pos.node(d).type.name === 'codeBlock') return $pos.node(d - 1).attrs.id === id ? $pos.start(d) : null
  return null
}

/** `"from:to"` (offsets in the block's text) when the selection lies inside block `id`, else `null`. */
function selectionIn(sel: PmSelection, id: string): string | null {
  const at = codeStart(sel.$from, id)
  return at !== null && codeStart(sel.$to, id) === at ? `${sel.from - at}:${sel.to - at}` : null
}

/** The text position under the pointer (WebKit/Blink `caretRangeFromPoint`, Gecko `caretPositionFromPoint`). */
function caretAt(x: number, y: number): { node: Node; offset: number } | null {
  if (typeof document.caretRangeFromPoint === 'function') {
    const r = document.caretRangeFromPoint(x, y)
    return r && { node: r.startContainer, offset: r.startOffset }
  }
  const p = document.caretPositionFromPoint?.(x, y)
  return p ? { node: p.offsetNode, offset: p.offset } : null
}

/** The top of the code-text character at `offset` in `slot`'s code element, relative to `slot` (0 when unknown). */
function lineTop(slot: HTMLElement | null, offset: number): number {
  const codeEl = slot?.querySelector('code')
  if (!slot || !codeEl) return 0
  const walk = document.createTreeWalker(codeEl, NodeFilter.SHOW_TEXT)
  for (let n = walk.nextNode(), left = offset; n; n = walk.nextNode()) {
    const len = n.textContent?.length ?? 0
    if (left < len) {
      const range = document.createRange()
      range.setStart(n, left)
      range.setEnd(n, left + 1)
      return Math.round(range.getBoundingClientRect().top - slot.getBoundingClientRect().top)
    }
    left -= len
  }
  return 0
}

/** "Review diff" / "Review": the owner's last run's code against the member's text, or the text alone if never run. */
function MemberReview({ run, code }: { run: string | null; code: string }) {
  const api = useApi()
  const q = useQuery({ queryKey: ['cells', 'code', run], queryFn: async () => decode(await api.cell_output_file(run as string, 'code')), enabled: run !== null, staleTime: Infinity })
  if (q.error) return <div className="px-3 py-1 text-xs text-danger">{errorText(q.error)}</div>
  const rows = run === null ? code.split('\n').map((line) => ({ op: '+' as const, line })) : q.data === undefined ? null : lineDiff(q.data.replace(/\n$/, ''), code)
  if (!rows) return <div className="px-3 py-1 text-xs text-fg-tertiary">loading the code of your last run…</div>
  return (
    <pre className="mx-3 mb-2 max-h-64 overflow-auto rounded-md border border-border-subtle font-mono text-xs">
      {rows.map((r, i) => (
        <div key={i} className={cn('px-2', r.op === '+' && 'bg-success-bg text-success', r.op === '-' && 'bg-danger-bg text-danger')}>
          {r.op} {r.line}
        </div>
      ))}
    </pre>
  )
}

/** §3.5 stale: the code the latest run ran, one click away (`runs/<run>/code` through `cell_output_file`). */
function RanCode({ run, lang }: { run: string; lang: string }) {
  const api = useApi()
  const q = useQuery({ queryKey: ['cells', 'code', run], queryFn: async () => decode(await api.cell_output_file(run, 'code')), staleTime: Infinity })
  if (q.error) return <div className="px-3 py-1 text-xs text-danger">{errorText(q.error)}</div>
  return <pre className="mx-3 mb-2 max-h-64 overflow-auto rounded-md border border-border-subtle px-2 py-1 font-mono text-xs text-fg-secondary">{q.data === undefined ? 'loading…' : <CodeTokens code={q.data} lang={lang} />}</pre>
}

/** §6.3: the python chip as a menu of `cell_interpreters(cwd)`; a pick rewrites the `#| python:` header. */
function PythonChip({ text, cwd, at, onPick }: { text: string; cwd: string; at: string; onPick(value: string): void }) {
  const api = useApi()
  const [open, setOpen] = useDisclosure(disclosureKey(at, 'python'))
  const q = useQuery({ queryKey: ['cells', 'interpreters', cwd], queryFn: () => api.cell_interpreters(cwd, null), enabled: open, staleTime: 60_000 })
  const picks = (q.data ?? []).filter((i) => i.kind === 'python' || i.kind === 'monty')
  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <button type="button" className="inline-flex rounded-md pointer-coarse:min-h-12" aria-label={`Interpreter: ${text}`}>
          <Chip>
            {text}
            <ChevronDown className="ml-0.5 inline size-3" aria-hidden />
          </Chip>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        {q.error && <DropdownMenuItem disabled>{errorText(q.error)}</DropdownMenuItem>}
        {q.isLoading && <DropdownMenuItem disabled>looking for interpreters…</DropdownMenuItem>}
        {q.data && picks.length === 0 && <DropdownMenuItem disabled>no python found</DropdownMenuItem>}
        {picks.map((i) => (
          <DropdownMenuItem key={`${i.source}:${i.path ?? ''}`} onSelect={() => onPick(pythonValue(i))} className="flex-col items-start gap-0">
            <span className="text-xs">
              {i.kind === 'monty' ? 'Monty · sandboxed' : `Python ${i.version ?? ''} · ${i.source}`}
              {i.auto && ' · auto'}
            </span>
            <span className="text-[11px] text-fg-tertiary">{i.why}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/**
 * 171 §4.2, §11.1, §11.5: a note code block with a `#|` header, mounted from `editor/CodeBlock.tsx`. U1's `Cell`
 * chrome around BlockNote's own code content; the run state is the engine's record for `Scope::Doc` (never the
 * Y.Doc). Note-only: run-line ► and the selection pill read the BlockNote selection; stale shows the code that ran;
 * a member's edit must be acknowledged ("Run Alice's edit" sends `member_ack`); "ran 14:02" in the footer.
 */
export function NoteCell({ doc, key_, cell, code, editor, editable, block, blocknote }: NoteCellProps) {
  const api = useApi()
  const host = useContext(NoteHost)
  const ref: CellRef = { type: 'note', doc, key: key_ }
  const key = refKey(ref)
  const scope = scopeOf(ref)
  const title = titleOf(cell)
  const label = labelOf(cell)
  // Registered by ref (`key`) and title only: re-registering per keystroke would re-read the record on every edit.
  useEffect(() => register(api, scopeOf(ref), { ref, spec: cell, label: title }), [api, key, title])
  mounted.set(key, { ref, text: code })
  useEffect(() => () => void mounted.delete(key), [key])
  // 171F4 N1: any edit of the block (code or header) re-reads the record once it settles, so validation and the
  // stale mark follow the text (171F B10 re-read headers only).
  const reread = useMemo(() => rereadAfterEdit(scope), [key])
  const seenCode = useRef(code)
  useEffect(() => {
    if (seenCode.current === code) return
    seenCode.current = code
    reread.edit()
  }, [code, reread])
  useEffect(() => reread.stop, [reread])

  const { state: rec, notice } = useCellRecord(ref)
  // §6.5: the newer, by `ended_ms`, of this engine's run and the replicated row; a running own run is the newest. In a mesh only.
  const { data: mesh } = useMesh()
  const local = useEngines((s) => s.active === LOCAL)
  const row = useLatestRow(scope, `${doc}:${key_.value}`)
  const own = rec?.latest ?? null
  const foreign = inMesh(mesh) && row && row.engine !== '' && row.engine !== mesh?.self_id && (!own || (own.ended_ms != null && row.ended_ms > BigInt(own.ended_ms))) ? row : null
  const record = foreign && rec ? { ...rec, latest: null } : rec
  const latest = record?.latest ?? null
  const pub8 = record?.member_edit ?? null
  const member = pub8 ? memberName(pub8, host?.provider.meta?.members) : null
  const view = viewOf(record, cell, false)
  const danger = cell.options.danger
  // CELLAFTER (§7.1, §7.6): the gate's reason while it is not armed. The button runs anyway; a line/selection ► of a
  // disarmed or danger cell warns first and runs on its second press. Only those presses reach `run` (keys hint).
  const gate = disarmedReason(record, cell)
  const [hint, setHint] = useState<string | null>(null)
  const [review, setReview] = useDisclosure(disclosureKey(key, 'review'))
  const [ranCode, setRanCode] = useDisclosure(disclosureKey(key, 'ran-code'))
  useEffect(() => {
    if (!hint) return
    const t = setTimeout(() => setHint(null), 4000)
    return () => clearTimeout(t)
  }, [hint])
  // 171F4 T2-3: the chat run's history Open as note wrote after this cell gives way to the cell's own first run, so
  // the note holds one output per cell (the owner's editor edits the note; members and `doc_read` read the result).
  const hasRun = latest !== null
  useEffect(() => {
    if (!hasRun || !editable) return
    const old = historyAfter(blocknote.document, block)
    if (old.length) blocknote.removeBlocks(old)
  }, [hasRun, editable, blocknote, block])

  const act = (f: () => Promise<unknown>, handled?: (e: unknown) => boolean) => {
    setNotice(ref, null)
    f().then(
      () => refresh(scope),
      (e: unknown) => {
        if (!handled?.(e)) setNotice(ref, errorText(e))
        void refresh(scope)
      },
    )
  }
  // `confirmed` only from the presses past a warning: Run anyway, the second ►, the danger confirm (or its "Don't ask
  // again"). `text`: the cell text the press was made on (an ask's press-time text; else what is on screen now).
  const run = (mode: CellRunMode, confirmed = false, text = code, ack = pub8) => {
    const pressed = pressNote(api, ref, text, mode, ack, confirmed)
    // §6.2, §12.3: the author is unknown (`MemberAck("?")`): the press waits for the user's go.
    act(
      () => pressed,
      (e) => e instanceof RpcError && (e.data as { reason?: string; member?: string } | null)?.member === '?' && (setUnsure({ mode, confirmed, text }), true),
    )
    return pressed
  }
  const [unsure, setUnsure] = useState<{ mode: CellRunMode; confirmed: boolean; text: string } | null>(null)
  const callbacks = {
    onRun: run,
    onStop: (kill: boolean) => latest && act(() => api.cell_stop(latest.id, kill)),
    onInput: (data: string) => latest && act(() => api.cell_input(latest.id, btoa(Array.from(new TextEncoder().encode(data), (b) => String.fromCharCode(b)).join('')))),
    onAsk: () => latest && act(() => api.cell_ask(latest.id)),
    // The note is the text: edits go through BlockNote, never through the cell.
    onEdit: () => undefined,
    onAttach: () => () => undefined,
  }

  // 171F4 T2-6: a line/selection run marks its first code line in the gutter (running, then ✓/✗ when it printed
  // nothing); a repeat press of a line whose run is in flight does nothing. CELLAFTER: the line ► runs a command
  // (`units.ts`): its mark sits on the command's first line.
  const slot = useRef<HTMLDivElement>(null)
  const lineRun = useLineRun(latest)
  const markTop = useRef(0)
  const units = useMemo(() => commandUnits(cell.code, cell.lang), [cell.code, cell.lang])
  const topOf = (line: number) => lineTop(slot.current, lineStart(code, cell, line))
  const runAt = (line: number, mode: CellRunMode, top?: number, warning: string | null = null, confirmed = false, text = code) => {
    markTop.current = top ?? topOf(line)
    lineRun.run(line, () => run(mode, confirmed, text), warning)
  }
  // A pointer ► (§7.1, §7.6): a danger cell asks first (unless "Don't ask again"), a disarmed one warns first.
  const [ask, setAsk] = useState<Ask | null>(null)
  const identity = cellIdentity(ref, cell)
  const pointAt = (line: number, last: number, mode: CellRunMode, what: 'line' | 'selection', top = topOf(line)) => {
    const g = lineGuard(danger, askSkipped(identity), gate)
    if (g.kind === 'confirm') return setAsk({ line, last, mode, what, text: code, reason: g.reason, identity })
    // A warned ► runs on its second press, past the gate; a danger ► the user stopped being asked about runs confirmed.
    runAt(line, mode, top, g.kind === 'warn' ? g.text : null, g.kind === 'warn' || danger)
  }
  const step = (e: AskEvent) => {
    if (!ask) return
    const s = askStep(ask, e)
    setAsk(s.ask)
    if (s.run) runAt(s.run.line, s.run.mode, undefined, null, true, s.run.text)
  }
  // M1: any change of the block's text closes an open ask (the run would no longer be what was confirmed).
  useEffect(() => setAsk((a) => a && askStep(a, { kind: 'code', text: code }).ask), [code])

  // §7.1/§11.7 keys inside the code block: Shift+Enter runs the cell and moves to the next one, Mod+Enter runs the
  // selection or the caret line, Mod+. stops. The caret is in BlockNote's root editable, so the listener sits on the
  // document (capture: before BlockNote's own code-block keys).
  // `editor` is null in a server render (BlockNote's snapshot before mount).
  const sel = useEditorState({ editor: blocknote, selector: ({ editor: e }) => (e ? selectionIn(e.prosemirrorState.selection, block) : null) })
  const anyway = member ? `Run ${member}'s edit anyway` : RUN_ANYWAY
  const keys = useRef({ code, cell, run, runAt, units, view, gate, latest, label, anyway, stop: callbacks.onStop })
  keys.current = { code, cell, run, runAt, units, view, gate, latest, label, anyway, stop: callbacks.onStop }
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = isApple() ? e.metaKey : e.ctrlKey
      if (!(e.target instanceof Node) || !blocknote.domElement?.contains(e.target)) return
      const at = selectionIn(blocknote.prosemirrorState.selection, block)
      if (at === null) return
      const k = keys.current
      if (e.key === '.' && mod) {
        e.preventDefault()
        if (k.latest && (k.latest.state === 'running' || k.latest.state === 'queued')) k.stop(false)
        return
      }
      if (e.key !== 'Enter' || !(e.shiftKey || mod) || (e.shiftKey && mod)) return
      e.preventDefault()
      e.stopPropagation()
      if (k.cell.options.danger || k.gate !== null || k.view === 'invalid')
        return setHint(`press ${k.gate !== null && k.view !== 'invalid' ? k.anyway : k.label} to run this`)
      if (e.shiftKey) {
        k.run({ type: 'cell' })
        const cells = [...document.querySelectorAll<HTMLElement>('[data-note-cell][data-block]')]
        const next = cells[cells.findIndex((el) => el.dataset.block === block) + 1]?.dataset.block
        if (next) blocknote.setTextCursorPosition(next, 'start')
        return
      }
      const [from, to] = at.split(':').map(Number) as [number, number]
      const mode = modeOf(k.code, k.cell, from, to)
      const caret = modeOf(k.code, k.cell, from, from)
      // The caret line runs its whole command, as its ► does.
      const unit = mode?.type === 'line' ? unitOf(k.units, mode.line) : null
      if (unit) k.runAt(unit[0], unitMode(k.cell.code, unit))
      else if (mode) k.runAt(caret?.type === 'line' ? caret.line : 1, mode)
      else setHint('put the caret on a code line to run it')
    }
    document.addEventListener('keydown', onKey, true)
    return () => document.removeEventListener('keydown', onKey, true)
    // The latest values ride `keys`; the listener is per block.
  }, [blocknote, block])

  // The gutter ► (§11.1): a positioned overlay at the hovered command's first line (danger-toned on a danger cell);
  // any visual row of its lines (soft wrap included) shows the one ►.
  const [hover, setHover] = useState<{ line: number; top: number } | null>(null)
  const onMove = (e: MouseEvent<HTMLDivElement>) => {
    if (e.target instanceof Element && e.target.closest('button')) return
    const codeEl = slot.current?.querySelector('code')
    if (!codeEl || !slot.current) return
    // 171F4 N2: the band before the code text, where the ► sits, keeps the hovered line (`noteHover`); over the
    // text the line under the pointer; leaving the slot clears it.
    const textLeft = codeEl.getBoundingClientRect().left
    const at = e.clientX < textLeft ? null : caretAt(e.clientX, e.clientY)
    if (e.clientX >= textLeft && (!at || !codeEl.contains(at.node))) return
    let line: number | null = null
    let top = hover?.top ?? 0
    if (at) {
      const range = document.createRange()
      range.setStart(codeEl, 0)
      range.setEnd(at.node, at.offset)
      const offset = range.toString().length
      const mode = modeOf(code, cell, offset, offset)
      if (mode?.type === 'line') {
        line = unitOf(units, mode.line)[0]
        top = topOf(line)
      }
    }
    const next = noteHover(hover?.line ?? null, e.clientX, textLeft, line)
    if (next === null) return setHover(null)
    if (hover?.line !== next || hover.top !== top) setHover({ line: next, top })
  }
  const [selFrom, selTo] = sel ? (sel.split(':').map(Number) as [number, number]) : [0, 0]
  const selMode = sel && selFrom !== selTo ? modeOf(code, cell, selFrom, selTo) : null
  const selLine = modeOf(code, cell, Math.min(selFrom, selTo), Math.min(selFrom, selTo))
  const selEnd = modeOf(code, cell, Math.max(selFrom, selTo), Math.max(selFrom, selTo))
  const mod = isApple() ? '⌘' : 'Ctrl'
  const mark = lineRun.mark
  // 171F4 N3: the `#|` lines fold behind the "#|" pill (the chips show the same options); display only, the text
  // keeps them. Shown while the caret is in them or the pill is on. The fold pulls the code up by the header's height
  // and clips the header (note-cells.css), so the caret can still arrow up into it.
  const [pill] = useDisclosure(disclosureKey(key, 'raw'))
  const folded = headerFolded(code, cell, sel ? [selFrom, selTo] : null, pill)
  const [foldPx, setFoldPx] = useState(0)
  // ponytail: measured per edit and fold change; a late font swap leaves it stale until the next one.
  useLayoutEffect(() => {
    if (folded) setFoldPx(lineTop(slot.current, lineStart(code, cell, 1)) - lineTop(slot.current, 0))
  }, [folded, code])
  const fold = folded && foldPx > 0 ? ({ '--header-fold': `${foldPx}px` } as CSSProperties) : undefined

  const slotted = (
    <div ref={slot} className={cn('relative', ask && CONFIRM_ROOM)} data-header-fold={fold ? '' : undefined} style={fold} onMouseMove={onMove} onMouseLeave={() => setHover(null)}>
      {editor}
      {mark && (
        <span
          contentEditable={false}
          role="status"
          aria-label={MARK_LOOK[mark.phase].text}
          title={MARK_LOOK[mark.phase].text}
          className={cn('pointer-events-none absolute left-1.5 flex size-5 items-center justify-center text-xs pointer-coarse:size-12', MARK_LOOK[mark.phase].tone)}
          style={{ top: markTop.current }}
        >
          {MARK_LOOK[mark.phase].glyph}
        </span>
      )}
      {lineRun.warn && (
        <span
          contentEditable={false}
          className="pointer-events-none absolute right-2 z-10 rounded bg-warning-bg px-2 py-0.5 text-[11px] text-warning shadow-sm"
          style={{ top: markTop.current }}
        >
          {lineRun.warn.text}
        </span>
      )}
      {hover && hover.line !== mark?.line && !ask && (
        <button
          type="button"
          contentEditable={false}
          className={cn('absolute left-1.5 flex size-5 items-center justify-center rounded hover:bg-layer-2 pointer-coarse:size-12', danger ? 'text-danger' : 'text-fg-accent')}
          style={{ top: hover.top }}
          aria-label={unitLabel(unitOf(units, hover.line))}
          title={danger ? `${unitLabel(unitOf(units, hover.line))} · asks first` : `${unitLabel(unitOf(units, hover.line))} · ${mod}↵`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            const [a, b] = unitOf(units, hover.line)
            pointAt(a, b, unitMode(cell.code, [a, b]), 'line', hover.top)
          }}
        >
          <Play className="size-3 fill-current" aria-hidden />
        </button>
      )}
      {selMode && (
        <button
          type="button"
          contentEditable={false}
          className={cn(
            'absolute top-1 right-2 rounded-full border bg-layer-1 px-2 py-0.5 text-[11px] shadow-sm hover:bg-layer-2 pointer-coarse:min-h-12',
            danger ? 'border-danger text-danger' : 'border-border text-fg-secondary',
          )}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            const a = selLine?.type === 'line' ? selLine.line : 1
            pointAt(a, selEnd?.type === 'line' ? selEnd.line : a, selMode, 'selection')
          }}
        >
          {danger ? 'Run selection' : `Run selection ${mod}↵`}
        </button>
      )}
      {ask && (
        <LineConfirm
          top={topOf(ask.last)}
          what={ask.what}
          lines={[ask.line, ask.last]}
          title={title}
          reason={ask.reason}
          onCancel={() => step({ kind: 'cancel' })}
          onRun={(skip) => step({ kind: 'run', skip })}
        />
      )}
    </div>
  )

  const interpreter = record?.interpreter
  const chipText = interpreterText(cell, interpreter ?? null)
  const o = cell.options
  const cwd = o.cwd && /^[~/]/.test(o.cwd) ? o.cwd : (latest?.cwd ?? '~')
  const monty = o.python?.kind === 'monty' || interpreter?.kind === 'monty'
  // 171F B11: the cell the scope's last Run all pass stopped at (its reason), until a new pass starts.
  const passStop = useCells((st) => {
    const s = st.scopes[scopeKey(scope)]
    const stop = s && lastPass(s)?.stopped
    return stop && refKey(stop.cell) === key ? stop.why : null
  })
  const extras: CellExtras = {
    // A note cell never folds: the folded row has no editor slot, and BlockNote needs its content mounted.
    open: true,
    // 171F4 N3: the "#|" pill unfolds the header in the editor (the header is the note's text).
    headerInEditor: true,
    notice,
    hint: hint ?? (monty && latest?.state === 'running' ? 'Monty shows output when the run ends' : null),
    // §12.3: in a mesh the button names the machine it runs on (`this Mac` / `this PC` / `this computer`).
    runText: member ? `Run ${member}'s edit` : inMesh(mesh) && !cell.options.label ? `Run on ${local ? LOCAL_NAME.replace(/^This/, 'this') : mesh?.self_name}` : undefined,
    below: foreign ? <ForeignRun row={foreign} machine={machineName(mesh, foreign.engine)} /> : undefined,
    chip: cell.lang === 'python' && editable ? <PythonChip text={chipText} cwd={cwd} at={key} onPick={(v) => blocknote.updateBlock(block, { content: withPython(code, v) })} /> : undefined,
    // 171F4 N1: the stale mark as `doc_read` writes it (`ran 15:08 · … · ran an earlier version`), once: the footer
    // replaces the cell's notice row whenever it shows.
    footer:
      latest?.ended_ms != null ? (
        <span className="tabular-nums">
          ran {clock(latest.ended_ms)}
          {record?.stale && ' · ran an earlier version'}
        </span>
      ) : undefined,
    staleInFooter: latest?.ended_ms != null,
    rows: (
      <>
        {member && (
          <Notice tone="warning">
            {latest ? `Changed by ${member} since your last run` : `Written by ${member}`}
            <span aria-hidden>·</span>
            <button type="button" className="underline underline-offset-2 pointer-coarse:min-h-12" aria-expanded={review} onClick={() => setReview(!review)}>
              {latest ? 'Review diff' : 'Review'}
            </button>
          </Notice>
        )}
        {unsure && (
          <UnsureRun server={host?.provider.server ? machineName(mesh, host.provider.server) : 'the hub'} onRun={() => (setUnsure(null), run(unsure.mode, unsure.confirmed, unsure.text, '?'))} onCancel={() => setUnsure(null)} />
        )}
        {member && review && <MemberReview run={latest?.id ?? null} code={cell.code} />}
        {passStop !== null && <Notice tone="danger">Run all stopped here: {passStop}</Notice>}
        {/* 171F B10: Monty refused the cell (the reason is its problem row); a regular Python may run it. */}
        {editable && record?.problems.some((p) => p.code === 'monty_insufficient') && (
          <Notice tone="info">
            A regular Python can run it
            <span aria-hidden>·</span>
            <button type="button" className="underline underline-offset-2 pointer-coarse:min-h-12" onClick={() => blocknote.updateBlock(block, { content: withPython(code, 'auto') })}>
              Use Python
            </button>
          </Notice>
        )}
        {view === 'stale' && latest && (
          <Notice tone="muted">
            <button type="button" className="underline underline-offset-2 pointer-coarse:min-h-12" aria-expanded={ranCode} onClick={() => setRanCode(!ranCode)}>
              {ranCode ? 'hide the code that ran' : 'show the code that ran'}
            </button>
          </Notice>
        )}
        {view === 'stale' && latest && ranCode && <RanCode run={latest.id} lang={cell.lang} />}
      </>
    ),
  }

  return (
    <div data-note-cell="" data-doc={doc} data-ref-key={key} data-block={block} className={cn(passStop !== null && 'rounded-xl ring-2 ring-danger')}>
      <CellExtras value={extras}>
        <Cell
          cellRef={ref}
          cell={cell}
          code={code}
          runnable
          view={view}
          // The member line is the note's own row (names, not pub8s; "Written by" when never run).
          state={record && { ...record, member_edit: null }}
          latest={latest}
          host={record?.host ? { engine: record.host, label: record.host } : null}
          callbacks={callbacks}
          editor={slotted}
        />
      </CellExtras>
    </div>
  )
}
