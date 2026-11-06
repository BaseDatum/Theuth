import { EditorState } from '@uiw/react-codemirror'
import { expect, test, vi } from 'vitest'
import { tokensOf } from '@/lib/shiki'
import { MAX_CHARS, MAX_LINES, Retokenize, shikiField, type Token, type Tokenize } from './shiki-marks'

const SAMPLE = 'aws s3 ls --profile example "exit $?" $HOME # note'

/** A view for `Retokenize`: the state and dispatch, no DOM. */
function viewOf(doc: string) {
  const v = {
    state: EditorState.create({ doc, extensions: [shikiField] }),
    dispatch: (spec: object) => (v.state = v.state.update(spec).state),
  }
  return v
}

/** The mark style over `text`'s first occurrence (one token), or `null`. */
function styleAt(state: EditorState, text: string) {
  const from = state.doc.toString().indexOf(text)
  let style: string | null = null
  state.field(shikiField).between(from, from + text.length, (a, b, d) => {
    if (a <= from && from + text.length <= b) style = d.spec.attributes.style
  })
  return style
}

const colours = (style: string | null) => Object.fromEntries((style ?? '').split(';').map((kv) => kv.split(':')))

test('bash: a command, a flag, a string, a $VAR and a comment are marked with light and dark colours', async () => {
  const v = viewOf(SAMPLE)
  new Retokenize(v, (code) => tokensOf(code, 'bash'), 0)
  await vi.waitFor(() => expect(styleAt(v.state, 'aws')).not.toBeNull(), { timeout: 10_000 })
  const plain = colours(styleAt(v.state, ' '))
  const seen = new Set<string>()
  for (const part of ['aws', '--profile', '"exit ', '$?', '$HOME', '# note']) {
    const c = colours(styleAt(v.state, part))
    expect(c['--shiki-light'], part).toMatch(/^#[0-9A-F]{6}$/i)
    expect(c['--shiki-dark'], part).toMatch(/^#[0-9A-F]{6}$/i)
    // github-light/dark leave a plain `$NAME` at the foreground (as the message fences do); `$?` is a constant.
    if (part !== '$HOME') expect(c, part).not.toEqual(plain)
    seen.add(`${c['--shiki-light']}/${c['--shiki-dark']}`)
  }
  // A flag and `$?` share the constant colour; `$HOME` is the foreground; the rest differ.
  expect(seen.size).toBe(5)
})

test('an edit maps the marks, re-tokenizes the new doc after the pause, and drops a result for an outdated doc', async () => {
  vi.useFakeTimers()
  try {
    const asked: string[] = []
    const pending: ((t: Token[][]) => void)[] = []
    const tokenize: Tokenize = (code) => {
      asked.push(code)
      return new Promise((r) => pending.push(r))
    }
    const v = viewOf('echo hi')
    const r = new Retokenize(v, tokenize, 100)
    expect(asked).toEqual(['echo hi'])
    // The first result arrives after an edit: it is for the old doc, so nothing is applied.
    v.dispatch({ changes: { from: 0, insert: 'x' } })
    r.update({ docChanged: true })
    pending[0]!([[{ content: 'echo', htmlStyle: { '--shiki-light': '#000001' } }, { content: ' hi' }]])
    await vi.advanceTimersByTimeAsync(0)
    expect(v.state.field(shikiField).size).toBe(0)
    // The pause passes: the new doc is tokenized and its marks applied.
    await vi.advanceTimersByTimeAsync(100)
    expect(asked).toEqual(['echo hi', 'xecho hi'])
    pending[1]!([[{ content: 'xecho', htmlStyle: { '--shiki-light': '#000002' } }, { content: ' hi' }]])
    await vi.advanceTimersByTimeAsync(0)
    expect(styleAt(v.state, 'xecho')).toBe('--shiki-light:#000002')
    // Until the next result the marks follow the text.
    v.dispatch({ changes: { from: 0, insert: '  ' } })
    r.update({ docChanged: true })
    expect(styleAt(v.state, 'xecho')).toBe('--shiki-light:#000002')
    r.destroy()
    await vi.advanceTimersByTimeAsync(200)
    expect(asked).toHaveLength(2)
  } finally {
    vi.useRealTimers()
  }
})

test('a result that arrives after destroy is dropped without throwing', async () => {
  let resolve: (t: Token[][]) => void = () => undefined
  const v = viewOf('echo hi')
  const dispatch = vi.spyOn(v, 'dispatch')
  const r = new Retokenize(v, () => new Promise((ok) => (resolve = ok)), 0)
  r.destroy()
  resolve([[{ content: 'echo', htmlStyle: { '--shiki-light': '#000001' } }]])
  await Promise.resolve()
  await Promise.resolve()
  expect(dispatch).not.toHaveBeenCalled()
})

test('above MAX_LINES the cell stays plain, and one that grows past it drops its marks', async () => {
  const tokenize = vi.fn<Tokenize>(async (code) => code.split('\n').map((l) => [{ content: l, htmlStyle: { '--shiki-light': '#000001' } }]))
  new Retokenize(viewOf('x\n'.repeat(MAX_LINES)), tokenize, 0)
  expect(tokenize).not.toHaveBeenCalled()
  vi.useFakeTimers()
  try {
    const v = viewOf('x')
    const r = new Retokenize(v, tokenize, 0)
    await vi.advanceTimersByTimeAsync(0)
    expect(v.state.field(shikiField).size).toBe(1)
    v.dispatch({ changes: { from: 1, insert: 'y'.repeat(MAX_CHARS) } })
    r.update({ docChanged: true })
    await vi.advanceTimersByTimeAsync(0)
    expect(v.state.field(shikiField).size).toBe(0)
    expect(tokenize).toHaveBeenCalledTimes(1)
  } finally {
    vi.useRealTimers()
  }
})
