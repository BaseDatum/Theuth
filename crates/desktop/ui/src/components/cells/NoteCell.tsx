// Owner: U2
import type { BlockNoteEditor } from '@blocknote/core'
import { useEditorState } from '@blocknote/react'
import { useQuery } from '@tanstack/react-query'
import { ChevronDown, Play } from 'lucide-react'
import { type MouseEvent, type ReactNode, useContext, useEffect, useRef, useState } from 'react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isApple } from '@/lib/chord'
import { useApi } from '@/lib/rpc'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import type { Api } from '@/rpc/api'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellRunMode } from '@/rpc/bindings/CellRunMode'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { DocId } from '@/rpc/bindings/DocId'
import type { NoteKey } from '@/rpc/bindings/NoteKey'
import { Cell, CellExtras, Chip, Notice } from './Cell'
import { digest } from './grammar'
import { MARK_LOOK, useLineRun } from './line-run'
import { lineDiff, lineStart, memberName, modeOf, pythonValue, withPython } from './note-edit'
import { NoteHost } from './note-context'
import { clock, errorText, lastPass, mintId, press, refKey, refresh, register, scopeKey, scopeOf, setNotice, useCellRecord, useCells } from './store'
import { interpreterText, labelOf, titleOf, viewOf } from './view'
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
 * on transport failures, no edited text (the note IS the text), and `member_ack` = the member's pub8 when the engine
 * reported a member edit (invariant 6, "Run Alice's edit").
 */
export async function pressNote(api: Pick<Api, 'cell_run'>, ref: CellRef, text: string, mode: CellRunMode, memberAck: string | null) {
  const seen = await digest(text)
  return press(mintId('r_'), (id) => api.cell_run(ref, seen, mode, id, null, memberAck))
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
function RanCode({ run }: { run: string }) {
  const api = useApi()
  const q = useQuery({ queryKey: ['cells', 'code', run], queryFn: async () => decode(await api.cell_output_file(run, 'code')), staleTime: Infinity })
  if (q.error) return <div className="px-3 py-1 text-xs text-danger">{errorText(q.error)}</div>
  return <pre className="mx-3 mb-2 max-h-64 overflow-auto rounded-md border border-border-subtle px-2 py-1 font-mono text-xs text-fg-secondary">{q.data ?? 'loading…'}</pre>
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
  // 171F (B10 proof): a header edit (Use Python, the chip menu, typed `#|` lines) changes validation; re-read the
  // record once the engine has the new text. Header only: code keystrokes still never re-read.
  const header = code.match(/^(?:#\|.*(?:\n|$))*/)?.[0] ?? ''
  const seenHeader = useRef(header)
  useEffect(() => {
    if (seenHeader.current === header) return
    seenHeader.current = header
    const t = setTimeout(() => void refresh(scope), 800)
    return () => clearTimeout(t)
  }, [header])

  const { state: record, notice } = useCellRecord(ref)
  const latest = record?.latest ?? null
  const pub8 = record?.member_edit ?? null
  const member = pub8 ? memberName(pub8, host?.provider.meta?.members) : null
  const view = viewOf(record, cell, false)
  const danger = cell.options.danger
  const [hint, setHint] = useState<string | null>(null)
  const [review, setReview] = useDisclosure(disclosureKey(key, 'review'))
  const [ranCode, setRanCode] = useDisclosure(disclosureKey(key, 'ran-code'))
  useEffect(() => {
    if (!hint) return
    const t = setTimeout(() => setHint(null), 4000)
    return () => clearTimeout(t)
  }, [hint])

  const act = (f: () => Promise<unknown>) => {
    setNotice(ref, null)
    f().then(
      () => refresh(scope),
      (e: unknown) => {
        setNotice(ref, errorText(e))
        void refresh(scope)
      },
    )
  }
  const run = (mode: CellRunMode) => {
    const pressed = pressNote(api, ref, code, mode, pub8)
    act(() => pressed)
    return pressed
  }
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
  // nothing); a repeat press of a line whose run is in flight does nothing.
  const slot = useRef<HTMLDivElement>(null)
  const lineRun = useLineRun(latest)
  const markTop = useRef(0)
  const runAt = (line: number, mode: CellRunMode, top?: number) => {
    markTop.current = top ?? lineTop(slot.current, lineStart(code, cell, line))
    lineRun.run(line, () => run(mode))
  }

  // §7.1/§11.7 keys inside the code block: Shift+Enter runs the cell and moves to the next one, Mod+Enter runs the
  // selection or the caret line, Mod+. stops. The caret is in BlockNote's root editable, so the listener sits on the
  // document (capture: before BlockNote's own code-block keys).
  // `editor` is null in a server render (BlockNote's snapshot before mount).
  const sel = useEditorState({ editor: blocknote, selector: ({ editor: e }) => (e ? selectionIn(e.prosemirrorState.selection, block) : null) })
  const keys = useRef({ code, cell, run, runAt, view, latest, label, stop: callbacks.onStop })
  keys.current = { code, cell, run, runAt, view, latest, label, stop: callbacks.onStop }
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
      if (k.cell.options.danger || k.view === 'disarmed' || k.view === 'invalid') return setHint(`press ${k.label} to run this`)
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
      if (mode) k.runAt(caret?.type === 'line' ? caret.line : 1, mode)
      else setHint('put the caret on a code line to run it')
    }
    document.addEventListener('keydown', onKey, true)
    return () => document.removeEventListener('keydown', onKey, true)
    // The latest values ride `keys`; the listener is per block.
  }, [blocknote, block])

  // The gutter ► (§11.1): a positioned overlay on the hovered code line, absent on danger cells.
  const [hover, setHover] = useState<{ line: number; top: number } | null>(null)
  const onMove = (e: MouseEvent<HTMLDivElement>) => {
    if (danger || (e.target instanceof Element && e.target.closest('button'))) return
    const codeEl = slot.current?.querySelector('code')
    const at = caretAt(e.clientX, e.clientY)
    // 171F B2: off the code text (the padding band before the ►) keeps the hover; leaving the slot clears it.
    if (!codeEl || !slot.current || !at || !codeEl.contains(at.node)) return
    const range = document.createRange()
    range.setStart(codeEl, 0)
    range.setEnd(at.node, at.offset)
    const offset = range.toString().length
    const mode = modeOf(code, cell, offset, offset)
    if (mode?.type !== 'line') return setHover(null)
    range.collapse(false)
    const box = range.getBoundingClientRect()
    const top = Math.round((box.height ? box.top : e.clientY - 10) - slot.current.getBoundingClientRect().top)
    if (hover?.line !== mode.line || hover.top !== top) setHover({ line: mode.line, top })
  }
  const [selFrom, selTo] = sel ? (sel.split(':').map(Number) as [number, number]) : [0, 0]
  const selMode = sel && selFrom !== selTo ? modeOf(code, cell, selFrom, selTo) : null
  const selLine = modeOf(code, cell, Math.min(selFrom, selTo), Math.min(selFrom, selTo))
  const mod = isApple() ? '⌘' : 'Ctrl'
  const mark = danger ? null : lineRun.mark

  const slotted = (
    <div ref={slot} className="relative" onMouseMove={onMove} onMouseLeave={() => setHover(null)}>
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
      {hover && !danger && hover.line !== mark?.line && (
        <button
          type="button"
          contentEditable={false}
          className="absolute left-1.5 flex size-5 items-center justify-center rounded text-fg-accent hover:bg-layer-2 pointer-coarse:size-12"
          style={{ top: hover.top }}
          aria-label={`Run line ${hover.line}`}
          title={`Run line ${hover.line} · ${mod}↵`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => runAt(hover.line, { type: 'line', line: hover.line }, hover.top)}
        >
          <Play className="size-3 fill-current" aria-hidden />
        </button>
      )}
      {selMode && !danger && (
        <button
          type="button"
          contentEditable={false}
          className="absolute top-1 right-2 rounded-full border border-border bg-layer-1 px-2 py-0.5 text-[11px] text-fg-secondary shadow-sm hover:bg-layer-2 pointer-coarse:min-h-12"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => runAt(selLine?.type === 'line' ? selLine.line : 1, selMode)}
        >
          Run selection {mod}↵
        </button>
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
    notice,
    hint: hint ?? (monty && latest?.state === 'running' ? 'Monty shows output when the run ends' : null),
    runText: member ? `Run ${member}'s edit` : undefined,
    chip: cell.lang === 'python' && editable ? <PythonChip text={chipText} cwd={cwd} at={key} onPick={(v) => blocknote.updateBlock(block, { content: withPython(code, v) })} /> : undefined,
    footer: latest?.ended_ms != null ? <span className="tabular-nums">ran {clock(latest.ended_ms)}</span> : undefined,
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
        {view === 'stale' && latest && ranCode && <RanCode run={latest.id} />}
      </>
    ),
  }

  return (
    <div data-note-cell="" data-ref-key={key} data-block={block} className={cn(passStop !== null && 'rounded-xl ring-2 ring-danger')}>
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
