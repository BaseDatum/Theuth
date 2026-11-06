// Owner: U1
// The chat cell's syntax colours: shiki tokens (`lib/shiki.ts`, the message fences' grammar and themes) applied as
// CodeMirror mark decorations. Plain text until the grammar loads; marks map through edits and re-tokenize after a
// pause in typing, so a keystroke never waits on the tokenizer.
import { Decoration, type DecorationSet, EditorView, type Extension, RangeSetBuilder, StateEffect, StateField, ViewPlugin } from '@uiw/react-codemirror'

/** A shiki token: its text and `htmlStyle` (`--shiki-light`/`--shiki-dark`, styles.css `.shiki` picks one). */
export type Token = { content: string; htmlStyle?: Record<string, string> | string }
export type Tokenize = (code: string) => Promise<Token[][]>

const setMarks = StateEffect.define<DecorationSet>()

/** The current marks; an edit maps them until the next tokenize replaces them. */
export const shikiField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update: (marks, tr) => tr.effects.find((e) => e.is(setMarks))?.value ?? marks.map(tr.changes),
  provide: (f) => EditorView.decorations.from(f),
})

const styleOf = (s: Token['htmlStyle']) => (typeof s === 'string' ? s : Object.entries(s ?? {}).map(([k, v]) => `${k}:${v}`).join(';'))

/** Marks for `tokens` (lines of the doc, split on `\n`); one `Decoration` per distinct style. */
export function marksOf(tokens: Token[][]): DecorationSet {
  const b = new RangeSetBuilder<Decoration>()
  const looks = new Map<string, Decoration>()
  let pos = 0
  for (const line of tokens) {
    for (const t of line) {
      const style = styleOf(t.htmlStyle)
      if (t.content && style) {
        let mark = looks.get(style)
        if (!mark) looks.set(style, (mark = Decoration.mark({ class: 'shiki', attributes: { style } })))
        b.add(pos, pos + t.content.length, mark)
      }
      pos += t.content.length
    }
    pos += 1
  }
  return b.finish()
}

type View = Pick<EditorView, 'state' | 'dispatch'>

/** Above either, the cell stays plain: one tokenize would block the page for longer than a frame many times over. */
export const MAX_CHARS = 100_000
export const MAX_LINES = 5_000

/** Tokenizes the doc now, then `delay` ms after the last edit; a result for an outdated doc is dropped. */
export class Retokenize {
  private edits = 0
  private timer: ReturnType<typeof setTimeout> | undefined
  private gone = false
  constructor(
    private view: View,
    private tokenize: Tokenize,
    private delay: number,
  ) {
    this.run()
  }
  private run() {
    const { doc } = this.view.state
    if (doc.length > MAX_CHARS || doc.lines > MAX_LINES) {
      // Grew past the cap: drop the marks it had (never the case at construction, where no dispatch may happen).
      if (this.view.state.field(shikiField).size) this.view.dispatch({ effects: setMarks.of(Decoration.none) })
      return
    }
    const at = this.edits
    this.tokenize(doc.toString()).then(
      (tokens) => !this.gone && at === this.edits && this.view.dispatch({ effects: setMarks.of(marksOf(tokens)) }),
      () => undefined, // no grammar: stays plain
    )
  }
  update(u: { docChanged: boolean }) {
    if (!u.docChanged) return
    this.edits++
    clearTimeout(this.timer)
    this.timer = setTimeout(() => this.run(), this.delay)
  }
  destroy() {
    this.gone = true
    clearTimeout(this.timer)
  }
}

/** The extension: `tokenize` is the cell's language bound (`tokensOf(code, lang)`). */
export const shikiMarks = (tokenize: Tokenize, delay = 120): Extension => [shikiField, ViewPlugin.define((v) => new Retokenize(v, tokenize, delay))]
