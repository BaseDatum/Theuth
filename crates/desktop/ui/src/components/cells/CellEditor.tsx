// Owner: U1
// 171 §11.1 editor + gutter, §7.1 modes, §11.7 keys: the chat cell's CodeMirror. Click and type to edit (F4) unless
// `locked`; Esc leaves the editor; ► on the hovered command (`units.ts`) runs it; a selection shows the "Run selection"
// pill. Danger and disarmed cells never run from the keys (a hint instead); on a danger cell the ► and the pill ask
// first (`LineConfirm`), on a disarmed one they warn and run on the second press (CELLAFTER). A run marks its line.
import { StateEffect, StateField } from '@codemirror/state'
import CodeMirror, { EditorView, GutterMarker, Prec, gutter, keymap, lineNumbers } from '@uiw/react-codemirror'
import { Play } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { isApple } from '@/lib/chord'
import { tokensOf } from '@/lib/shiki'
import { cn } from '@/lib/utils'
import type { CellRun } from "@/rpc/bindings/method.cells.output"
import type { RunMode as CellRunMode } from "@/rpc/bindings/method.cells.output"
import type { Lang } from '@/rpc/bindings/auxiliary.surface.output'
import { CONFIRM_ROOM, LineConfirm } from './LineConfirm'
import { type Ask, askSkipped, askStep, type HoverEvent, type LineMark, lineGuard, MARK_LOOK, hoverAfter, useLineRun } from './line-run'
import { commandUnits, unitMode, unitOf } from './units'
import { shikiMarks } from './shiki-marks'
import { cellTheme } from './theme'

/** UTF-8 byte offset of a code position (RunMode offsets count the code's bytes, header excluded). */
const bytesBefore = (doc: string, pos: number) => new TextEncoder().encode(doc.slice(0, pos)).length

const hover = StateEffect.define<number | null>()
const pointer = (v: EditorView, e: HoverEvent) => {
  const next = hoverAfter(v.state.field(hovered), e)
  if (next !== v.state.field(hovered)) v.dispatch({ effects: hover.of(next) })
}

const hovered = StateField.define<number | null>({
  create: () => null,
  update: (v, tr) => {
    for (const e of tr.effects) if (e.is(hover)) return e.value
    return v
  },
})
class TextMarker extends GutterMarker {
  constructor(
    readonly text: string,
    readonly tip: string,
    readonly tone = '',
  ) {
    super()
  }
  override eq(other: GutterMarker): boolean {
    return other === this
  }
  override toDOM() {
    const el = document.createElement('span')
    el.textContent = this.text
    el.title = this.tip
    if (this.tone) el.className = this.tone
    return el
  }
}
const RUN = new TextMarker('►', 'Run this line')
const RUN_DANGER = new TextMarker('►', 'Run this line · asks first', 'text-danger')
const MARKS = Object.fromEntries(Object.entries(MARK_LOOK).map(([k, l]) => [k, new TextMarker(l.glyph, l.text, l.tone)])) as Record<LineMark['phase'], TextMarker>

/**
 * Line `n`'s gutter: its run mark, else ► when `hovered` is `n` on a runnable cell — danger-toned on a danger cell
 * (CELLAFTER: danger cells get the ► too; its first press warns).
 */
export function gutterMark(mark: LineMark | null, n: number, hovered: number | false | null, danger: boolean): TextMarker | null {
  if (mark?.line === n) return MARKS[mark.phase]
  return hovered === n ? (danger ? RUN_DANGER : RUN) : null
}
const setMark = StateEffect.define<LineMark | null>()
const marked = StateField.define<LineMark | null>({
  create: () => null,
  update: (v, tr) => {
    for (const e of tr.effects) if (e.is(setMark)) return e.value
    return v
  },
})

export type CellEditorProps = {
  lang: Lang
  code: string
  /** The press is possible at all (valid, not superseded, the reply has ended, not a read-only view). */
  runnable: boolean
  /** Keys refuse (`onRefused` explains, §7.1); a ► or the pill asks first (`LineConfirm`) unless "Don't ask again". */
  danger: boolean
  /** CELLAFTER (§7.6): the unarmed `after` gate's reason. Keys refuse (`onRefused`); a ► warns, its second press runs. */
  disarmed: string | null
  /** The cell's title, named by the danger ask. */
  title: string
  /** `cellIdentity`: the key of "Don't ask again for this cell". */
  identity: string
  /** No edits (not runnable, a run is queued or live, superseded, invalid). */
  locked: boolean
  onChange(code: string): void
  /** The cell's latest run: settles a line/selection run's gutter mark. */
  latest: CellRun | null
  /** `confirmed` only past a warning (the second ►, the danger confirm or its "Don't ask again"); keys never pass it.
   * `text`: the code the press was made on (a danger ask's press-time text), else the code on screen. */
  onRun(mode: CellRunMode, next?: boolean, confirmed?: boolean, text?: string): Promise<CellRun> | undefined
  onRefused(): void
}

/**
 * §7.1, §11.7: a keyboard or gutter run happens only on a runnable, non-danger cell; a danger cell explains instead
 * ("press <label> to run this"), as does a keyboard run of a disarmed one. Always handled.
 */
export function keyRun(
  cell: Pick<CellEditorProps, 'runnable' | 'danger' | 'disarmed' | 'onRefused'> & { onRun(mode: CellRunMode, next?: boolean): unknown },
  mode: CellRunMode,
  next?: boolean,
): true {
  if (!cell.runnable) return true
  if (cell.danger || cell.disarmed) cell.onRefused()
  else cell.onRun(mode, next)
  return true
}

/** The chat cell's editor; extensions are memoised on `lang`/`danger`, callbacks read through a ref. */
export function CellEditor({ lang, code, runnable, danger, disarmed, title, identity, locked, latest, onChange, onRun, onRefused }: CellEditorProps) {
  const [selection, setSelection] = useState<{ from: number; to: number } | null>(null)
  const [ask, setAsk] = useState<Ask | null>(null)
  const view = useRef<EditorView | null>(null)
  const box = useRef<HTMLDivElement>(null)
  const lineRun = useLineRun(latest)
  // CELLAFTER: the ► runs a command (`units.ts`), marked on its first line.
  const units = useMemo(() => commandUnits(code, lang), [code, lang])
  const live = useRef({ code, runnable, danger, disarmed, identity, units, onRun, onRefused, lineRun })
  live.current = { code, runnable, danger, disarmed, identity, units, onRun, onRefused, lineRun }
  // M1: any change of the code (an F4 edit) closes an open ask: the run would no longer be what was confirmed.
  useEffect(() => setAsk((a) => a && askStep(a, { kind: 'code', text: code }).ask), [code])
  useEffect(() => view.current?.dispatch({ effects: setMark.of(lineRun.mark) }), [lineRun.mark])
  /** A line/selection run marks `line`; a repeat press while it is in flight does nothing. Reads only `live`, so the memoised keymap may hold it. */
  const runAt = (line: number, mode: CellRunMode) => keyRun({ ...live.current, onRun: (m) => void live.current.lineRun.run(line, () => live.current.onRun(m)) }, mode)
  /** A ► or the pill (not a key): a danger cell asks first, a disarmed one warns first (`lineGuard`). */
  const pointAt = (line: number, last: number, mode: CellRunMode, what: 'line' | 'selection') => {
    const l = live.current
    if (!l.runnable) return true
    const g = lineGuard(l.danger, askSkipped(l.identity), l.disarmed)
    if (g.kind === 'confirm') setAsk({ line, last, mode, what, text: l.code, reason: g.reason, identity: l.identity })
    // A warned ► runs on its second press, past the gate; a danger ► the user stopped being asked about runs confirmed.
    else l.lineRun.run(line, () => l.onRun(mode, false, g.kind === 'warn' || l.danger), g.kind === 'warn' ? g.text : null)
    return true
  }

  const extensions = useMemo(() => {
    const run = (mode: CellRunMode, next?: boolean) => keyRun(live.current, mode, next)
    const caret = (v: EditorView) => {
      const s = v.state.selection.main
      const doc = v.state.doc.toString()
      if (s.empty) {
        // The caret line runs its whole command, as its ► does.
        const unit = unitOf(live.current.units, v.state.doc.lineAt(s.head).number)
        return runAt(unit[0], unitMode(doc, unit))
      }
      return runAt(v.state.doc.lineAt(s.from).number, { type: 'selection', from: bytesBefore(doc, s.from), to: bytesBefore(doc, s.to) })
    }
    return [
      ...cellTheme,
      shikiMarks((code) => tokensOf(code, lang)),
      lineNumbers(),
      hovered,
      marked,
      gutter({
        class: 'cm-run-gutter',
        lineMarker: (v, line) => {
          const h = v.state.field(hovered)
          // Any line of the hovered command shows the one ► on its first line.
          const start = live.current.runnable && h !== null && unitOf(live.current.units, h)[0]
          return gutterMark(v.state.field(marked), v.state.doc.lineAt(line.from).number, start, danger)
        },
        lineMarkerChange: (u) => u.transactions.some((tr) => tr.effects.some((e) => e.is(hover) || e.is(setMark))),
        domEventHandlers: {
          mousedown: (v, line) => {
            const unit = unitOf(live.current.units, v.state.doc.lineAt(line.from).number)
            return pointAt(unit[0], unit[1], unitMode(v.state.doc.toString(), unit), 'line')
          },
          mousemove: (v, line) => {
            pointer(v, { kind: 'move', line: v.state.doc.lineAt(line.from).number })
            return false
          },
        },
      }),
      EditorView.domEventHandlers({
        mousemove: (e, v) => {
          const pos = v.posAtCoords({ x: e.clientX, y: e.clientY })
          pointer(v, { kind: 'move', line: pos === null ? null : v.state.doc.lineAt(pos).number })
        },
        mouseleave: (e, v) => pointer(v, { kind: 'leave', inEditor: e.relatedTarget instanceof Node && v.dom.contains(e.relatedTarget) }),
      }),
      Prec.highest(
        keymap.of([
          { key: 'Shift-Enter', run: () => run({ type: 'cell' }, true) },
          { key: 'Mod-Enter', run: caret },
          {
            key: 'Escape',
            run: (v) => {
              v.contentDOM.blur()
              return true
            },
          },
        ]),
      ),
    ]
  }, [lang, danger])

  const mod = isApple() ? '⌘' : 'Ctrl'
  // A code line's top in this box: CodeMirror's block top (document coordinates) moved to the viewport.
  const topOf = (v: EditorView, b: HTMLElement, line: number) =>
    v.lineBlockAt(v.state.doc.line(Math.min(line, v.state.doc.lines)).from).top + v.documentTop - b.getBoundingClientRect().top
  return (
    // Leaving the whole editor clears the ► (the line-number gutter has no handler of its own).
    <div ref={box} className={cn('relative', ask && CONFIRM_ROOM)} onMouseLeave={() => view.current && pointer(view.current, { kind: 'leave', inEditor: false })}>
      <CodeMirror
        value={code}
        basicSetup={false}
        theme="none"
        editable
        readOnly={locked}
        extensions={extensions}
        onChange={onChange}
        onCreateEditor={(v) => (view.current = v)}
        onUpdate={(u) => {
          if (!u.selectionSet) return
          const s = u.state.selection.main
          setSelection(s.empty ? null : { from: s.from, to: s.to })
        }}
        aria-label={locked ? 'Cell code' : 'Cell code, editable'}
      />
      {lineRun.warn && view.current && box.current && (
        <span
          className="pointer-events-none absolute right-2 z-10 rounded bg-warning-bg px-2 py-0.5 text-[11px] text-warning shadow-sm"
          style={{ top: topOf(view.current, box.current, lineRun.warn.line) }}
        >
          {lineRun.warn.text}
        </span>
      )}
      {ask && view.current && box.current && (
        <LineConfirm
          top={topOf(view.current, box.current, ask.last)}
          what={ask.what}
          lines={[ask.line, ask.last]}
          title={title}
          reason={ask.reason}
          onCancel={() => setAsk(null)}
          onRun={(skip) => {
            const s = askStep(ask, { kind: 'run', skip })
            setAsk(s.ask)
            const r = s.run
            if (r) lineRun.run(r.line, () => onRun(r.mode, false, true, r.text))
          }}
        />
      )}
      {selection && runnable && (
        <Button
          size="sm"
          variant={danger ? 'outline' : 'secondary'}
          className={cn('absolute top-1 right-2 h-7 rounded-full px-3 text-xs pointer-coarse:h-12', danger && 'border-danger text-danger')}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            const v = view.current
            const doc = v?.state.doc.toString() ?? code
            const first = v?.state.doc.lineAt(selection.from).number ?? 1
            pointAt(first, v?.state.doc.lineAt(selection.to).number ?? first, { type: 'selection', from: bytesBefore(doc, selection.from), to: bytesBefore(doc, selection.to) }, 'selection')
          }}
        >
          <Play className="size-3" aria-hidden />
          Run selection {!danger && <kbd className="font-sans text-fg-tertiary">{mod}↵</kbd>}
        </Button>
      )}
    </div>
  )
}
