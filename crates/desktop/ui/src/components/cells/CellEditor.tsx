// Owner: U1
// 171 §11.1 editor + gutter, §7.1 modes, §11.7 keys: the chat cell's CodeMirror. Click and type to edit (F4) unless
// `locked`; Esc leaves the editor; ► on the hovered line runs it; a selection shows the "Run selection" pill. Danger
// cells run from their button only: the keys show a hint instead. A line/selection run marks its line (`line-run.ts`).
import { type Extension, StateEffect, StateField } from '@codemirror/state'
import { langs } from '@uiw/codemirror-extensions-langs'
import CodeMirror, { EditorView, GutterMarker, Prec, gutter, keymap, lineNumbers } from '@uiw/react-codemirror'
import { Play } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { isApple } from '@/lib/chord'
import type { CellRun } from '@/rpc/bindings/CellRun'
import type { CellRunMode } from '@/rpc/bindings/CellRunMode'
import type { Lang } from '@/rpc/bindings/Lang'
import { type HoverEvent, type LineMark, MARK_LOOK, hoverAfter, useLineRun } from './line-run'
import { cellTheme } from './theme'

const LANGUAGE: Partial<Record<Lang, () => Extension>> = { bash: langs.bash, zsh: langs.sh, sh: langs.sh, pwsh: langs.ps1, python: langs.python }

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
const MARKS = Object.fromEntries(Object.entries(MARK_LOOK).map(([k, l]) => [k, new TextMarker(l.glyph, l.text, l.tone)])) as Record<LineMark['phase'], TextMarker>
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
  /** Keys and line/selection runs refuse; `onRefused` explains (§7.1). */
  danger: boolean
  /** No edits (not runnable, a run is queued or live, superseded, invalid). */
  locked: boolean
  onChange(code: string): void
  /** The cell's latest run: settles a line/selection run's gutter mark. */
  latest: CellRun | null
  onRun(mode: CellRunMode, next?: boolean): Promise<CellRun> | undefined
  onRefused(): void
}

/** §7.1, §11.7: a keyboard or gutter run happens only on a runnable, non-danger cell; a danger cell explains instead ("press <label> to run this"). Always handled. */
export function keyRun(cell: Pick<CellEditorProps, 'runnable' | 'danger' | 'onRefused'> & { onRun(mode: CellRunMode, next?: boolean): unknown }, mode: CellRunMode, next?: boolean): true {
  if (!cell.runnable) return true
  if (cell.danger) cell.onRefused()
  else cell.onRun(mode, next)
  return true
}

/** The chat cell's editor; extensions are memoised on `lang`/`danger`, callbacks read through a ref. */
export function CellEditor({ lang, code, runnable, danger, locked, latest, onChange, onRun, onRefused }: CellEditorProps) {
  const [selection, setSelection] = useState<{ from: number; to: number } | null>(null)
  const view = useRef<EditorView | null>(null)
  const lineRun = useLineRun(latest)
  const live = useRef({ runnable, danger, onRun, onRefused, lineRun })
  live.current = { runnable, danger, onRun, onRefused, lineRun }
  useEffect(() => view.current?.dispatch({ effects: setMark.of(lineRun.mark) }), [lineRun.mark])
  /** A line/selection run marks `line`; a repeat press while it is in flight does nothing. Reads only `live`, so the memoised keymap may hold it. */
  const runAt = (line: number, mode: CellRunMode) => keyRun({ ...live.current, onRun: (m) => void live.current.lineRun.run(line, () => live.current.onRun(m)) }, mode)

  const extensions = useMemo(() => {
    const run = (mode: CellRunMode, next?: boolean) => keyRun(live.current, mode, next)
    const caret = (v: EditorView) => {
      const s = v.state.selection.main
      const doc = v.state.doc.toString()
      return runAt(
        v.state.doc.lineAt(s.from).number,
        s.empty ? { type: 'line', line: v.state.doc.lineAt(s.head).number } : { type: 'selection', from: bytesBefore(doc, s.from), to: bytesBefore(doc, s.to) },
      )
    }
    const support = LANGUAGE[lang]?.()
    return [
      ...cellTheme,
      ...(support ? [support] : []),
      lineNumbers(),
      hovered,
      marked,
      ...(danger
        ? []
        : [
            gutter({
              class: 'cm-run-gutter',
              lineMarker: (v, line) => {
                const n = v.state.doc.lineAt(line.from).number
                const m = v.state.field(marked)
                if (m?.line === n) return MARKS[m.phase]
                return live.current.runnable && v.state.field(hovered) === n ? RUN : null
              },
              lineMarkerChange: (u) => u.transactions.some((tr) => tr.effects.some((e) => e.is(hover) || e.is(setMark))),
              domEventHandlers: {
                mousedown: (v, line) => {
                  const n = v.state.doc.lineAt(line.from).number
                  return runAt(n, { type: 'line', line: n })
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
          ]),
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
  return (
    // Leaving the whole editor clears the ► (the line-number gutter has no handler of its own).
    <div className="relative" onMouseLeave={() => view.current && pointer(view.current, { kind: 'leave', inEditor: false })}>
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
      {selection && runnable && !danger && (
        <Button
          size="sm"
          variant="secondary"
          className="absolute top-1 right-2 h-7 rounded-full px-3 text-xs pointer-coarse:h-12"
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            const v = view.current
            const doc = v?.state.doc.toString() ?? code
            runAt(v?.state.doc.lineAt(selection.from).number ?? 1, { type: 'selection', from: bytesBefore(doc, selection.from), to: bytesBefore(doc, selection.to) })
          }}
        >
          <Play className="size-3" aria-hidden />
          Run selection <kbd className="font-sans text-fg-tertiary">{mod}↵</kbd>
        </Button>
      )}
    </div>
  )
}
